#!/usr/bin/env bash
# Sync the VERSION file into Cargo.toml, tauri.conf.json, and package.json.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"

if [[ -z "$VERSION" ]]; then
  echo "ERROR: VERSION file is empty" >&2
  exit 1
fi

echo "Syncing version: $VERSION"

# Cargo workspace version
sed -i '' "s/^version = \".*\"/version = \"$VERSION\"/" "$ROOT/Cargo.toml"

# tauri.conf.json
sed -i '' "s/\"version\": \".*\"/\"version\": \"$VERSION\"/" "$ROOT/crates/comrade-app/tauri.conf.json"

# Frontend package.json
sed -i '' "s/\"version\": \".*\"/\"version\": \"$VERSION\"/" "$ROOT/crates/comrade-app/ui/package.json"

echo "Done."
