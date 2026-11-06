//! The rules as a factor graph: the variables -- the inputs a backend gives
//! and the quantities the rules derive -- and the factors between them, each
//! a part: a conditional distribution of its quantity given the variables it
//! reads. A part may read another quantity, so the graph may chain.
//!
//! The canvas edits the graph's structure: drag a node to move it, drag from
//! a variable's port onto a factor to make that part read it, or onto a
//! quantity to give the quantity a new part reading it. A link that would
//! make a quantity read itself is refused. Selecting a node opens its form
//! beside the canvas, the rules editor's own: a part's points and
//! distribution, a quantity's parts, an input's default.

use std::collections::HashMap;

use leptos::prelude::*;
use openodd::rules::RuleSet;

use crate::app::unique;
use crate::doc::fresh_id;
use crate::icons::Icon;
use crate::io;
use crate::rules_app::{rctx, ErrorCard, PartView, QuantityView, RSel, RulesPanel, RulesToolbar};
use crate::rules_doc::{
    self, num, short, tidy, FixedDoc, InputDoc, PartDoc, QuantityDoc, RulesDoc,
};

const LAYOUT_KEY: &str = "openodd-editor/graph-layout";

/// A node's place in the document.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKey {
    /// An input the backend gives, by name.
    Input(String),
    /// A quantity, by id.
    Quantity(u64),
    /// A part (a factor): its quantity's id and its own.
    Part(u64, u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Input,
    Quantity,
    Part,
}

/// What the side panel shows.
#[derive(Clone, Debug, PartialEq)]
pub enum GSel {
    Node(NodeKey),
    /// Axis `k` of part `p` of quantity `q`: an edge into a factor.
    Edge(u64, u64, usize),
}

#[derive(Clone, Debug, PartialEq)]
struct GNode {
    key: NodeKey,
    /// Stable across edits and reloads: what the layout is kept by.
    id: String,
    kind: Kind,
    label: String,
    /// A second line: a variable's value at the tested inputs, a factor's
    /// distribution.
    sub: String,
    x: f64,
    y: f64,
}

impl GNode {
    fn size(&self) -> (f64, f64) {
        match self.kind {
            Kind::Part => (FACTOR, FACTOR),
            _ => (VAR_W, VAR_H),
        }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        let (w, h) = self.size();
        let pad = if self.kind == Kind::Part { 14.0 } else { 4.0 };
        (x - self.x).abs() <= w / 2.0 + pad && (y - self.y).abs() <= h / 2.0 + pad
    }

    /// Where an edge leaves it (`out`) or reaches it.
    fn anchor(&self, out: bool) -> (f64, f64) {
        let (w, _) = self.size();
        let dx = if out { w / 2.0 } else { -w / 2.0 };
        (self.x + dx, self.y)
    }

    /// The variable's name, for a variable.
    fn variable(&self, doc: &RulesDoc) -> Option<String> {
        match &self.key {
            NodeKey::Input(n) => Some(n.clone()),
            NodeKey::Quantity(q) => doc.quantity(*q).map(|x| x.name.clone()),
            NodeKey::Part(..) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct GEdge {
    from: String,
    to: String,
    /// The axis it stands for; `None` for a factor's edge to its quantity.
    axis: Option<(u64, u64, usize)>,
    /// A quantity read by a part: a link in a chain.
    chain: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    /// Moving node `id`, grabbed `dx, dy` from its centre, from `start`.
    Move {
        id: String,
        dx: f64,
        dy: f64,
        start: (f64, f64),
        moved: bool,
    },
    /// A link from the variable `from` (a node id), its end at `at`.
    Wire { from: String, at: (f64, f64) },
}

const VAR_W: f64 = 150.0;
const VAR_H: f64 = 46.0;
const FACTOR: f64 = 26.0;
const COL: f64 = 200.0;
const ROW: f64 = 92.0;
const MARGIN: f64 = 40.0;

/// The graph editor's own state; the document is the rules editor's.
#[derive(Clone, Copy)]
pub struct GCtx {
    /// Node id -> centre, where the user put it.
    pos: RwSignal<HashMap<String, (f64, f64)>>,
    pub sel: RwSignal<Option<GSel>>,
    drag: RwSignal<Option<Drag>>,
}

pub fn gctx() -> GCtx {
    expect_context::<GCtx>()
}

pub fn graph_context() -> GCtx {
    let pos = RwSignal::new(load_layout());
    let ctx = GCtx {
        pos,
        sel: RwSignal::new(None),
        drag: RwSignal::new(None),
    };
    Effect::new(move |_| save_layout(&pos.get()));
    ctx
}

fn load_layout() -> HashMap<String, (f64, f64)> {
    io::load_text(LAYOUT_KEY)
        .map(|t| {
            t.lines()
                .filter_map(|l| {
                    let mut it = l.rsplitn(3, ' ');
                    let y = it.next()?.parse().ok()?;
                    let x = it.next()?.parse().ok()?;
                    Some((it.next()?.to_string(), (x, y)))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save_layout(pos: &HashMap<String, (f64, f64)>) {
    let mut lines: Vec<String> = pos
        .iter()
        .map(|(k, (x, y))| format!("{k} {x:.0} {y:.0}"))
        .collect();
    lines.sort();
    io::save_text(LAYOUT_KEY, &lines.join("\n"));
}

fn input_id(name: &str) -> String {
    format!("in:{name}")
}

fn quantity_id(name: &str) -> String {
    format!("q:{name}")
}

fn part_id(quantity: &str, part: &str) -> String {
    format!("p:{quantity}/{part}")
}

/// Each quantity's depth: 1 + the deepest quantity its parts read (the
/// inputs are at 0). A cycle, which the reader refuses, stops the count.
fn depths(doc: &RulesDoc) -> HashMap<String, usize> {
    fn depth(
        name: &str,
        doc: &RulesDoc,
        memo: &mut HashMap<String, usize>,
        path: &mut Vec<String>,
    ) -> usize {
        if let Some(d) = memo.get(name) {
            return *d;
        }
        let Some(q) = doc.quantities.iter().find(|q| q.name == name) else {
            return 0;
        };
        if path.iter().any(|p| p == name) {
            return 1;
        }
        path.push(name.to_string());
        let mut d = 1;
        for part in &q.parts {
            for r in part.reads() {
                if doc.quantities.iter().any(|x| x.name == r) {
                    d = d.max(depth(&r, doc, memo, path) + 1);
                }
            }
        }
        path.pop();
        memo.insert(name.to_string(), d);
        d
    }
    let mut memo = HashMap::new();
    for q in &doc.quantities {
        depth(&q.name, doc, &mut memo, &mut Vec::new());
    }
    memo
}

/// Every node where the layered layout puts it: the inputs in the first
/// column, then for each depth a column of factors and one of quantities.
fn auto_layout(doc: &RulesDoc) -> HashMap<String, (f64, f64)> {
    let depth = depths(doc);
    let mut rows: HashMap<usize, usize> = HashMap::new();
    let mut place = |col: usize| {
        let row = rows.entry(col).or_insert(0);
        let at = (
            MARGIN + VAR_W / 2.0 + col as f64 * COL,
            MARGIN + VAR_H / 2.0 + *row as f64 * ROW,
        );
        *row += 1;
        at
    };
    let mut out = HashMap::new();
    for name in doc.free_inputs() {
        out.insert(input_id(&name), place(0));
    }
    let mut quantities: Vec<&QuantityDoc> = doc.quantities.iter().collect();
    quantities.sort_by_key(|q| depth.get(&q.name).copied().unwrap_or(1));
    for q in quantities {
        let d = depth.get(&q.name).copied().unwrap_or(1);
        for p in &q.parts {
            out.insert(part_id(&q.name, &p.name), place(2 * d - 1));
        }
        out.insert(quantity_id(&q.name), place(2 * d));
    }
    out
}

/// The tested values: the inputs (the probe's, else the default, else the
/// middle of their points) and every quantity at its median.
fn tested(set: &RuleSet, probe: &[(String, f64)]) -> HashMap<String, f64> {
    let mut inputs = indexmap::IndexMap::new();
    for q in set.quantities.keys() {
        for (k, v) in rules_doc::values_at(set, q, probe) {
            inputs.entry(k).or_insert(v);
        }
    }
    let mut out: HashMap<String, f64> = inputs.iter().map(|(k, v)| (k.clone(), *v)).collect();
    if let Ok(all) = set.sample(&inputs, &indexmap::IndexMap::new()) {
        out.extend(all);
    }
    out
}

fn dist_label(part: &PartDoc) -> String {
    let family = part
        .dist
        .as_ref()
        .map(|d| d.family.name().to_string())
        .unwrap_or_else(|| "extended".into());
    match &part.fixed {
        FixedDoc::None => family,
        FixedDoc::All => format!("{family} · fixed"),
        FixedDoc::Some(_) => format!("{family} · part fixed"),
    }
}

/// The nodes and edges of the document, placed.
fn build(
    doc: &RulesDoc,
    pos: &HashMap<String, (f64, f64)>,
    values: &HashMap<String, f64>,
) -> (Vec<GNode>, Vec<GEdge>) {
    let auto = auto_layout(doc);
    let at = |id: &str| -> (f64, f64) {
        pos.get(id)
            .or_else(|| auto.get(id))
            .copied()
            .unwrap_or((MARGIN + VAR_W / 2.0, MARGIN + VAR_H / 2.0))
    };
    let value = |name: &str| {
        values
            .get(name)
            .map(|v| format!("= {}", short(*v)))
            .unwrap_or_default()
    };
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for name in doc.free_inputs() {
        let id = input_id(&name);
        let (x, y) = at(&id);
        nodes.push(GNode {
            key: NodeKey::Input(name.clone()),
            id,
            kind: Kind::Input,
            sub: value(&name),
            label: name,
            x,
            y,
        });
    }
    let quantity_names: Vec<&str> = doc.quantities.iter().map(|q| q.name.as_str()).collect();
    for q in &doc.quantities {
        let qid = quantity_id(&q.name);
        let (x, y) = at(&qid);
        nodes.push(GNode {
            key: NodeKey::Quantity(q.id),
            id: qid.clone(),
            kind: Kind::Quantity,
            label: q.name.clone(),
            sub: value(&q.name),
            x,
            y,
        });
        for p in &q.parts {
            let pid = part_id(&q.name, &p.name);
            let (x, y) = at(&pid);
            nodes.push(GNode {
                key: NodeKey::Part(q.id, p.id),
                id: pid.clone(),
                kind: Kind::Part,
                label: p.name.clone(),
                sub: dist_label(p),
                x,
                y,
            });
            for (k, input) in p.reads().iter().enumerate() {
                let chain = quantity_names.contains(&input.as_str());
                edges.push(GEdge {
                    from: if chain {
                        quantity_id(input)
                    } else {
                        input_id(input)
                    },
                    to: pid.clone(),
                    axis: Some((q.id, p.id, k)),
                    chain,
                });
            }
            edges.push(GEdge {
                from: pid,
                to: qid.clone(),
                axis: None,
                chain: false,
            });
        }
    }
    (nodes, edges)
}

/// The points a new axis over `input` starts with: those another part
/// reads it at, else a quantity's range at the tested inputs, else 0-100.
fn points_for(doc: &RulesDoc, set: Option<&RuleSet>, input: &str) -> Vec<f64> {
    for q in &doc.quantities {
        for p in &q.parts {
            for a in p.axes.iter().flatten() {
                if a.input == input {
                    return a.points.clone();
                }
            }
        }
    }
    if let Some(set) = set {
        if set.quantities.contains_key(input) {
            let at = rules_doc::values_at(set, input, &[]);
            if let Ok(j) = set.joined(input, &at) {
                let (lo, hi) = j.support();
                if hi > lo {
                    return vec![tidy(lo), tidy(hi)];
                }
            }
        }
    }
    vec![0.0, 100.0]
}

/// Make the variable `from` read by the node `to` (a factor, or a quantity,
/// which then gets a part of its own). `Err` says why not.
fn connect(from: &str, to: &NodeKey) -> Result<Option<GSel>, String> {
    let r = rctx();
    let set = r.good.get_untracked();
    let mut out = Err(String::new());
    r.doc.update(|d| {
        let q = match to {
            NodeKey::Part(q, _) | NodeKey::Quantity(q) => *q,
            NodeKey::Input(_) => {
                out = Err("an input reads nothing: link a variable to a factor".into());
                return;
            }
        };
        let Some(qname) = d.quantity(q).map(|x| x.name.clone()) else {
            return;
        };
        if d.reaches(&qname, from) {
            out = Err(if qname == from {
                format!("{qname} cannot read itself")
            } else {
                format!("{from} reads {qname}: {qname} reading {from} would make a cycle")
            });
            return;
        }
        let points = points_for(d, set.as_ref(), from);
        match to {
            NodeKey::Part(q, p) => {
                let Some(part) = d.part_mut(*q, *p) else {
                    return;
                };
                if part.reads().iter().any(|i| i == from) {
                    out = Err(format!("{} already reads {from}", part.name));
                    return;
                }
                let k = part.reads().len();
                part.add_axis(from.to_string(), points);
                out = Ok(Some(GSel::Edge(*q, *p, k)));
            }
            NodeKey::Quantity(q) => {
                let Some(qd) = d.quantity_mut(*q) else { return };
                let taken: Vec<String> = qd.parts.iter().map(|b| b.name.clone()).collect();
                let mut part = PartDoc::new(unique(from, &taken), from.to_string());
                if let Some(axes) = &mut part.axes {
                    axes[0].points = points.clone();
                }
                if let Some(dist) = &mut part.dist {
                    for prm in &mut dist.params {
                        let v = prm.values[0];
                        prm.values = vec![v; points.len()];
                    }
                }
                let id = part.id;
                qd.parts.push(part);
                out = Ok(Some(GSel::Node(NodeKey::Part(*q, id))));
            }
            NodeKey::Input(_) => {}
        }
    });
    out
}

fn add_input() {
    let r = rctx();
    let mut name = String::new();
    r.doc.update(|d| {
        let mut taken = d.free_inputs();
        taken.extend(d.quantities.iter().map(|q| q.name.clone()));
        name = unique("input", &taken);
        d.inputs.push(InputDoc {
            id: fresh_id(),
            name: name.clone(),
            default: None,
            text: String::new(),
            concept: None,
            unit: String::new(),
        });
    });
    gctx().sel.set(Some(GSel::Node(NodeKey::Input(name))));
}

fn add_quantity() {
    let r = rctx();
    let g = gctx();
    let mut sel = None;
    r.doc.update(|d| {
        let mut taken: Vec<String> = d.quantities.iter().map(|q| q.name.clone()).collect();
        taken.extend(d.free_inputs());
        let input = d
            .free_inputs()
            .into_iter()
            .next()
            .unwrap_or_else(|| "x".into());
        let q = QuantityDoc {
            id: fresh_id(),
            name: unique("quantity", &taken),
            combine: None,
            parts: vec![PartDoc::new("part".into(), input)],
            text: String::new(),
            basis: None,
        };
        sel = Some(GSel::Node(NodeKey::Quantity(q.id)));
        d.quantities.push(q);
    });
    g.sel.set(sel);
}

/// The point of `ev` in the graph's coordinates: the canvas is drawn
/// scaled down to fit its width, `size` across.
fn local(svg: &NodeRef<leptos::svg::Svg>, size: (f64, f64), x: i32, y: i32) -> Option<(f64, f64)> {
    let el = svg.get_untracked()?;
    let rect = el.get_bounding_client_rect();
    if rect.width() <= 0.0 {
        return None;
    }
    let k = size.0 / rect.width();
    Some(((x as f64 - rect.left()) * k, (y as f64 - rect.top()) * k))
}

/// A cubic from `a` rightwards into `b`.
fn curve(a: (f64, f64), b: (f64, f64)) -> String {
    let c = ((b.0 - a.0).abs() / 2.0).max(40.0);
    format!(
        "M{:.1},{:.1} C{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}",
        a.0,
        a.1,
        a.0 + c,
        a.1,
        b.0 - c,
        b.1,
        b.0,
        b.1
    )
}

#[component]
pub fn GraphStudio() -> impl IntoView {
    let r = rctx();
    let g = gctx();
    // The rules editor's selection follows the graph's, and the other way
    // round, so its forms and its test panel act on what is selected here.
    Effect::new(move |_| match r.sel.get() {
        RSel::Part(q, p) => {
            let want = Some(GSel::Node(NodeKey::Part(q, p)));
            if g.sel.get_untracked() != want
                && !matches!(g.sel.get_untracked(), Some(GSel::Edge(q2, p2, _)) if q2 == q && p2 == p)
            {
                g.sel.set(want);
            }
        }
        RSel::Quantity(q) => {
            let want = Some(GSel::Node(NodeKey::Quantity(q)));
            if g.sel.get_untracked() != want {
                g.sel.set(want);
            }
        }
        RSel::File => {
            if matches!(
                g.sel.get_untracked(),
                Some(GSel::Node(NodeKey::Part(..) | NodeKey::Quantity(_)))
            ) {
                g.sel.set(None);
            }
        }
    });
    Effect::new(move |_| {
        let want = match g.sel.get() {
            Some(GSel::Node(NodeKey::Part(q, p))) | Some(GSel::Edge(q, p, _)) => RSel::Part(q, p),
            Some(GSel::Node(NodeKey::Quantity(q))) => RSel::Quantity(q),
            _ => return,
        };
        if r.sel.get_untracked() != want {
            r.sel.set(want);
        }
    });
    view! {
        <div class="app">
            <RulesToolbar/>
            <main class="layout graph-layout">
                <section class="canvas-wrap">
                    <ErrorCard/>
                    <GraphCanvas/>
                </section>
                <aside class="panel graph-side">
                    <SidePanel/>
                </aside>
            </main>
        </div>
    }
}

#[component]
fn GraphCanvas() -> impl IntoView {
    let r = rctx();
    let g = gctx();
    let svg = NodeRef::<leptos::svg::Svg>::new();
    let values = Memo::new(move |_| {
        let probe = r.probe.get();
        r.good
            .with(|s| s.as_ref().map(|s| tested(s, &probe)))
            .unwrap_or_default()
    });
    let graph = Memo::new(move |_| {
        let pos = g.pos.get();
        let values = values.get();
        r.doc.with(|d| build(d, &pos, &values))
    });
    let node_at = move |x: f64, y: f64| -> Option<GNode> {
        graph.with_untracked(|(nodes, _)| nodes.iter().rev().find(|n| n.contains(x, y)).cloned())
    };
    // Held while a node is dragged, so the drawing's scale does not change
    // under the pointer; it fits the graph again on release.
    let size = Memo::new(move |prev: Option<&(f64, f64)>| {
        let dragging = g.drag.with(Option::is_some);
        if let (true, Some(p)) = (dragging, prev) {
            return *p;
        }
        graph.with(|(nodes, _)| {
            let w = nodes.iter().map(|n| n.x + VAR_W).fold(600.0, f64::max);
            let h = nodes
                .iter()
                .map(|n| n.y + VAR_H + 40.0)
                .fold(400.0, f64::max);
            (w + MARGIN, h + MARGIN)
        })
    });
    let down = move |ev: leptos::ev::PointerEvent| {
        if ev.button() != 0 {
            return;
        }
        let Some((x, y)) = local(&svg, size.get_untracked(), ev.client_x(), ev.client_y()) else {
            return;
        };
        let target = ev
            .target()
            .and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok());
        let port = target.as_ref().and_then(|t| t.get_attribute("data-port"));
        if let Some(from) = port {
            g.drag.set(Some(Drag::Wire { from, at: (x, y) }));
        } else if let Some(n) = node_at(x, y) {
            g.drag.set(Some(Drag::Move {
                id: n.id.clone(),
                dx: x - n.x,
                dy: y - n.y,
                start: (x, y),
                moved: false,
            }));
        } else {
            if target
                .as_ref()
                .and_then(|t| t.get_attribute("data-edge"))
                .is_none()
            {
                g.sel.set(None);
            }
            return;
        }
        if let Some(el) = svg.get_untracked() {
            let _ = el.set_pointer_capture(ev.pointer_id());
        }
        ev.prevent_default();
    };
    let moving = move |ev: leptos::ev::PointerEvent| {
        let Some(drag) = g.drag.get_untracked() else {
            return;
        };
        let Some((x, y)) = local(&svg, size.get_untracked(), ev.client_x(), ev.client_y()) else {
            return;
        };
        match drag {
            Drag::Move {
                id,
                dx,
                dy,
                start,
                moved,
            } => {
                let far = (x - start.0).abs() + (y - start.1).abs() > 3.0;
                if moved || far {
                    let at = (
                        (x - dx).max(VAR_W / 2.0 + 4.0),
                        (y - dy).max(VAR_H / 2.0 + 4.0),
                    );
                    g.pos.update(|p| {
                        p.insert(id.clone(), at);
                    });
                    g.drag.set(Some(Drag::Move {
                        id,
                        dx,
                        dy,
                        start,
                        moved: true,
                    }));
                }
            }
            Drag::Wire { from, .. } => g.drag.set(Some(Drag::Wire { from, at: (x, y) })),
        }
    };
    let up = move |ev: leptos::ev::PointerEvent| {
        let Some(drag) = g.drag.get_untracked() else {
            return;
        };
        g.drag.set(None);
        if let Some(el) = svg.get_untracked() {
            let _ = el.release_pointer_capture(ev.pointer_id());
        }
        let Some((x, y)) = local(&svg, size.get_untracked(), ev.client_x(), ev.client_y()) else {
            return;
        };
        match drag {
            Drag::Move { id, moved, .. } => {
                if !moved {
                    let key = graph.with_untracked(|(nodes, _)| {
                        nodes.iter().find(|n| n.id == id).map(|n| n.key.clone())
                    });
                    if let Some(k) = key {
                        g.sel.set(Some(GSel::Node(k)));
                    }
                }
            }
            Drag::Wire { from, .. } => {
                let Some(target) = node_at(x, y) else { return };
                let name = graph.with_untracked(|(nodes, _)| {
                    let n = nodes.iter().find(|n| n.id == from)?;
                    r.doc.with_untracked(|d| n.variable(d))
                });
                let Some(name) = name else { return };
                match connect(&name, &target.key) {
                    Ok(sel) => {
                        if sel.is_some() {
                            g.sel.set(sel);
                        }
                    }
                    Err(e) if !e.is_empty() => r.notice.set(Some((false, e))),
                    Err(_) => {}
                }
            }
        }
    };
    let selected_node = move |id: &str| -> bool {
        let sel = g.sel.get();
        graph.with(|(nodes, _)| {
            nodes
                .iter()
                .any(|n| n.id == id && sel == Some(GSel::Node(n.key.clone())))
        })
    };
    let edges = move || {
        let sel = g.sel.get();
        graph.with(|(nodes, edges)| {
            let find = |id: &str| nodes.iter().find(|n| n.id == id);
            edges
                .iter()
                .filter_map(|e| {
                    let a = find(&e.from)?.anchor(true);
                    let b = find(&e.to)?.anchor(false);
                    let d = curve(a, b);
                    let selected = matches!((&sel, e.axis), (Some(GSel::Edge(q, p, k)), Some((q2, p2, k2))) if *q == q2 && *p == p2 && *k == k2);
                    let class = format!(
                        "g-edge{}{}{}",
                        if e.axis.is_none() { " out" } else { "" },
                        if e.chain { " chain" } else { "" },
                        if selected { " selected" } else { "" }
                    );
                    let axis = e.axis;
                    let hit = d.clone();
                    Some(view! {
                        <g>
                            <path class=class d=d marker-end=if axis.is_none() { "url(#g-arrow)" } else { "" }/>
                            {axis.map(|(q, p, k)| view! {
                                <path class="g-edge-hit" d=hit data-edge="1"
                                    on:pointerdown=move |ev| {
                                        ev.stop_propagation();
                                        g.sel.set(Some(GSel::Edge(q, p, k)));
                                    }/>
                            })}
                        </g>
                    })
                })
                .collect_view()
        })
    };
    let nodes = move || {
        graph.with(|(nodes, _)| {
            nodes
                .iter()
                .map(|n| {
                    let id = n.id.clone();
                    let sel = { let id = id.clone(); move || selected_node(&id) };
                    let (w, h) = n.size();
                    let (x, y) = (n.x, n.y);
                    let label = n.label.clone();
                    let sub = n.sub.clone();
                    match n.kind {
                        Kind::Part => view! {
                            <g class="g-node factor" class:selected=sel>
                                <rect x=x - w / 2.0 y=y - h / 2.0 width=w height=h rx="4"/>
                                <text class="g-label" x=x y=y + h / 2.0 + 15.0>{label}</text>
                                <text class="g-sub" x=x y=y + h / 2.0 + 29.0>{sub}</text>
                            </g>
                        }
                        .into_any(),
                        kind => {
                            let class = if kind == Kind::Input { "g-node var input" } else { "g-node var quantity" };
                            view! {
                                <g class=class class:selected=sel>
                                    <rect x=x - w / 2.0 y=y - h / 2.0 width=w height=h rx=h / 2.0/>
                                    <text class="g-label" x=x y=y - 2.0>{label}</text>
                                    <text class="g-sub" x=x y=y + 13.0>{sub}</text>
                                    <circle class="g-port" cx=x + w / 2.0 cy=y r="7" data-port=id>
                                        <title>"Drag onto a factor, or a quantity, to make it read this"</title>
                                    </circle>
                                </g>
                            }
                            .into_any()
                        }
                    }
                })
                .collect_view()
        })
    };
    let wire = move || {
        let Some(Drag::Wire { from, at }) = g.drag.get() else {
            return None;
        };
        let a =
            graph.with(|(nodes, _)| nodes.iter().find(|n| n.id == from).map(|n| n.anchor(true)))?;
        Some(view! { <path class="g-edge wire" d=curve(a, at)/> })
    };
    let relayout = move |_| {
        g.pos.set(HashMap::new());
    };
    view! {
        <div class="graph-tools">
            <button class="button" on:click=move |_| add_input()><Icon name="plus"/>"Input"</button>
            <button class="button" on:click=move |_| add_quantity()><Icon name="plus"/>"Quantity"</button>
            <button class="button" title="Place every node again, in layers" on:click=relayout><Icon name="graph"/>"Arrange"</button>
            <span class="graph-hint">
                "Drag a variable's "<span class="g-port-key"></span>" onto a factor "<span class="g-factor-key"></span>" or a quantity to make it read the variable."
            </span>
            <span class="graph-key">
                <span class="key input">"input"</span>
                <span class="key quantity">"quantity"</span>
                <span class="key factor">"factor (part)"</span>
                <span class="key chain">"chain"</span>
            </span>
        </div>
        <div class="graph-scroll">
            <svg
                node_ref=svg
                class="graph"
                class:wiring=move || matches!(g.drag.get(), Some(Drag::Wire { .. }))
                viewBox=move || { let (w, h) = size.get(); format!("0 0 {w:.0} {h:.0}") }
                style=move || format!("max-width: {:.0}px", size.get().0)
                on:pointerdown=down
                on:pointermove=moving
                on:pointerup=up
                on:pointercancel=move |_| g.drag.set(None)
            >
                <defs>
                    <marker id="g-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                        <path d="M0,0 L10,5 L0,10 z" class="g-arrowhead"/>
                    </marker>
                </defs>
                {edges}
                {nodes}
                {wire}
            </svg>
        </div>
    }
}

#[component]
fn SidePanel() -> impl IntoView {
    let g = gctx();
    move || {
        match g.sel.get() {
        Some(GSel::Node(NodeKey::Part(q, p))) => view! {
            <div class="graph-form"><PartView q=q p=p/></div>
        }
        .into_any(),
        Some(GSel::Node(NodeKey::Quantity(q))) => view! {
            <div class="graph-form"><QuantityView q=q/></div>
        }
        .into_any(),
        Some(GSel::Node(NodeKey::Input(name))) => view! { <InputCard name=name/> }.into_any(),
        Some(GSel::Edge(q, p, k)) => view! { <EdgeCard q=q p=p k=k/> }.into_any(),
        None => view! {
            <div class="graph-form">
                <section class="card">
                    <h3>"Factor graph"</h3>
                    <p class="muted">
                        "Variables are the inputs the backend gives and the quantities the rules derive. "
                        "Each factor is a part: the distribution of its quantity given the variables it reads. "
                        "A factor may read a quantity, so quantities chain; a quantity read and not given is taken at its median."
                    </p>
                    <p class="muted">"Select a node to edit it. The test below moves the inputs and shows every quantity."</p>
                </section>
                <crate::fit_app::FitCard/>
            </div>
            <RulesPanel/>
        }
        .into_any(),
    }
    }
}

#[component]
fn InputCard(name: String) -> impl IntoView {
    let r = rctx();
    let g = gctx();
    let current = RwSignal::new(name);
    let described = Memo::new(move |_| {
        let n = current.get();
        r.doc
            .with(|d| d.inputs.iter().find(|i| i.name == n).cloned())
    });
    let readers = Memo::new(move |_| {
        let n = current.get();
        r.doc.with(|d| {
            d.quantities
                .iter()
                .flat_map(|q| {
                    q.parts
                        .iter()
                        .filter(|p| p.reads().contains(&n))
                        .map(|p| (q.id, p.id, format!("{} · {}", q.name, p.name)))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
    });
    let ensure = move |d: &mut RulesDoc, n: &str| {
        if !d.inputs.iter().any(|i| i.name == n) {
            d.inputs.push(InputDoc {
                id: fresh_id(),
                name: n.to_string(),
                default: None,
                text: String::new(),
                concept: None,
                unit: String::new(),
            });
        }
    };
    let rename = move |ev: leptos::ev::Event| {
        let new = event_target_value(&ev).trim().to_string();
        let old = current.get_untracked();
        if new.is_empty() || new == old {
            return;
        }
        let clash = r.doc.with_untracked(|d| {
            d.quantities.iter().any(|q| q.name == new) || d.free_inputs().contains(&new)
        });
        if clash {
            r.notice.set(Some((false, format!("'{new}' is taken"))));
            return;
        }
        r.doc.update(|d| d.rename_input(&old, &new));
        g.pos.update(|p| {
            if let Some(at) = p.remove(&input_id(&old)) {
                p.insert(input_id(&new), at);
            }
        });
        current.set(new.clone());
        g.sel.set(Some(GSel::Node(NodeKey::Input(new))));
    };
    let set_default = move |ev: leptos::ev::Event| {
        let v = event_target_value(&ev).trim().parse::<f64>().ok();
        let n = current.get_untracked();
        r.doc.update(|d| {
            ensure(d, &n);
            if let Some(i) = d.inputs.iter_mut().find(|i| i.name == n) {
                i.default = v;
            }
        });
    };
    let set_text = move |ev: leptos::ev::Event| {
        let t = event_target_value(&ev);
        let n = current.get_untracked();
        r.doc.update(|d| {
            ensure(d, &n);
            if let Some(i) = d.inputs.iter_mut().find(|i| i.name == n) {
                i.text = t;
            }
        });
    };
    let remove = move |_| {
        let n = current.get_untracked();
        r.doc.update(|d| d.inputs.retain(|i| i.name != n));
        g.sel.set(None);
    };
    view! {
        <div class="graph-form">
            <section class="card">
                <div class="card-head">
                    <span class="kind-chip input">"input"</span>
                    <input class="title-input" prop:value=move || current.get() on:change=rename aria-label="Input name"/>
                </div>
                <p class="muted">"A value the backend gives the rules (a knob's, or one it derives)."</p>
                <div class="grid2">
                    <label class="field">
                        <span>"Default"</span>
                        <input inputmode="decimal" placeholder="none"
                            prop:value=move || described.with(|i| i.as_ref().and_then(|i| i.default).map(num).unwrap_or_default())
                            on:change=set_default/>
                    </label>
                    <label class="field">
                        <span>"What it is"</span>
                        <input prop:value=move || described.with(|i| i.as_ref().map(|i| i.text.clone()).unwrap_or_default())
                            on:change=set_text/>
                    </label>
                </div>
                <h4>"Read by"</h4>
                {move || {
                    let rs = readers.get();
                    if rs.is_empty() {
                        view! { <p class="muted">"No factor reads it yet: drag its port onto one."</p> }.into_any()
                    } else {
                        view! {
                            <ul class="plain">
                                {rs.into_iter().map(|(q, p, label)| view! {
                                    <li><a href="#graph" on:click=move |ev| {
                                        ev.prevent_default();
                                        g.sel.set(Some(GSel::Node(NodeKey::Part(q, p))));
                                    }>{label}</a></li>
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }
                }}
                <div class="actions">
                    <button class="button danger" disabled=move || !readers.with(Vec::is_empty) on:click=remove>
                        <Icon name="x"/>"Remove"
                    </button>
                </div>
            </section>
        </div>
    }
}

#[component]
fn EdgeCard(q: u64, p: u64, k: usize) -> impl IntoView {
    let r = rctx();
    let g = gctx();
    let info = Memo::new(move |_| {
        r.doc.with(|d| {
            let qd = d.quantity(q)?;
            let pd = qd.parts.iter().find(|x| x.id == p)?;
            let axes = pd.axes.as_ref()?;
            let a = axes.get(k)?;
            let chain = d.quantities.iter().any(|x| x.name == a.input);
            Some((
                a.input.clone(),
                qd.name.clone(),
                pd.name.clone(),
                a.points.clone(),
                axes.len(),
                chain,
            ))
        })
    });
    let remove = move |_| {
        let mut removed = false;
        r.doc.update(|d| {
            if let Some(part) = d.part_mut(q, p) {
                removed = part.remove_axis(k);
            }
        });
        if removed {
            g.sel.set(Some(GSel::Node(NodeKey::Part(q, p))));
        }
    };
    let remove_part = move |_| {
        r.doc.update(|d| {
            if let Some(qd) = d.quantity_mut(q) {
                qd.parts.retain(|x| x.id != p);
            }
        });
        g.sel.set(Some(GSel::Node(NodeKey::Quantity(q))));
    };
    move || {
        let Some((input, qn, pn, points, n, chain)) = info.get() else {
            return view! { <div class="graph-form"><p class="muted">"Gone."</p></div> }.into_any();
        };
        let pts = points
            .iter()
            .map(|v| num(*v))
            .collect::<Vec<_>>()
            .join(", ");
        view! {
            <div class="graph-form">
                <section class="card">
                    <div class="card-head">
                        <span class=if chain { "kind-chip chain" } else { "kind-chip input" }>{if chain { "chain" } else { "reads" }}</span>
                        <strong>{format!("{input} → {qn} · {pn}")}</strong>
                    </div>
                    <p class="muted">
                        {if chain {
                            format!("The part {pn} of {qn} reads the quantity {input}: its distribution is written at {input}'s points, and {input} is taken at its own quantile unless given.")
                        } else {
                            format!("The part {pn} of {qn} reads the input {input}.")
                        }}
                    </p>
                    <p>"Points: "<code>{pts}</code></p>
                    <div class="actions">
                        <button class="button" on:click=move |_| g.sel.set(Some(GSel::Node(NodeKey::Part(q, p))))>
                            <Icon name="graph"/>"Edit the factor"
                        </button>
                        {if n > 1 {
                            view! {
                                <button class="button danger" title="The factor stops reading it: its distribution is the one at the first point" on:click=remove>
                                    <Icon name="x"/>"Remove the link"
                                </button>
                            }.into_any()
                        } else {
                            view! {
                                <button class="button danger" title="It is the factor's only input: remove the factor" on:click=remove_part>
                                    <Icon name="x"/>"Remove the factor"
                                </button>
                            }.into_any()
                        }}
                    </div>
                </section>
            </div>
        }
        .into_any()
    }
}
