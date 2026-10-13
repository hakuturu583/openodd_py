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

  // Axis pickers.
  var SEP = "\u001f";
  function picks(bar) {
    return {
      x: bar.querySelector('select[data-oo-axis="x"]'),
      y: bar.querySelector('select[data-oo-axis="y"]'),
    };
  }
  function reset(bar) {
    var s = picks(bar);
    s.x.value = s.x.getAttribute("data-oo-current");
    s.y.value = s.y.getAttribute("data-oo-current");
  }
  function apply(bar) {
    var root = bar.closest(".oo-cov");
    var s = picks(bar), x = s.x.value, y = s.y.value;
    var map = bar.getAttribute("data-oo-map");
    if (map) {
      var id = JSON.parse(map)[x + SEP + y];
      reset(bar); // this panel keeps showing its own pair
      if (id) show(root, id);
      return;
    }
    var conditions = JSON.parse(bar.getAttribute("data-oo-conditions"));
    var axes = new CustomEvent("openodd:axes", {
      bubbles: true, cancelable: true,
      detail: { conditions: conditions, x: x, y: y, root: root.id },
    });
    if (!bar.dispatchEvent(axes)) return;
    var link = bar.getAttribute("data-oo-link");
    if (link) {
      var q = [bar.getAttribute("data-oo-at"), "x=" + encodeURIComponent(x), "y=" + encodeURIComponent(y)];
      window.location.href = link.replace("{conditions}", q.filter(Boolean).join("&"));
    }
  }
  document.addEventListener("change", function (event) {
    var sel = event.target;
    if (!sel.matches || !sel.matches(".oo-axes select")) return;
    var bar = sel.closest(".oo-axes"), s = picks(bar);
    var was = { x: s.x.getAttribute("data-oo-current"), y: s.y.getAttribute("data-oo-current") };
    // Picking the other axis's attribute swaps them.
    if (s.x.value === s.y.value) {
      if (sel === s.x) s.y.value = was.x;
      else s.x.value = was.y || was.x;
    }
    apply(bar);
  });
  document.addEventListener("click", function (event) {
    var btn = event.target.closest && event.target.closest(".oo-axes [data-oo-swap]");
    if (!btn) return;
    var bar = btn.closest(".oo-axes"), s = picks(bar);
    if (!s.y.value) return;
    var x = s.x.value;
    s.x.value = s.y.value;
    s.y.value = x;
    apply(bar);
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
