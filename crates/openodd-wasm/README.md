# openodd-wasm

openodd's coverage view and case plot for the browser (`wasm32-unknown-unknown`,
wasm-bindgen): a `Viewer` reads coverage documents and renders both views
client side, the case plot's predicted background included. `js/viewer.js`
mounts a view on an element and keeps its axis pickers live. See
[docs/wasm.md](../../docs/wasm.md) and the demo in
[examples/wasm-viewer](../../examples/wasm-viewer).

```bash
wasm-pack build crates/openodd-wasm --release --target web
wasm-pack test --node crates/openodd-wasm
```
