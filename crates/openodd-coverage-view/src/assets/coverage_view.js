/* openodd coverage view: dispatches `openodd:drill` from host-driven cells,
   and shows the explorer's panels. Idempotent; no dependencies. */
(function () {
  if (window.__openoddCoverageView) return;
  window.__openoddCoverageView = true;
  function show(root, id) {
    var panels = root.querySelectorAll(".oo-panel");
    for (var i = 0; i < panels.length; i++) panels[i].hidden = panels[i].id !== id;
    var here = root.querySelector("#" + CSS.escape(id));
    if (here && here.scrollIntoView) here.scrollIntoView({ block: "nearest" });
  }
  document.addEventListener("click", function (event) {
    var el = event.target.closest && event.target.closest("[data-oo-open], [data-oo-drill]");
    if (!el) return;
    var root = el.closest(".oo-cov");
    if (!root) return;
    if (el.hasAttribute("data-oo-open")) {
      event.preventDefault();
      show(root, el.getAttribute("data-oo-open"));
      return;
    }
    var conditions = JSON.parse(el.getAttribute("data-oo-drill"));
    var drill = new CustomEvent("openodd:drill", {
      bubbles: true, cancelable: true, detail: { conditions: conditions, root: root.id },
    });
    // A host that handles the drill itself cancels the event; the link (if
    // any) is then not followed.
    if (!el.dispatchEvent(drill)) event.preventDefault();
  });
})();
