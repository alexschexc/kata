#!/usr/bin/env bash
# Build the standalone release binary and install it to ~/.local/bin/kata,
# which the app-launcher entry (kata.desktop) runs.
# Usage: scripts/install-local.sh [destination]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="${1:-$HOME/.local/bin/kata}"
prefix="$(mktemp -d "${TMPDIR:-/tmp}/kata-install.XXXXXX")"
trap 'rm -rf "$prefix"' EXIT

cd "$root"
# Build into a scratch prefix so the tracked zig-out/ binary is left alone.
mise exec -- zig build -Dtarget=x86_64-linux-musl -Doptimize=ReleaseSmall -Dstrip=true --prefix "$prefix"
built="$prefix/bin/kata"

# Smoke check with isolated state before replacing the installed copy.
state="$prefix/state.json"
"$built" --state "$state" --passage 'John:1:1' --dump | grep -q 'In the beginning was the Word'

mkdir -p "$(dirname "$dest")"
install -m 755 "$built" "$dest.new"
mv -f "$dest.new" "$dest"   # atomic replace; a running kata keeps its old inode
echo "installed $dest ($(stat -c %s "$dest") bytes, sha256 $(sha256sum "$dest" | cut -c1-16))"
