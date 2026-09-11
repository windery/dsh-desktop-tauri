// Stamp a release version into the manifests that carry one.
//
// The bundle name comes from tauri.conf.json, never from the git tag. Without
// this, tagging `v0.2.0` would publish a DMG still named `0.1.0` — a mismatch
// nobody notices until they read the file name. Doing it here rather than by
// hand is what makes "push a tag" the entire release procedure.
//
// Kept as a real script rather than an inline heredoc so it can be run, and
// tested, outside the workflow.
//
// usage: node .github/stamp-version.mjs 0.2.0
import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+/.test(version ?? "")) {
  console.error(`stamp-version: '${version}' is not of the form X.Y.Z`);
  process.exit(1);
}

/**
 * Replace the first match of `pattern`, keeping its captured prefix.
 *
 * The callback form is deliberate: `$1` followed by a version that starts with a
 * digit would be read as `$10`, `$12`, ... and silently splice the wrong group.
 *
 * The match is checked separately from the write, because "the replacement
 * changed nothing" is not the same as "there was nothing to replace". Stamping a
 * tag whose version already matches the manifests is a legitimate no-op — and it
 * is the *common* case, since a first release usually tags the version that is
 * already committed. Conflating the two fails precisely there.
 */
function stamp(file, pattern) {
  const before = readFileSync(file, "utf8");
  if (!pattern.test(before)) throw new Error(`no version field found in ${file}`);
  writeFileSync(file, before.replace(pattern, (_match, prefix) => prefix + version));
}

stamp("package.json", /("version":\s*")[^"]+/);
stamp("src-tauri/tauri.conf.json", /("version":\s*")[^"]+/);
// Cargo.toml's package version is the only line that starts with `version =`;
// dependency versions are written inline as tables.
stamp("src-tauri/Cargo.toml", /^(version = ")[^"]+/m);

console.log(`stamped ${version} into 3 manifests`);
