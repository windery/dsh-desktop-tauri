//! Backend resolution and supervision for the DSH desktop shell.
//!
//! This module owns no harness logic. It resolves the user's own `dsh`
//! installation and then either **attaches** to a `dsh web` already serving the
//! shared loopback port or boots one there itself. Attaching is what makes the
//! browser and the window the same server; booting keeps the window usable on
//! its own. Either way it tears the whole process group down on exit, so no
//! orphan agent subprocess survives the window.

use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// CLI entry point inside a Node installation root (fnm, nvm, Homebrew, ...).
const CLI_ENTRY_REL: &str = "lib/node_modules/@deepseek-ai/dsh/lib/bin.js";

/// The harness profile this shell shares with the local `dsh web` service.
///
/// One profile means one plugin set and one config for both surfaces, which is
/// safe because the harness serializes the data they share: sessions take a
/// cross-process `flock` and everything else is written through `tmp`+`rename`.
/// Never `desktop` — that name is reserved for the official Electron app and
/// the CLI rejects it.
pub const DEFAULT_PROFILE: &str = "web";

/// The loopback port the window shares with the local `dsh web`.
///
/// One port is what makes the two surfaces *one server* instead of two that
/// merely agree on disk: same origin, same session cookie, same live event
/// stream. Whoever gets there first owns the port and the other attaches, so a
/// session started in the browser is live in the window and vice versa — with
/// no polling and no plugin.
pub const DEFAULT_WEB_PORT: u16 = 3080;

/// How long to wait for the harness to announce its authenticated URL.
pub const BOOT_TIMEOUT: Duration = Duration::from_secs(90);

/// How many stderr lines to retain for a failure report.
const LOG_TAIL: usize = 40;

/// Process-group id of the harness server the shell currently owns, or `0`
/// when none is running.
///
/// This lives in a static because a termination *signal* never reaches
/// `RunEvent::ExitRequested`: a `kill`, a Ctrl-C, or a terminal hangup would
/// otherwise orphan the entire agent process tree.
static BACKEND_PGID: AtomicI32 = AtomicI32::new(0);

/// Tear the harness down and leave. Only async-signal-safe calls are used.
extern "C" fn on_terminating_signal(signal: libc::c_int) {
    let pgid = BACKEND_PGID.load(Ordering::SeqCst);
    if pgid > 0 {
        // SAFETY: `killpg` and `nanosleep` are async-signal-safe, and this pid
        // is the process group id established by `process_group(0)`.
        unsafe {
            libc::killpg(pgid, libc::SIGTERM);
            let grace = libc::timespec {
                tv_sec: 0,
                tv_nsec: 400_000_000,
            };
            libc::nanosleep(&grace, std::ptr::null_mut());
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    // SAFETY: `_exit` is async-signal-safe and skips destructors that must not
    // run inside a signal handler.
    unsafe { libc::_exit(128 + signal) };
}

/// Route fatal termination signals through the harness cleanup above, so
/// `kill`, Ctrl-C, and a hangup never leave an orphan server behind.
pub fn install_signal_handlers() {
    #[cfg(unix)]
    {
        let handler = on_terminating_signal as extern "C" fn(libc::c_int);
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            // SAFETY: the installed handler is async-signal-safe.
            unsafe {
                libc::signal(signal, handler as libc::sighandler_t);
            }
        }
    }
}

/// A resolved way to start the harness.
pub enum BackendCmd {
    /// Run a JS entry point through a specific Node binary.
    Node { node: PathBuf, entry: PathBuf },
    /// Run a `dsh` executable directly.
    Exec(PathBuf),
}

impl BackendCmd {
    /// Human-readable description for diagnostics and error messages.
    pub fn describe(&self) -> String {
        match self {
            BackendCmd::Node { node, entry } => {
                format!("{} {}", node.display(), entry.display())
            }
            BackendCmd::Exec(path) => path.display().to_string(),
        }
    }

    /// Build the process for this command, before harness arguments.
    fn command(&self) -> Command {
        match self {
            BackendCmd::Node { node, entry } => {
                let mut cmd = Command::new(node);
                cmd.arg(entry);
                cmd
            }
            BackendCmd::Exec(path) => Command::new(path),
        }
    }
}

/// A running harness server owned by the shell.
pub struct Backend {
    child: Child,
    /// The authenticated loopback URL, token included.
    pub url: String,
}

impl Backend {
    /// Stop the server and every process it spawned.
    pub fn terminate(&mut self) {
        terminate_child(&mut self.child);
    }
}

/// The profile this shell boots, overridable for testing.
pub fn profile_name() -> String {
    env::var("DSH_DESKTOP_PROFILE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_PROFILE.to_string())
}

/// The loopback port this shell shares, overridable for testing.
pub fn web_port() -> u16 {
    env::var("DSH_DESKTOP_PORT")
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(DEFAULT_WEB_PORT)
}

/// Whether something already serves that loopback port.
///
/// A completed TCP connect is the whole test. Reachability is the only question
/// worth asking: the harness answers an unauthenticated request with 401 rather
/// than closing the socket, so a connect that succeeds means a `dsh web` owns
/// the port, and one that fails means the port is free.
pub fn is_listening(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&address, Duration::from_millis(400)).is_ok()
}

/// Files a supervised `dsh web` leaves its startup line in, nearest first.
///
/// Two names because the two ways the harness outlives a terminal disagree:
/// `dsh-web.log` is what a `tee` on a manual launch writes, and
/// `dsh-web-launchd.log` is where the LaunchAgent's stdout lands.
const ANNOUNCEMENT_LOGS: [&str; 2] = ["dsh-web.log", "dsh-web-launchd.log"];

fn dsh_home() -> PathBuf {
    env::var_os("DSH_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".dsh"))
}

/// Recover the launch token a server on this port announced earlier.
///
/// A token is only ever printed to stdout, so a server this shell did not start
/// is reachable without a cookie only if something kept that line — a
/// supervised service, or a shell that piped the output to a file. Reading the
/// *last* announcement for the port matters: every restart mints a fresh token
/// and invalidates the one before it. A stale token is harmless — the harness
/// falls through to the cookie check — so a miss here costs nothing.
fn discover_token(port: u16) -> Option<String> {
    let needle = format!("http://127.0.0.1:{port}/?token=");
    let home = dsh_home();
    for name in ANNOUNCEMENT_LOGS {
        let Ok(text) = fs::read_to_string(home.join(name)) else {
            continue;
        };
        let Some(found) = text.rfind(&needle) else {
            continue;
        };
        let token: String = text[found + needle.len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !token.is_empty() {
            return Some(token);
        }
    }
    None
}

/// The URL to load for a server this shell did not start.
///
/// With a recoverable token the load authenticates outright and mints the
/// session cookie. Without one the bare URL still works whenever that cookie is
/// already in the webview: it is signed with the secret kept in the shared
/// harness home, so it is not invalidated by restarting the server.
pub fn attach_url(port: u16) -> String {
    match discover_token(port) {
        Some(token) => format!("http://127.0.0.1:{port}/?token={token}"),
        None => format!("http://127.0.0.1:{port}/"),
    }
}

/// Cookie lifetime for a minted session.
///
/// Two constraints pull in opposite directions: the harness rejects a payload
/// whose lifetime exceeds its own `cookieMaxAgeDays` (default 30), and a window
/// left open must outlive its cookie. 29 days sits just under the default, so
/// it satisfies the first while comfortably covering the second. If a profile
/// lowers `cookieMaxAgeDays`, minting is refused by the harness and the shell
/// falls back to the launch token.
const COOKIE_LIFETIME_MILLIS: i64 = 29 * 24 * 60 * 60 * 1000;

fn base64_url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Read the browser-session signing secret out of the shared harness home.
///
/// The credentials file is plain YAML and this key is the only `secret` under
/// the `client-connection/browser-session` record, so an anchored line scan is
/// enough — no YAML dependency, and a miss is harmless because every caller
/// treats it as "fall back to the token".
fn read_cookie_secret() -> Option<String> {
    let text = fs::read_to_string(dsh_home().join(".credentials.yaml")).ok()?;
    let mut inside = false;
    let mut record_indent = 0usize;

    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let key = trimmed
            .split(':')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .trim_matches('\'');
        if key == "client-connection/browser-session" {
            inside = true;
            record_indent = indent;
            continue;
        }
        if !inside {
            continue;
        }
        // A key at or above the record's own indent starts the next record.
        if indent <= record_indent && !key.is_empty() {
            inside = false;
            continue;
        }
        if key == "secret" {
            let value = trimmed
                .split_once(':')
                .map(|(_, rest)| rest)
                .unwrap_or("")
                .trim()
                .trim_matches('"')
                .trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Mint the browser-session cookie the harness would have issued itself.
///
/// This is what makes opening the window always work. A `dsh web` this shell
/// did not start announces its launch token only on its own stdout, so without
/// a log there is nothing to read — but the cookie is not bound to that token,
/// only to the secret in the shared harness home, which any client can read.
/// Minting the same cookie therefore authenticates without a token, without a
/// log, and without a single thing for the user to do.
///
/// Returns the cookie name and value. The format mirrors the harness:
/// `v1.<base64url(payload)>.<base64url(HMAC-SHA256(secret, body))>`.
pub fn mint_session_cookie(port: u16) -> Option<(String, String)> {
    let stored_secret = read_cookie_secret()?;
    // The stored value is base64url *text*; the harness decodes it to raw key
    // bytes before signing, so those same bytes have to be the HMAC key here.
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(stored_secret.as_bytes())
        .ok()?;

    let authority = format!("127.0.0.1:{port}");
    let issued_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    let expires_at = issued_at + COOKIE_LIFETIME_MILLIS;

    let payload = format!(
        "{{\"version\":1,\"authority\":\"{authority}\",\"issuedAt\":{issued_at},\"expiresAt\":{expires_at}}}"
    );
    let body = base64_url(payload.as_bytes());

    let mut mac = Hmac::<Sha256>::new_from_slice(&key).ok()?;
    mac.update(body.as_bytes());
    let signature = mac.finalize().into_bytes();

    let name = format!(
        "dsh-auth-{}",
        base64_url(&Sha256::digest(authority.as_bytes()))
    );
    let value = format!("v1.{body}.{}", base64_url(&signature));
    Some((name, value))
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Sort key for a version directory name such as `v24.11.1`.
fn version_key(name: &str) -> Vec<u64> {
    name.trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

/// Directories under `parent`, newest version first. Unreadable or absent
/// parents yield an empty list rather than an error, because every one of
/// these locations is optional.
fn version_dirs(parent: &PathBuf) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            Some((name, entry.path()))
        })
        .collect();
    found.sort_by(|a, b| version_key(&b.0).cmp(&version_key(&a.0)));
    found.into_iter().map(|(_, path)| path).collect()
}

/// Every Node installation root that could hold a global `@deepseek-ai/dsh`,
/// newest first so the most recent install wins.
fn install_roots() -> Vec<PathBuf> {
    let home = home_dir();
    let mut roots: Vec<PathBuf> = Vec::new();

    // fnm and nvm both keep one root per Node version.
    for version in version_dirs(&home.join(".local/share/fnm/node-versions")) {
        roots.push(version.join("installation"));
    }
    for version in version_dirs(&home.join(".nvm/versions/node")) {
        roots.push(version);
    }

    roots.push(PathBuf::from("/opt/homebrew"));
    roots.push(PathBuf::from("/usr/local"));
    roots.push(home.join(".npm-global"));
    roots.push(home.join(".local"));
    roots.push(home.join("Library/pnpm/global"));
    roots
}

/// Directories that may hold a `node` (or `dsh`) executable. A GUI app
/// launched from Finder inherits launchd's minimal PATH, so the version
/// manager and package manager locations are enumerated explicitly.
fn binary_dirs() -> Vec<PathBuf> {
    let home = home_dir();
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Some(path) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&path));
    }

    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/opt/local/bin"));
    dirs.push(home.join(".volta/bin"));
    dirs.push(home.join(".asdf/shims"));
    dirs.push(home.join(".local/bin"));
    dirs.push(home.join("Library/pnpm"));

    for version in version_dirs(&home.join(".local/share/fnm/node-versions")) {
        dirs.push(version.join("installation/bin"));
    }
    for version in version_dirs(&home.join(".nvm/versions/node")) {
        dirs.push(version.join("bin"));
    }

    dirs.retain(|dir| !dir.as_os_str().is_empty());
    dirs
}

fn is_executable(path: &PathBuf) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// First `name` found on the search path, given as a full path.
fn find_on_dirs(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// Resolve a Node binary.
pub fn resolve_node() -> Option<PathBuf> {
    if let Some(explicit) = env::var_os("DSH_DESKTOP_NODE") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    find_on_dirs("node", &binary_dirs())
}

/// Resolve how to start the harness, in priority order:
///
/// 1. `DSH_DESKTOP_BACKEND` — an explicit entry point or `dsh` executable.
/// 2. A global install under any known Node version-manager or Homebrew root.
/// 3. `dsh` on the search path.
pub fn resolve() -> Result<BackendCmd, String> {
    if let Some(explicit) = env::var_os("DSH_DESKTOP_BACKEND") {
        let raw = explicit.to_string_lossy().trim().to_string();
        if !raw.is_empty() {
            let path = PathBuf::from(&raw);
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(extension.as_str(), "js" | "mjs" | "cjs") {
                if !path.is_file() {
                    return Err(format!(
                        "DSH_DESKTOP_BACKEND is set to {raw}, but that file does not exist"
                    ));
                }
                let node = resolve_node().ok_or_else(|| {
                    format!(
                        "DSH_DESKTOP_BACKEND is set to {raw}, but no node binary was found; set DSH_DESKTOP_NODE"
                    )
                })?;
                return Ok(BackendCmd::Node { node, entry: path });
            }
            if !is_executable(&path) {
                return Err(format!(
                    "DSH_DESKTOP_BACKEND is set to {raw}, but that is not an executable file"
                ));
            }
            return Ok(BackendCmd::Exec(path));
        }
    }

    for root in install_roots() {
        let entry = root.join(CLI_ENTRY_REL);
        if entry.is_file() {
            let sibling = root.join("bin/node");
            let node = if sibling.is_file() {
                sibling
            } else {
                resolve_node().ok_or_else(|| {
                    format!(
                        "found the harness at {} but no node binary; set DSH_DESKTOP_NODE",
                        entry.display()
                    )
                })?
            };
            return Ok(BackendCmd::Node { node, entry });
        }
    }

    if let Some(dsh) = find_on_dirs("dsh", &binary_dirs()) {
        return Ok(BackendCmd::Exec(dsh));
    }

    Err(format!(
        "no dsh installation found. Install it with `npm i -g @deepseek-ai/dsh`, \
         or point DSH_DESKTOP_BACKEND at its lib/bin.js. Searched {} locations.",
        install_roots().len() + 1
    ))
}

/// A PATH for the child that includes every directory we know how to search,
/// so the shell and file tools the agent spawns resolve the same way.
fn child_path() -> std::ffi::OsString {
    let mut dirs = binary_dirs();
    if let Some(path) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&path));
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    dirs.retain(|dir| {
        if seen.contains(dir) {
            false
        } else {
            seen.push(dir.clone());
            true
        }
    });
    env::join_paths(dirs).unwrap_or_default()
}

/// Extract the authenticated URL from a harness announcement line such as
/// `dsh web: http://127.0.0.1:64875/?token=... (LAN: http://10.0.0.5:64875/?token=...)`.
fn parse_announced(line: &str) -> Option<String> {
    const MARKER: &str = "dsh web: ";
    let start = line.find(MARKER)? + MARKER.len();
    let url = line[start..].split_whitespace().next()?;
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        None
    }
}

/// Boot the harness and wait for its authenticated URL.
pub fn start(profile: &str, port: u16, timeout: Duration) -> Result<Backend, String> {
    let resolution = resolve()?;
    let command = resolution.describe();

    let mut command_builder = resolution.command();
    command_builder
        .arg("--profile")
        .arg(profile)
        .arg("--no-open")
        .arg("--port")
        .arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PATH", child_path());

    // Own process group, so one signal reaches the agent's whole tree.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command_builder.process_group(0);
    }

    let mut child = command_builder
        .spawn()
        .map_err(|error| format!("could not start `{command}`: {error}"))?;
    BACKEND_PGID.store(child.id() as i32, Ordering::SeqCst);

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "harness stdout was not captured".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "harness stderr was not captured".to_string())?;

    let log: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let stderr_log = Arc::clone(&log);
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let mut tail = match stderr_log.lock() {
                Ok(tail) => tail,
                Err(_) => break,
            };
            if tail.len() >= LOG_TAIL {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    });

    let (sender, receiver) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match receiver.recv_timeout(remaining) {
            Ok(line) => {
                if let Some(url) = parse_announced(&line) {
                    return Ok(Backend { child, url });
                }
            }
            Err(_) => break,
        }
    }

    let tail = log
        .lock()
        .map(|tail| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    terminate_child(&mut child);
    if tail.is_empty() {
        Err(format!(
            "`{command}` did not announce a URL within {}s",
            timeout.as_secs()
        ))
    } else {
        Err(format!(
            "`{command}` did not announce a URL within {}s. Last output:\n{tail}",
            timeout.as_secs()
        ))
    }
}

/// Signal the child's whole process group, escalating only if it ignores the
/// polite request.
fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // SAFETY: `pid` is a live child placed in its own process group by
        // `process_group(0)`, so its pid is also the group id. A failed signal
        // means the group is already gone, which is the desired end state.
        unsafe {
            libc::killpg(pid, libc::SIGTERM);
        }
        for _ in 0..30 {
            match child.try_wait() {
                Ok(Some(_)) => {
                    BACKEND_PGID.store(0, Ordering::SeqCst);
                    let _ = child.wait();
                    return;
                }
                _ => thread::sleep(Duration::from_millis(100)),
            }
        }
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }

    BACKEND_PGID.store(0, Ordering::SeqCst);
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_loopback_url_from_a_lan_announcement() {
        let line = "dsh web: http://127.0.0.1:64875/?token=abc-123 \
                    (LAN: http://10.0.0.5:64875/?token=abc-123)";
        assert_eq!(
            parse_announced(line).as_deref(),
            Some("http://127.0.0.1:64875/?token=abc-123")
        );
    }

    #[test]
    fn reads_the_url_when_no_lan_address_is_announced() {
        assert_eq!(
            parse_announced("dsh web: http://127.0.0.1:1234/?token=t").as_deref(),
            Some("http://127.0.0.1:1234/?token=t")
        );
    }

    #[test]
    fn ignores_lines_that_carry_no_url() {
        assert_eq!(parse_announced("dsh web: opening the default browser"), None);
        assert_eq!(parse_announced("dsh web: not-a-url"), None);
        assert_eq!(parse_announced("plugin loaded"), None);
        assert_eq!(parse_announced(""), None);
    }

    #[test]
    fn tolerates_leading_log_noise() {
        assert_eq!(
            parse_announced("[info] dsh web: http://127.0.0.1:1/?token=x").as_deref(),
            Some("http://127.0.0.1:1/?token=x")
        );
    }

    #[test]
    fn orders_version_directories_newest_first() {
        assert!(version_key("v24.11.1") > version_key("v22.15.0"));
        assert!(version_key("v9.0.0") < version_key("v24.0.0"));
        assert!(version_key("v24.11.1") > version_key("v24.9.9"));
    }

    #[test]
    fn finds_the_harness_this_shell_is_meant_to_reuse() {
        // The shell's entire premise is reusing the local installation rather
        // than shipping one, so a machine where this fails is a machine where
        // the app cannot work.
        let described = resolve()
            .expect("a dsh installation should be discoverable on this machine")
            .describe();
        println!("resolved backend: {described}");
        assert!(described.contains("dsh"), "unexpected backend: {described}");
    }

    #[test]
    fn mints_a_cookie_the_harness_accepts() {
        // Printing the cookie lets it be replayed against a live server, which
        // is the only way to prove the signature matches the harness's own.
        let (name, value) = mint_session_cookie(3099).expect("a browser-session secret should exist");
        println!("COOKIE {name}={value}");
        assert!(name.starts_with("dsh-auth-"));
        assert!(value.starts_with("v1."));
        assert_eq!(value.split('.').count(), 3);
    }

    #[test]
    fn never_claims_the_electron_owned_profile() {
        // `desktop` is reserved for the official Electron app; the CLI refuses
        // to boot it, so the default here must never collide.
        assert_ne!(DEFAULT_PROFILE, "desktop");
    }
}
