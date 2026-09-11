//! DSH Desktop — a thin Tauri shell around the user's own harness install.
//!
//! The shell renders nothing of its own beyond a boot screen. It resolves the
//! user's `dsh`, boots one private profile on a loopback port, and hands the
//! authenticated URL to the system webview, so the interface, the sessions and
//! the configuration are exactly the ones the CLI already uses.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::webview::cookie::{Cookie, SameSite};
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// The boot screen, shown while the harness starts.
const SPLASH_LABEL: &str = "splash";
/// The window that loads the harness interface itself.
const MAIN_LABEL: &str = "main";
/// Static page backing the boot screen.
const SPLASH_PAGE: &str = "index.html";

/// Set while a restart is swapping the main window, so the deliberate teardown
/// of the old window is not mistaken for the user quitting.
static RESTARTING: AtomicBool = AtomicBool::new(false);

/// Shell state: the single harness server this window owns.
#[derive(Default)]
struct Shell {
    backend: Mutex<Option<backend::Backend>>,
}

impl Shell {
    /// Lock the backend slot, recovering from a poisoned mutex instead of
    /// panicking: one failed boot must never wedge the whole shell.
    fn backend_slot(&self) -> std::sync::MutexGuard<'_, Option<backend::Backend>> {
        match self.backend.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

fn main() {
    // Installed before the first harness process exists, so no signal can slip
    // past and orphan it.
    backend::install_signal_handlers();

    let app = tauri::Builder::default()
        .manage(Shell::default())
        .setup(|app| {
            let handle = app.handle().clone();

            build_splash(&handle)?;
            // Without an Edit menu the standard clipboard accelerators never
            // reach the webview on macOS.
            let reload = MenuItemBuilder::with_id("reload", "Reload Interface")
                .accelerator("CmdOrCtrl+R")
                .build(app)?;
            let restart = MenuItemBuilder::with_id("restart", "Restart Harness")
                .accelerator("CmdOrCtrl+Shift+R")
                .build(app)?;

            let app_menu = SubmenuBuilder::new(app, "DSH Desktop")
                .about(None)
                .separator()
                .hide()
                .hide_others()
                .show_all()
                .separator()
                .quit()
                .build()?;
            let edit_menu = SubmenuBuilder::new(app, "Edit")
                .undo()
                .redo()
                .separator()
                .cut()
                .copy()
                .paste()
                .select_all()
                .build()?;
            let view_menu = SubmenuBuilder::new(app, "View")
                .item(&reload)
                .item(&restart)
                .build()?;
            let menu = MenuBuilder::new(app)
                .items(&[&app_menu, &edit_menu, &view_menu])
                .build()?;
            app.set_menu(menu)?;

            std::thread::spawn(move || boot(handle));
            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "reload" => {
                if let Some(window) = app.get_webview_window(MAIN_LABEL) {
                    let _ = window.eval("window.location.reload()");
                }
            }
            "restart" => restart(app.clone()),
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("failed to build DSH Desktop");

    app.run(|handle, event| {
        if let RunEvent::ExitRequested { .. } = event {
            if let Some(state) = handle.try_state::<Shell>() {
                let mut guard = state.backend_slot();
                if let Some(server) = guard.as_mut() {
                    server.terminate();
                }
            }
        }
    });
}

/// Show the boot screen, if it is not already up.
fn build_splash(app: &tauri::AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(SPLASH_LABEL).is_some() {
        return Ok(());
    }
    let splash = WebviewWindowBuilder::new(
        app,
        SPLASH_LABEL,
        WebviewUrl::App(SPLASH_PAGE.into()),
    )
    .title("DSH Desktop")
    .inner_size(560.0, 360.0)
    .resizable(false)
    .center()
    .build()?;

    let handle = app.clone();
    splash.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { .. } = event {
            quit_unless_restarting(&handle);
        }
    });
    Ok(())
}

/// Exit the application, unless the window is going away as part of a restart.
fn quit_unless_restarting(app: &tauri::AppHandle) {
    if !RESTARTING.load(Ordering::SeqCst) {
        app.exit(0);
    }
}

/// Open the interface.
///
/// Attaches to whatever already serves the shared loopback port, and boots one
/// there only when the port is free. Attaching is the point: the browser and
/// the window then share one server, one event stream and one session cookie,
/// so a session or a message on either side is live on the other. A `dsh web`
/// the user started themselves is therefore never disturbed — the window simply
/// becomes its second view.
///
/// Attaching is also unattended. A server this shell did not start announces
/// its token only on its own stdout, so the window mints the session cookie the
/// harness would have issued itself, from the secret both surfaces already
/// share. There is no token to find and nothing for the user to paste.
fn boot(app: tauri::AppHandle) {
    let port = backend::web_port();

    let (url, cookie) = if backend::is_listening(port) {
        match backend::mint_session_cookie(port) {
            Some(cookie) => (format!("http://127.0.0.1:{port}/"), Some(cookie)),
            // No readable secret: fall back to a logged launch token, and then
            // to the cookie already in the webview.
            None => (backend::attach_url(port), None),
        }
    } else {
        let profile = backend::profile_name();
        match backend::start(&profile, port, backend::BOOT_TIMEOUT) {
            Ok(server) => {
                let url = server.url.clone();
                let state = app.state::<Shell>();
                let mut guard = state.backend_slot();
                if let Some(mut previous) = guard.take() {
                    previous.terminate();
                }
                *guard = Some(server);
                (url, None)
            }
            Err(error) => {
                report(&app, &error);
                RESTARTING.store(false, Ordering::SeqCst);
                return;
            }
        }
    };

    match tauri::Url::parse(&url) {
        Ok(parsed) => {
            if let Err(error) = open_main(&app, parsed, cookie) {
                report(&app, &format!("could not open {url}: {error}"));
            }
        }
        Err(error) => report(
            &app,
            &format!("the harness announced an unusable URL {url}: {error}"),
        ),
    }

    RESTARTING.store(false, Ordering::SeqCst);
    if app.get_webview_window(MAIN_LABEL).is_none() {
        // Nothing to show and nothing to retry into: do not linger headless.
        if app.get_webview_window(SPLASH_LABEL).is_none() {
            app.exit(0);
        }
    }
}

/// Open the harness interface in its own window.
///
/// The window is *created* at the URL rather than navigated to it: a fresh
/// `WebviewUrl::External` load is the supported path. When this shell booted the
/// server the URL carries the launch token and the request authenticates on its
/// own; when it attached, `cookie` holds a freshly minted session cookie that
/// has to be installed before the request is re-issued.
fn open_main(
    app: &tauri::AppHandle,
    url: tauri::Url,
    cookie: Option<(String, String)>,
) -> tauri::Result<()> {
    if let Some(existing) = app.get_webview_window(MAIN_LABEL) {
        let _ = existing.destroy();
    }

    let window = WebviewWindowBuilder::new(app, MAIN_LABEL, WebviewUrl::External(url))
        .title("DSH Desktop")
        .inner_size(1280.0, 840.0)
        .min_inner_size(760.0, 520.0)
        .center()
        .build()?;

    if let Some((name, value)) = cookie {
        let mut jar = Cookie::new(name, value);
        // wry hands `domain` straight to NSHTTPCookieDomain, so it has to be
        // spelled out; the harness itself sets a host-only cookie.
        jar.set_domain("127.0.0.1");
        jar.set_path("/");
        jar.set_http_only(true);
        jar.set_same_site(SameSite::Strict);
        if window.set_cookie(jar).is_ok() {
            // The window is already loading `/` and may have been refused, so
            // re-issue the request now that the cookie is in the store.
            let _ = window.eval("window.location.reload()");
        }
    }

    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { .. } = event {
            quit_unless_restarting(&handle);
        }
    });

    if let Some(splash) = app.get_webview_window(SPLASH_LABEL) {
        let _ = splash.destroy();
    }
    Ok(())
}

/// Show a failure on the boot screen, falling back to stderr when it is gone.
fn report(app: &tauri::AppHandle, message: &str) {
    let Some(window) = app.get_webview_window(SPLASH_LABEL) else {
        eprintln!("dsh-desktop: {message}");
        return;
    };
    let payload =
        serde_json::to_string(message).unwrap_or_else(|_| "\"unprintable error\"".to_string());
    let _ = window.eval(format!(
        "window.__dshFailed && window.__dshFailed({payload})"
    ));
}

/// Stop the current server and boot a fresh one — the way to pick up plugin or
/// harness code changes without quitting the app.
fn restart(app: tauri::AppHandle) {
    RESTARTING.store(true, Ordering::SeqCst);

    {
        let state = app.state::<Shell>();
        let mut guard = state.backend_slot();
        if let Some(mut server) = guard.take() {
            server.terminate();
        }
    }
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        let _ = window.destroy();
    }
    let _ = build_splash(&app);
    std::thread::spawn(move || boot(app));
}
