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
//   // Two sets on the same ODD, laid over one another: the same mount.
//   const cmp = new Comparison(JSON.stringify(before), JSON.stringify(after), "v1.2", "v1.3");
//   mount(cmp, el3, { view: "cases", background: "delta" });
//   mount(cmp, el4, { view: "coverage" });
//
//   // A backend's fuzzy rules: the same mount. loadRules fetches a rule
//   // file and every file it extends.
//   const rules = await loadRules(RulesView, "https://example.com/rules/mini.yaml");
//   mount(rules, el5, { view: "rules", quantity: "tire_friction", inputs: { water: 70 } });
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
 * Render a view of `viewer` (a `Viewer`, or a `Comparison` of two sets) in
 * `el`, and keep it live.
 *
 * `options`: `view` ("cases" or "coverage"), the query (`x`, `y`,
 * `conditions`, `scenario`, `environment`, ...) and the component's options
 * (`id`, `labels`, `table`, `prediction`, `instability_rings`; for a
 * `Comparison`, `names` and the case plot's `background`). The view's
 * own `openodd:axes` (and, for the coverage matrix, `openodd:drill`) events
 * are handled here: the chosen pair, or the cell clicked into, is rendered
 * at once. Each render dispatches `openodd:rendered` on `el`, with the
 * state and the time it took (`detail.ms`).
 */
export function mount(viewer, el, options = {}) {
  if (options.view === "rules") return mountRules(viewer, el, options);
  const view = ["coverage", "drift"].includes(options.view) ? options.view : "cases";
  const queryKeys = ["x", "y", "conditions", "scenario", "environment", "min_runs", "band", "min_seconds",
    "basis", "measured_at"];
  const optionKeys = ["id", "labels", "table", "prediction", "instability_rings", "drift_leaders", "knobs",
    "names", "background"];
  if (view === "drift" && typeof viewer.drift_html !== "function") {
    throw new Error("openodd: a Comparison has no drift view; mount each set's Viewer for it");
  }
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

/**
 * Render a `RulesView` in `el` and keep it live: its sliders (`inputs`) and
 * pickers (`quantity`, `x`) dispatch `openodd:rules`, handled here. `options`:
 * the query (`quantity`, `inputs`: `{name: value}`, `x`: the input the
 * response chart moves) and the component's options (`id`, `labels`,
 * `samples`). Each render dispatches `openodd:rendered` on `el`, as above.
 */
function mountRules(viewer, el, options) {
  const state = {};
  for (const k of ["quantity", "inputs", "x"]) if (options[k] !== undefined) state[k] = options[k];
  const opts = {};
  for (const k of ["id", "labels", "samples"]) if (options[k] !== undefined) opts[k] = options[k];
  if (!opts.id) opts.id = el.id ? el.id + "-view" : "oo-rules-" + Math.random().toString(36).slice(2, 8);

  function render(focus) {
    const t0 = performance.now();
    el.innerHTML = viewer.rules_html(JSON.stringify(state), JSON.stringify(opts));
    if (focus) {
      // Keep the keyboard on the control just used: the render replaced it.
      const [kind, name] = focus.split(":");
      const sel = kind === "input" ? `input[data-oo-input="${CSS.escape(name)}"]`
        : `select[data-oo-rules="${CSS.escape(name)}"]`;
      const again = el.querySelector(sel);
      if (again) again.focus({ preventScroll: true });
    }
    const ms = performance.now() - t0;
    el.dispatchEvent(new CustomEvent("openodd:rendered", { detail: { view: "rules", state: { ...state }, ms } }));
    return ms;
  }

  function onRules(event) {
    if (!el.contains(event.target)) return;
    event.preventDefault();
    const d = event.detail || {};
    if (d.quantity !== undefined && d.quantity !== state.quantity) {
      state.quantity = d.quantity;
      delete state.x;                     // the next quantity's first input
    }
    if (d.x !== undefined) state.x = d.x;
    if (d.inputs) state.inputs = { ...(state.inputs || {}), ...d.inputs };
    render(d.focus);
  }
  render();                               // throws on a query the rules cannot answer
  el.addEventListener("openodd:rules", onRules);
  return {
    state,
    render,
    /** Change the query (e.g. `{inputs: {water: 80}}`) and render. */
    set(changes) {
      if (changes.inputs) changes = { ...changes, inputs: { ...(state.inputs || {}), ...changes.inputs } };
      Object.assign(state, changes);
      return render();
    },
    /** Stop listening; the HTML stays. */
    unmount() { el.removeEventListener("openodd:rules", onRules); },
  };
}

/**
 * Fetch the rule file at `url` and every file it `extends` (relative paths
 * against the file naming them, `https://` URLs as they are), and read them
 * into a `RulesView` (pass the class: `loadRules(RulesView, url)`). A git
 * source cannot be read in the browser. `text`, if given, is the file's
 * content already in hand (an editor's), read as if fetched from `url`.
 */
export async function loadRules(RulesView, url, { text, fetchText } = {}) {
  const get = fetchText || (async (u) => {
    const response = await fetch(u);
    if (!response.ok) throw new Error(`cannot read rules ${u}: ${response.status} ${response.statusText}`);
    return response.text();
  });
  const source = new URL(url, document.baseURI).href;
  const top = text !== undefined ? text : await get(source);
  const files = {};
  const queue = [[top, source]];
  while (queue.length) {
    const [body, from] = queue.shift();
    for (const ref of JSON.parse(RulesView.references(body, from))) {
      if (ref in files || ref === source) continue;
      files[ref] = await get(ref);
      queue.push([files[ref], ref]);
    }
  }
  return new RulesView(top, source, JSON.stringify(files));
}
