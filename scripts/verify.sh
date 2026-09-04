#!/usr/bin/env bash
# Project verify gate — run before declaring work done. fmt, clippy, tests and a
# debug compile of the real app. Not packaging: installers, signing and updater
# artifacts belong to the tag-driven release workflow.
set -euo pipefail
cd "$(dirname "$0")/.."

npm run build
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets

# The three files that carry the version must agree, or the installer name,
# the About box and the updater manifest would disagree about what shipped.
cargo_version=$(sed -n '0,/^version = /s/^version = "\(.*\)"/\1/p' src-tauri/Cargo.toml)
tauri_version=$(sed -n '0,/"version":/s/.*"version": "\(.*\)".*/\1/p' src-tauri/tauri.conf.json)
npm_version=$(sed -n '0,/"version":/s/.*"version": "\(.*\)".*/\1/p' package.json)
# Read before compared: an empty match from a moved key would make all three
# "equal" and the check silently vacuous.
for pair in "src-tauri/Cargo.toml:$cargo_version" "tauri.conf.json:$tauri_version" "package.json:$npm_version"; do
  if [ -z "${pair#*:}" ]; then
    echo "version check is broken: read nothing from ${pair%%:*}"
    exit 1
  fi
done
if [ "$cargo_version" != "$tauri_version" ] || [ "$cargo_version" != "$npm_version" ]; then
  echo "version mismatch: Cargo.toml $cargo_version, tauri.conf.json $tauri_version, package.json $npm_version"
  exit 1
fi
echo "version agreement OK ($cargo_version)"
