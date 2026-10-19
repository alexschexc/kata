#!/usr/bin/env bash
# Cross-build the Rust port for all release targets from Linux with
# cargo-zigbuild (Zig 0.16.0 is the linker only), then verify the musl
# binary is statically linked. Output: rust/target/<triple>/release/.
set -euo pipefail
cd "$(dirname "$0")/../rust"

targets=(x86_64-unknown-linux-musl aarch64-apple-darwin x86_64-pc-windows-gnu)
for t in "${targets[@]}"; do
  echo "== $t"
  cargo zigbuild --release --locked --target "$t"
done

musl=target/x86_64-unknown-linux-musl/release/kata
file "$musl" | grep -q "statically linked" || { echo "FAIL: $musl is not static" >&2; exit 1; }
if readelf -d "$musl" 2>/dev/null | grep -q NEEDED; then
  echo "FAIL: $musl has dynamic NEEDED entries" >&2; exit 1
fi
if readelf -l "$musl" 2>/dev/null | grep -q INTERP; then
  echo "FAIL: $musl requests an ELF interpreter" >&2; exit 1
fi
echo "OK: $musl is static (no NEEDED, no INTERP)"
ls -l target/x86_64-unknown-linux-musl/release/kata \
      target/aarch64-apple-darwin/release/kata \
      target/x86_64-pc-windows-gnu/release/kata.exe
