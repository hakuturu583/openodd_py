/* openodd fuzzy rules view: dispatches `openodd:rules` from its sliders
   (`detail.inputs`, on release) and pickers (`detail.quantity`, `detail.x`),
   and shows a slider's value while it moves. A host -- js/viewer.js's
   `mount` -- answers by rendering again. Idempotent; no dependencies. */
(function () {
  if (window.__openoddRulesView) return;
  window.__openoddRulesView = true;

  function root(el) { return el.closest && el.closest(".oo-cov.oo-rules"); }

  document.addEventListener("input", function (event) {
    var el = event.target;
    if (!el.matches || !el.matches("input[data-oo-input]") || !root(el)) return;
    var out = el.parentNode.querySelector("output");
    if (out) out.textContent = String(Math.round(Number(el.value) * 1000) / 1000);
  });

  document.addEventListener("change", function (event) {
    var el = event.target;
    var r = el.matches && root(el);
    if (!r) return;
    var detail = null;
    if (el.matches("input[data-oo-input]")) {
      var inputs = {};
      var all = r.querySelectorAll("input[data-oo-input]");
      for (var i = 0; i < all.length; i++) inputs[all[i].getAttribute("data-oo-input")] = Number(all[i].value);
      detail = { inputs: inputs, focus: "input:" + el.getAttribute("data-oo-input") };
    } else if (el.matches("select[data-oo-rules]")) {
      var what = el.getAttribute("data-oo-rules");
      detail = { focus: "select:" + what };
      detail[what] = el.value;
    }
    if (!detail) return;
    detail.root = r.id;
    el.dispatchEvent(new CustomEvent("openodd:rules", { bubbles: true, cancelable: true, detail: detail }));
  });
})();
