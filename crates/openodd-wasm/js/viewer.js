// openodd's views in the browser: mount a coverage matrix or a case plot on
// an element and re-render it, client side, when its axis pickers or cells
// are used. Works with the package wasm-pack builds from crates/openodd-wasm.
//
//   import init, { Viewer, styles, script } from "./openodd_wasm.js";
//   import { install, mount } from "./viewer.js";
//   await init();
//   install({ styles, script });                 // once per page
//   const viewer = new Viewer(JSON.stringify(documents));
//   const plot = mount(viewer, el, { view: "cases", x: "speed", y: "gap" });
//   plot.set({ y: "rain" });                     // or let the pickers do it
//   mount(viewer, el2, { view: "drift" });       // the scenario's realisation
//
// No dependencies; every number is computed in WebAssembly.

/** Put the components' CSS and script on the page, once. */
export function install({ styles, script }) {
  if (document.getElementById("openodd-styles")) return;
  const style = document.createElement("style");
  style.id = "openodd-styles";
  style.textContent = styles();
  document.head.appendChild(style);
  const js = document.createElement("script");
  js.id = "openodd-script";
  js.textContent = script();
  document.head.appendChild(js);
}

/**
 * Render a view of `viewer` in `el`, and keep it live.
 *
 * `options`: `view` ("cases" or "coverage"), the query (`x`, `y`,
 * `conditions`, `scenario`, `environment`, ...) and the component's options
 * (`id`, `labels`, `table`, `prediction`, `instability_rings`). The view's
 * own `openodd:axes` (and, for the coverage matrix, `openodd:drill`) events
 * are handled here: the chosen pair, or the cell clicked into, is rendered
 * at once. Each render dispatches `openodd:rendered` on `el`, with the
 * state and the time it took (`detail.ms`).
 */
export function mount(viewer, el, options = {}) {
  const view = ["coverage", "drift"].includes(options.view) ? options.view : "cases";
  const queryKeys = ["x", "y", "conditions", "scenario", "environment", "min_runs", "band", "min_seconds",
    "basis", "measured_at"];
  const optionKeys = ["id", "labels", "table", "prediction", "instability_rings", "drift_leaders", "knobs"];
  const state = {};
  for (const k of queryKeys) if (options[k] !== undefined) state[k] = options[k];
  const opts = {};
  for (const k of optionKeys) if (options[k] !== undefined) opts[k] = options[k];
  if (!opts.id) opts.id = el.id ? el.id + "-view" : "oo-" + view + "-" + Math.random().toString(36).slice(2, 8);

  function render() {
    const t0 = performance.now();
    const query = JSON.stringify(state);
    const o = JSON.stringify(opts);
    el.innerHTML = view === "cases" ? viewer.cases_html(query, o)
      : view === "drift" ? viewer.drift_html(query, o)
      : viewer.coverage_html(query, o);
    const ms = performance.now() - t0;
    el.dispatchEvent(new CustomEvent("openodd:rendered", { detail: { view, state: { ...state }, ms } }));
    return ms;
  }

  function onAxes(event) {
    const d = event.detail || {};
    const own = (d.view || "coverage") === view;
    if (!own || !el.contains(event.target)) return;
    event.preventDefault();               // render here instead of following a link
    state.x = d.x;
    state.y = d.y;
    if (d.basis) state.basis = d.basis;  // the case plot's drawn / measured toggle
    if (view === "coverage") state.conditions = d.conditions || [];
    render();
  }
  function onDrill(event) {
    if (view !== "coverage" || !el.contains(event.target)) return;
    event.preventDefault();
    state.conditions = (event.detail && event.detail.conditions) || [];
    delete state.x;                       // the next level lays out its default axes
    delete state.y;
    render();
  }
  el.addEventListener("openodd:axes", onAxes);
  el.addEventListener("openodd:drill", onDrill);
  render();
  return {
    state,
    render,
    /** Change the query (e.g. `{x, y}`) and render. */
    set(changes) { Object.assign(state, changes); return render(); },
    /** Stop listening; the HTML stays. */
    unmount() {
      el.removeEventListener("openodd:axes", onAxes);
      el.removeEventListener("openodd:drill", onDrill);
    },
  };
}
