/* openodd coverage view: dispatches `openodd:drill` from host-driven cells,
   shows the explorer's panels, and draws the cells' tooltip cards.
   Idempotent; no dependencies. */
(function () {
  if (window.__openoddCoverageView) return;
  window.__openoddCoverageView = true;

  function show(root, id) {
    var target = document.getElementById(id);
    if (!target || target.closest(".oo-cov") !== root) return; // no such panel: stay
    var panels = root.querySelectorAll(":scope > .oo-panel");
    for (var i = 0; i < panels.length; i++) panels[i].hidden = panels[i] !== target;
    // Keep the keyboard where the reader is: on the panel just shown.
    target.focus({ preventScroll: true });
    if (target.scrollIntoView) target.scrollIntoView({ block: "nearest" });
  }

  document.addEventListener("click", function (event) {
    var el = event.target.closest && event.target.closest("[data-oo-open], [data-oo-drill]");
    if (!el) return;
    var root = el.closest(".oo-cov");
    if (!root) return;
    hideTip();
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

  // Tooltip cards, from each cell's data-oo-tip.
  var tip = null, current = null;
  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  function hideTip() {
    current = null;
    if (tip) tip.classList.remove("oo-show");
  }
  function showTip(cell, x, y) {
    var data;
    try { data = JSON.parse(cell.getAttribute("data-oo-tip")); } catch (e) { return; }
    if (!tip) {
      tip = el("div", "oo-tip");
      tip.setAttribute("role", "tooltip");
      document.body.appendChild(tip);
    }
    var root = cell.closest(".oo-cov");
    var theme = root && root.getAttribute("data-oo-theme");
    if (theme) tip.setAttribute("data-oo-theme", theme); else tip.removeAttribute("data-oo-theme");
    if (current !== cell) {
      current = cell;
      tip.textContent = "";
      tip.appendChild(el("div", "oo-tip-place", data.place));
      var state = el("div", "oo-tip-state");
      state.appendChild(el("span", "oo-tip-dot oo-" + data.kind));
      state.appendChild(el("span", null, data.state));
      tip.appendChild(state);
      var dl = el("dl");
      (data.rows || []).forEach(function (row) {
        dl.appendChild(el("dt", null, row[0]));
        dl.appendChild(el("dd", null, row[1]));
      });
      tip.appendChild(dl);
      if (data.tentative) tip.appendChild(el("div", "oo-tip-note", data.tentative));
    }
    var pad = 14, w = tip.offsetWidth, h = tip.offsetHeight;
    var left = x + pad, top = y + pad;
    if (left + w > window.innerWidth - 8) left = x - w - pad;
    if (top + h > window.innerHeight - 8) top = y - h - pad;
    tip.style.left = Math.max(8, left) + "px";
    tip.style.top = Math.max(8, top) + "px";
    tip.classList.add("oo-show");
  }
  document.addEventListener("mousemove", function (event) {
    var cell = event.target.closest && event.target.closest(".oo-cov [data-oo-tip]");
    if (cell) showTip(cell, event.clientX, event.clientY); else if (current) hideTip();
  });
  document.addEventListener("focusin", function (event) {
    var cell = event.target.closest && event.target.closest(".oo-cov [data-oo-tip]");
    if (!cell) return hideTip();
    var r = cell.getBoundingClientRect();
    showTip(cell, r.right, r.bottom);
  });
  document.addEventListener("focusout", hideTip);
  window.addEventListener("scroll", hideTip, true);
})();
