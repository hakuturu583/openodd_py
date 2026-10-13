#!/bin/sh
# Build the WebAssembly package the demo loads (pkg/), then serve this folder:
#   examples/wasm-viewer/build.sh && python3 -m http.server -d examples/wasm-viewer 8000
# (a module page cannot be opened from file://). Needs wasm-pack and the
# wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
wasm-pack build "$here/../../crates/openodd-wasm" --release --target web --out-dir "$here/pkg" --no-pack
cp "$here/../../crates/openodd-wasm/js/viewer.js" "$here/pkg/viewer.js"
ls -l "$here/pkg/openodd_wasm_bg.wasm"
