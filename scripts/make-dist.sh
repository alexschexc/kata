#!/usr/bin/env bash
# Assemble dist/ for the current commit: the Linux static binary built here, and the macOS and
# Windows binaries downloaded from the native GitHub Actions runs for the same commit.
# Usage: scripts/make-dist.sh   (needs mise, cargo-zigbuild, gh; run on a pushed, clean commit)
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
version="$(sed -nE 's/^version = "([^"]+)"/\1/p' rust/Cargo.toml | head -n1)"
sha="$(git rev-parse HEAD)"
repo=alexschexc/kata
rm -rf dist && mkdir -p dist
scratch="$(mktemp -d "${TMPDIR:-/tmp}/kata-dist.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

# Linux x86-64: static musl.
(cd rust && mise exec -- cargo zigbuild --release --locked --target x86_64-unknown-linux-musl --target-dir "$scratch/target")
install -m 755 "$scratch/target/x86_64-unknown-linux-musl/release/kata" "dist/kata-v$version-linux-x86_64"

# macOS arm64 and Windows x64: the binaries the native runners smoke-tested for this commit.
fetch() {  # workflow artifact
  local run
  run="$(gh run list --repo "$repo" --workflow "$1" --commit "$sha" --status success --limit 1 --json databaseId -q '.[0].databaseId')"
  [ -n "$run" ] || { echo "no successful $1 run for $sha; push and wait for CI" >&2; exit 1; }
  gh run download "$run" --repo "$repo" --name "$2" --dir "$scratch/$2"
}
fetch macos-smoke kata-macos-arm64
fetch windows-smoke kata-windows-x64
install -m 755 "$scratch/kata-macos-arm64/kata" "dist/kata-v$version-macos-aarch64"
install -m 755 "$scratch/kata-windows-x64/kata.exe" "dist/kata-v$version-windows-x86_64.exe"

# macOS release archive: binary, Finder launcher, README, LICENSE.
name="kata-v$version-macos-aarch64"
stage="$scratch/$name"
mkdir -p "$stage"
install -m 755 "dist/$name" "$stage/kata"
cp README.md LICENSE "$stage/"
cat > "$stage/Run Kata.command" <<'LAUNCH'
#!/bin/sh
# Double-click in Finder to run Kata in Terminal.
cd "$(dirname "$0")" || exit 1
xattr -d com.apple.quarantine ./kata 2>/dev/null
chmod +x ./kata
exec ./kata
LAUNCH
chmod 755 "$stage/Run Kata.command"
tar -C "$scratch" -czf "dist/$name.tar.gz" "$name"

(cd dist && sha256sum -- * > SHA256SUMS)
ls -l dist
