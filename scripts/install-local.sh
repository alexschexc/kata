#!/usr/bin/env bash
# Build the standalone static Rust release binary (linux-musl) and install it to
# ~/.local/bin/kata, which the app-launcher entry (kata.desktop) runs.
# Zig 0.16.0 (mise.toml) is used as the linker only, via cargo-zigbuild.
# Usage: scripts/install-local.sh [destination]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="${1:-$HOME/.local/bin/kata}"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/kata-install.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

cd "$root/rust"
target=x86_64-unknown-linux-musl
mise exec -- cargo zigbuild --release --locked --target "$target" --target-dir "$scratch/target"
built="$scratch/target/$target/release/kata"

# Static check, then a smoke run with isolated state before replacing the installed copy.
file "$built" | grep -q "statically linked"
if readelf -d "$built" 2>/dev/null | grep -q NEEDED; then echo "FAIL: dynamic NEEDED entries" >&2; exit 1; fi
"$built" --state "$scratch/state.json" --passage 'John:1:1' --dump | grep -q 'In the beginning was the Word'

mkdir -p "$(dirname "$dest")"
install -m 755 "$built" "$dest.new"
mv -f "$dest.new" "$dest"   # atomic replace; a running kata keeps its old inode
echo "installed $dest ($(stat -c %s "$dest") bytes, sha256 $(sha256sum "$dest" | cut -c1-16))"
