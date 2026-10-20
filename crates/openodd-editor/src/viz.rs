//! What the editor draws: an attribute's domain coloured by where the ODD
//! stands in each bucket (a number line one can drag, a strip of literals),
//! the graph of modules and what they test, and a map of the ODD over two
//! attributes.

use std::collections::HashMap;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::check::{Attribute, Report, State};
use crate::doc::{tail, Document, KindTag, Rule, RuleKind};
use crate::icons::{concept_icon, Icon};
use openodd::value::parse_float;

// ---------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------

/// The span a number line shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub lo: f64,
    pub hi: f64,
    /// Values snap to whole numbers.
    pub integer: bool,
}

impl Scale {
    /// Around the finite edges of an attribute and any other values to show;
    /// `integer` snaps values to whole numbers.
    pub fn around(edges: &[f64], extra: &[f64], integer: bool) -> Scale {
        let finite: Vec<f64> = edges
            .iter()
            .chain(extra)
            .copied()
            .filter(|v| v.is_finite())
            .collect();
        let (min, max) = match (
            finite.iter().copied().reduce(f64::min),
            finite.iter().copied().reduce(f64::max),
        ) {
            (Some(a), Some(b)) => (a, b),
            _ => (0.0, 100.0),
        };
        let span = if max > min {
            max - min
        } else {
            min.abs().max(10.0)
        };
        let pad = span * 0.25;
        let mut lo = min - pad;
        // A domain starting at zero (a speed, a count) is shown from zero.
        if min >= 0.0 && lo < 0.0 {
            lo = if min == 0.0 { -span * 0.08 } else { 0.0 };
        }
        Scale {
            lo,
            hi: max + pad,
            integer,
        }
    }

    pub fn x(&self, v: f64, width: f64) -> f64 {
        if v == f64::NEG_INFINITY {
            return 0.0;
        }
        if v == f64::INFINITY {
            return width;
        }
        ((v - self.lo) / (self.hi - self.lo) * width).clamp(0.0, width)
    }

    pub fn value(&self, x: f64, width: f64) -> f64 {
        let v = self.lo + (x / width).clamp(0.0, 1.0) * (self.hi - self.lo);
        round_nice(v, self.step())
    }

    /// The step dragged values snap to: 1, 2 or 5 times a power of ten.
    pub fn step(&self) -> f64 {
        let raw = (self.hi - self.lo) / 120.0;
        if self.integer && raw <= 1.0 {
            return 1.0;
        }
        let p = 10f64.powf(raw.log10().floor());
        let m = raw / p;
        p * if m < 1.5 {
            1.0
        } else if m < 3.5 {
            2.0
        } else if m < 7.5 {
            5.0
        } else {
            10.0
        }
    }
}

fn round_nice(v: f64, step: f64) -> f64 {
    let r = (v / step).round() * step;
    // 0.30000000000000004 -> 0.3
    let digits = (-step.log10().floor()).max(0.0) as i32;
    let f = 10f64.powi(digits);
    (r * f).round() / f
}

/// A number as a person writes it: `60`, `2.5`, `∞`.
pub fn fmt_num(v: f64) -> String {
    if v == f64::INFINITY {
        return "∞".into();
    }
    if v == f64::NEG_INFINITY {
        return "−∞".into();
    }
    let r = (v * 1e6).round() / 1e6;
    let s = format!("{r}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

const W: f64 = 600.0;

/// An attribute's number line: its buckets coloured by state, with what a
/// condition requires drawn over it (its ends draggable) and a test value.
#[component]
pub fn NumberLine(
    #[prop(into)] attr: Signal<Option<Attribute>>,
    /// The module whose verdict to draw instead of the ODD's.
    #[prop(default = None)]
    module: Option<Signal<String>>,
    /// A condition's interval, its ends possibly infinite.
    #[prop(optional)]
    interval: Option<Signal<Option<(f64, f64)>>>,
    /// Draggable values (a condition's finite ends, a test value).
    #[prop(default = None)]
    handles: Option<Signal<Vec<f64>>>,
    #[prop(default = None)] on_drag: Option<Callback<(usize, f64)>>,
    /// A click on the line, at a value.
    #[prop(default = None)]
    on_pick: Option<Callback<f64>>,
    #[prop(optional)] marker: Option<Signal<Option<f64>>>,
    #[prop(optional)] compact: bool,
) -> impl IntoView {
    let svg_ref = NodeRef::<leptos::svg::Svg>::new();
    let dragging = RwSignal::new(None::<usize>);
    let frozen = RwSignal::new(None::<Scale>);
    let w = if compact { 360.0 } else { W };
    let height = if compact { 46.0 } else { 62.0 };
    let track_y = if compact { 12.0 } else { 20.0 };
    let track_h = 12.0;
    let scale = Memo::new(move |_| {
        if let Some(s) = frozen.get() {
            return s;
        }
        let (edges, integer) = attr.with(|a| {
            a.as_ref()
                .map(|a| (a.edges.clone(), a.integer))
                .unwrap_or_default()
        });
        let mut extra = vec![];
        if let Some(i) = interval {
            if let Some((a, b)) = i.get() {
                extra.extend([a, b]);
            }
        }
        if let Some(m) = marker {
            extra.extend(m.get());
        }
        if let Some(h) = handles {
            extra.extend(h.get());
        }
        Scale::around(&edges, &extra, integer)
    });
    let to_value = move |client_x: f64| -> Option<f64> {
        let el = svg_ref.get()?;
        let el: &web_sys::Element = el.unchecked_ref();
        let rect = el.get_bounding_client_rect();
        if rect.width() <= 0.0 {
            return None;
        }
        let x = (client_x - rect.left()) / rect.width() * (w + 24.0) - 12.0;
        Some(scale.get_untracked().value(x, w))
    };
    let pointer_down = move |ev: web_sys::PointerEvent| {
        let target: web_sys::Element = ev.target().unwrap().unchecked_into();
        if let Some(i) = target
            .get_attribute("data-handle")
            .and_then(|s| s.parse().ok())
        {
            if on_drag.is_some() {
                frozen.set(Some(scale.get_untracked()));
                dragging.set(Some(i));
                if let Some(el) = svg_ref.get() {
                    let el: &web_sys::Element = el.unchecked_ref();
                    let _ = el.set_pointer_capture(ev.pointer_id());
                }
                ev.prevent_default();
            }
        } else if let (Some(pick), Some(v)) = (on_pick, to_value(ev.client_x() as f64)) {
            pick.run(v);
        }
    };
    let pointer_move = move |ev: web_sys::PointerEvent| {
        if let (Some(i), Some(cb)) = (dragging.get_untracked(), on_drag) {
            if let Some(v) = to_value(ev.client_x() as f64) {
                cb.run((i, v));
            }
        }
    };
    let pointer_up = move |_ev: web_sys::PointerEvent| {
        dragging.set(None);
        frozen.set(None);
    };
    // Where the ODD (or the module) stands along the line.
    let ctx = crate::app::ctx();
    let runs = Memo::new(move |_| {
        let s = scale.get();
        let m = module.map(|m| m.get());
        attr.with(|a| {
            let Some(a) = a else { return vec![] };
            ctx.report.with(|r| match r {
                Ok(r) => crate::check::runs(r, &a.name, m.as_deref(), s.lo, s.hi),
                Err(_) => vec![],
            })
        })
    });
    let segments = move || {
        let s = scale.get();
        runs.get()
            .into_iter()
            .map(|(a, b, state)| {
                let x0 = s.x(a, w);
                let x1 = s.x(b, w);
                let title = format!("{} .. {} — {}", fmt_num(a), fmt_num(b), state_words(state));
                view! {
                    <rect
                        class=format!("seg {}", state.class())
                        x=x0
                        y=track_y
                        width=(x1 - x0).max(0.0)
                        height=track_h
                    >
                        <title>{title}</title>
                    </rect>
                }
            })
            .collect_view()
    };
    let ticks = move || {
        let s = scale.get();
        let unit = attr.with(|a| a.as_ref().map(|a| a.unit.clone()).unwrap_or_default());
        let edges: Vec<f64> = attr.with(|a| {
            a.as_ref()
                .map(|a| a.edges.iter().copied().filter(|e| e.is_finite()).collect())
                .unwrap_or_default()
        });
        let held: Vec<f64> = handles.map(|h| h.get()).unwrap_or_default();
        let n = edges.len();
        edges
            .into_iter()
            .enumerate()
            .map(|(i, e)| {
                let x = s.x(e, w);
                let label = if held.iter().any(|h| (h - e).abs() < 1e-9) {
                    String::new()
                } else if i + 1 == n && !unit.is_empty() {
                    format!("{} {unit}", fmt_num(e))
                } else {
                    fmt_num(e)
                };
                view! {
                    <line class="tick" x1=x x2=x y1=track_y - 3.0 y2=track_y + track_h + 3.0/>
                    <text class="tick-label" x=x y=track_y + track_h + 16.0 text-anchor="middle">{label}</text>
                }
            })
            .collect_view()
    };
    let overlay = move || {
        let s = scale.get();
        let (a, b) = interval.and_then(|i| i.get())?;
        let x0 = s.x(a, w);
        let x1 = s.x(b, w);
        Some(view! {
            <rect class="interval" x=x0 y=track_y - 5.0 width=(x1 - x0).max(1.0) height=track_h + 10.0 rx="4"/>
            {(a == f64::NEG_INFINITY).then(|| view! { <path class="arrow" d=format!("M 8 {} l -7 5 l 7 5", track_y + 1.0)/> })}
            {(b == f64::INFINITY).then(|| view! { <path class="arrow" d=format!("M {} {} l 7 5 l -7 5", w - 8.0, track_y + 1.0)/> })}
        })
    };
    let handle_views = move || {
        let s = scale.get();
        let hs = handles.map(|h| h.get()).unwrap_or_default();
        hs.into_iter()
            .enumerate()
            .filter(|(_, v)| v.is_finite())
            .map(|(i, v)| {
                let x = s.x(v, w);
                let active = move || dragging.get() == Some(i);
                view! {
                    <g class="handle" class:active=active>
                        <line x1=x x2=x y1=track_y - 8.0 y2=track_y + track_h + 8.0/>
                        <circle cx=x cy=track_y + track_h / 2.0 r="8" data-handle=i.to_string()/>
                        {(!compact).then(|| view! {
                            <text class="handle-label" x=x y=track_y - 10.0 text-anchor="middle">{fmt_num(v)}</text>
                        })}
                    </g>
                }
            })
            .collect_view()
    };
    let marker_view = move || {
        let s = scale.get();
        let v = marker.and_then(|m| m.get())?;
        let x = s.x(v, w);
        Some(view! {
            <g class="marker">
                <path d=format!("M {x} {} l -6 -9 h 12 z", track_y - 1.0)/>
                <line x1=x x2=x y1=track_y y2=track_y + track_h/>
            </g>
        })
    };
    view! {
        <svg
            node_ref=svg_ref
            class="number-line"
            class:interactive=on_pick.is_some() || on_drag.is_some()
            viewBox=format!("-12 0 {} {height}", w + 24.0)
            preserveAspectRatio="xMidYMid meet"
            on:pointerdown=pointer_down
            on:pointermove=pointer_move
            on:pointerup=pointer_up
            on:pointercancel=pointer_up
        >
            <rect class="track" x="0" y=track_y width=w height=track_h rx="6"/>
            {segments}
            {ticks}
            {overlay}
            {marker_view}
            {handle_views}
        </svg>
    }
}

// ---------------------------------------------------------------------------
// Literals
// ---------------------------------------------------------------------------

/// A categorical's (or a boolean's) values, coloured by state.
#[component]
pub fn CategoryStrip(
    #[prop(into)] attr: Signal<Option<Attribute>>,
    #[prop(default = None)] states: Option<Signal<Vec<State>>>,
    #[prop(optional)] marker: Option<Signal<Option<String>>>,
    #[prop(default = None)] on_pick: Option<Callback<String>>,
) -> impl IntoView {
    move || {
        attr.with(|a| {
            let Some(a) = a else { return ().into_any() };
            let st = states.map(|s| s.get()).unwrap_or_else(|| a.states.clone());
            a.values
                .iter()
                .zip(st)
                .map(|(v, state)| {
                    let (shown, pick) = (v.clone(), v.clone());
                    let picked =
                        move || marker.and_then(|m| m.get()).as_deref() == Some(shown.as_str());
                    view! {
                        <button
                            class=format!("cat {}", state.class())
                            class:picked=picked
                            class:static=on_pick.is_none()
                            title=state_words(state)
                            on:click=move |_| if let Some(cb) = on_pick { cb.run(pick.clone()) }
                        >
                            <span class="dot"></span>
                            {v.clone()}
                        </button>
                    }
                })
                .collect_view()
                .into_any()
        })
    }
}

/// The domain of an attribute, drawn as fits it, with the test value on it,
/// set by dragging or clicking. With `module`, what that module accepts
/// instead of the ODD, to look at.
#[component]
pub fn Domain(
    #[prop(into)] attr: Signal<Option<Attribute>>,
    #[prop(optional)] module: Option<Signal<String>>,
    #[prop(optional)] compact: bool,
) -> impl IntoView {
    let ctx = crate::app::ctx();
    let name =
        Memo::new(move |_| attr.with(|a| a.as_ref().map(|a| a.name.clone()).unwrap_or_default()));
    let probe = move || name.with(|n| ctx.probe_value(n));
    let number = Memo::new(move |_| attr.with(|a| a.as_ref().is_some_and(Attribute::is_number)));
    let set = move |v: String| name.with(|n| ctx.set_probe(n, v));
    let states = module.map(|m| {
        Signal::from(Memo::new(move |_| {
            let m = m.get();
            name.with(|n| {
                ctx.report.with(|r| {
                    r.as_ref()
                        .map(|r| crate::check::module_states(r, &m, n))
                        .unwrap_or_default()
                })
            })
        }))
    });
    // A module's view is to look at; only the ODD's sets the test value.
    let editable = module.is_none();
    move || {
        if number.get() {
            let marker = Signal::derive(move || probe().and_then(|t| parse_float(&t)));
            let handles = editable
                .then(|| Signal::derive(move || marker.get().into_iter().collect::<Vec<_>>()));
            let drag = editable.then(|| Callback::new(move |(_, v): (usize, f64)| set(fmt_num(v))));
            let pick = editable.then(|| Callback::new(move |v: f64| set(fmt_num(v))));
            view! {
                <NumberLine attr=attr module=module marker=marker handles=handles on_drag=drag on_pick=pick compact=compact/>
            }
            .into_any()
        } else {
            let marker = Signal::derive(probe);
            let pick = editable.then(|| {
                Callback::new(move |v: String| {
                    let same = probe().as_deref() == Some(v.as_str());
                    set(if same { String::new() } else { v });
                })
            });
            view! {
                <div class="cats"><CategoryStrip attr=attr states=states marker=marker on_pick=pick/></div>
            }
            .into_any()
        }
    }
}

pub fn state_words(s: State) -> &'static str {
    match s {
        State::Inside => "allowed: inside the ODD for some values of the others",
        State::Outside => "excluded: outside the ODD whatever the others are",
    }
}

/// The share of buckets in each state, as a stacked bar.
#[component]
pub fn StateBar(counts: [usize; 2]) -> impl IntoView {
    let total = counts.iter().sum::<usize>().max(1) as f64;
    view! {
        <div class="state-bar">
            {[State::Inside, State::Outside]
                .into_iter()
                .zip(counts)
                .filter(|(_, n)| *n > 0)
                .map(|(s, n)| view! {
                    <span class=format!("part {}", s.class()) style=format!("flex: {}", n as f64 / total)></span>
                })
                .collect_view()}
        </div>
    }
}

#[component]
pub fn Legend() -> impl IntoView {
    view! {
        <div class="legend">
            <span><i class="sw inside"></i>"allowed"</span>
            <span><i class="sw outside"></i>"excluded"</span>
        </div>
    }
}

// ---------------------------------------------------------------------------
// The graph of modules
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NodeKind {
    Root,
    Module,
    Label,
    Attribute,
}

#[derive(Clone, Debug, PartialEq)]
struct Node {
    id: String,
    kind: NodeKind,
    title: String,
    sub: String,
    active: bool,
    col: usize,
    x: f64,
    y: f64,
    w: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct Edge {
    from: String,
    to: String,
    class: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
struct Layout {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    /// Each column's heading, at its left.
    headings: Vec<(f64, &'static str)>,
    width: f64,
    height: f64,
}

const NODE_H: f64 = 38.0;
const ROW: f64 = 50.0;
const COL: f64 = 225.0;
/// Room above the nodes for the columns' headings.
const HEAD: f64 = 30.0;

fn layout(r: &Report) -> Layout {
    let labels: Vec<String> = {
        let mut l: Vec<String> = vec![];
        for m in &r.modules {
            for x in &m.labels {
                if !l.contains(x) {
                    l.push(x.clone());
                }
            }
        }
        l
    };
    // Who points at whom, among modules and labels.
    let mut parents: HashMap<String, Vec<String>> = HashMap::new();
    let mut edges = vec![];
    for m in &r.modules {
        for l in &m.links {
            let class = if l.include { "include" } else { "exclude" };
            edges.push(Edge {
                from: m.name.clone(),
                to: l.target.clone(),
                class,
            });
            if l.module {
                parents
                    .entry(l.target.clone())
                    .or_default()
                    .push(m.name.clone());
            }
        }
        for label in &m.labels {
            edges.push(Edge {
                from: label.clone(),
                to: m.name.clone(),
                class: "carries",
            });
            parents
                .entry(m.name.clone())
                .or_default()
                .push(label.clone());
        }
    }
    let mut depth: HashMap<String, usize> = HashMap::new();
    let names: Vec<String> = r
        .modules
        .iter()
        .map(|m| m.name.clone())
        .chain(labels.iter().cloned())
        .collect();
    for n in &names {
        depth.insert(n.clone(), 0);
    }
    for _ in 0..names.len() + 1 {
        let mut changed = false;
        for n in &names {
            let d = parents
                .get(n)
                .map(|ps| ps.iter().map(|p| depth[p] + 1).max().unwrap_or(0))
                .unwrap_or(0)
                .min(names.len());
            if depth[n] != d {
                depth.insert(n.clone(), d);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let attr_col = depth.values().copied().max().map_or(0, |d| d + 1);
    let mut nodes: Vec<Node> = vec![];
    for m in &r.modules {
        let kind = if m.root {
            NodeKind::Root
        } else {
            NodeKind::Module
        };
        let sub = [("include", &m.include), ("exclude", &m.exclude)]
            .into_iter()
            .filter_map(|(w, s)| {
                s.as_ref()
                    .map(|(all, _)| format!("{w} {}", if *all { "all" } else { "any" }))
            })
            .collect::<Vec<_>>()
            .join(" · ");
        nodes.push(Node {
            id: m.name.clone(),
            kind,
            title: m.name.clone(),
            sub,
            active: m.active,
            col: depth[&m.name],
            x: 0.0,
            y: 0.0,
            w: 184.0,
        });
    }
    for l in &labels {
        nodes.push(Node {
            id: l.clone(),
            kind: NodeKind::Label,
            title: l.clone(),
            sub: "label".into(),
            active: true,
            col: depth[l],
            x: 0.0,
            y: 0.0,
            w: 140.0,
        });
    }
    let mut tested: Vec<String> = vec![];
    for m in &r.modules {
        for l in m.links.iter().filter(|l| !l.module) {
            if !tested.contains(&l.target) {
                tested.push(l.target.clone());
            }
        }
    }
    for a in &tested {
        let attr = r.attribute(a);
        nodes.push(Node {
            id: a.clone(),
            kind: NodeKind::Attribute,
            title: tail(a).to_string(),
            sub: attr
                .map(|x| {
                    let out = x.states.iter().filter(|s| **s == State::Outside).count();
                    format!("{} · {out}/{} out", x.kind.label(), x.states.len())
                })
                .unwrap_or_default(),
            active: true,
            col: attr_col,
            x: 0.0,
            y: 0.0,
            w: 200.0,
        });
    }
    // Columns in order; each sorted by where its parents are.
    let cols = attr_col + 1;
    let mut placed: HashMap<String, f64> = HashMap::new();
    let mut tallest = 0usize;
    let mut by_col: Vec<Vec<usize>> = vec![vec![]; cols];
    for (i, n) in nodes.iter().enumerate() {
        by_col[n.col].push(i);
    }
    for col in &mut by_col {
        let mean = |i: &usize| -> f64 {
            let id = &nodes[*i].id;
            let ys: Vec<f64> = edges
                .iter()
                .filter(|e| e.to == *id)
                .filter_map(|e| placed.get(&e.from).copied())
                .collect();
            if ys.is_empty() {
                f64::MAX
            } else {
                ys.iter().sum::<f64>() / ys.len() as f64
            }
        };
        let mut keyed: Vec<(f64, usize, usize)> = col
            .iter()
            .enumerate()
            .map(|(k, i)| (mean(i), k, *i))
            .collect();
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        *col = keyed.into_iter().map(|(_, _, i)| i).collect();
        for (row, i) in col.iter().enumerate() {
            placed.insert(nodes[*i].id.clone(), row as f64);
        }
        tallest = tallest.max(col.len());
    }
    let height = HEAD + tallest.max(1) as f64 * ROW + 20.0;
    let mut headings = vec![];
    for (c, col) in by_col.iter().enumerate() {
        let offset = (tallest - col.len()) as f64 * ROW / 2.0;
        for (row, i) in col.iter().enumerate() {
            nodes[*i].x = 10.0 + c as f64 * COL;
            nodes[*i].y = HEAD + 10.0 + offset + row as f64 * ROW;
        }
        let kinds = || col.iter().map(|i| nodes[*i].kind);
        let heading = if c == attr_col {
            "ATTRIBUTES TESTED"
        } else if kinds().all(|k| k == NodeKind::Root) {
            "ODD ROOT"
        } else if kinds().all(|k| k == NodeKind::Label) {
            "LABELS"
        } else {
            "MODULES"
        };
        headings.push((10.0 + c as f64 * COL, heading));
    }
    let width = 10.0 + (cols.max(1) - 1) as f64 * COL + 220.0;
    Layout {
        nodes,
        edges,
        headings,
        width,
        height,
    }
}

/// What an arrow says, for its tooltip: the lines behind it, in words.
fn edge_lines(doc: &Document, e: &Edge) -> (String, Vec<String>) {
    if e.class == "carries" {
        return (
            format!("{} carries the label {}", e.to, e.from),
            vec![format!(
                "{} holds when any module labelled {} holds",
                e.from, e.from
            )],
        );
    }
    let include = e.class == "include";
    let lines = doc
        .lines_naming(&e.from, &e.to)
        .into_iter()
        .filter(|(inc, _)| *inc == include)
        .map(|(_, l)| l)
        .collect();
    let what = if include {
        "include: must hold"
    } else {
        "exclude: outside when it holds"
    };
    (format!("{} → {} · {what}", e.from, e.to), lines)
}

/// Roots, the modules and labels they read, and the attributes tested, as a
/// graph from left to right: include arrows blue, exclude arrows orange.
#[component]
pub fn ModuleGraph(on_select: Callback<(String, bool)>) -> impl IntoView {
    let ctx = crate::app::ctx();
    let hovered = RwSignal::new(None::<String>);
    // The arrow under the pointer, and apart from it where the pointer is,
    // so that moving along an arrow only moves its tooltip.
    let hot = RwSignal::new(None::<usize>);
    let tip_at = RwSignal::new((0.0, 0.0));
    let wrap = NodeRef::<leptos::html::Div>::new();
    let graph = Memo::new(move |_| ctx.report.with(|r| r.as_ref().ok().map(layout)));
    // The hovered node and every node an arrow joins it to.
    let lit = Memo::new(move |_| {
        let h = hovered.get()?;
        graph.with(|g| {
            let g = g.as_ref()?;
            let mut near: std::collections::HashSet<String> = g
                .edges
                .iter()
                .filter_map(|e| {
                    (e.from == h)
                        .then(|| e.to.clone())
                        .or_else(|| (e.to == h).then(|| e.from.clone()))
                })
                .collect();
            near.insert(h);
            Some(near)
        })
    });
    let tip_text = Memo::new(move |_| {
        let i = hot.get()?;
        graph.with(|g| {
            let e = g.as_ref()?.edges.get(i)?;
            let (title, lines) = ctx.doc.with(|d| edge_lines(d, e));
            Some((e.class, title, lines))
        })
    });
    let tip_view = move || {
        let (class, title, lines) = tip_text.get()?;
        let style = move || {
            let (x, y) = tip_at.get();
            format!("left: {}px; top: {}px", x + 14.0, y + 14.0)
        };
        Some(view! {
            <div class=format!("edge-tip {class}") style=style>
                <div class="tip-title">{title}</div>
                {if lines.is_empty() {
                    view! { <div class="hint">"(see the module)"</div> }.into_any()
                } else {
                    lines.into_iter().map(|l| view! { <div class="mono">{l}</div> }).collect_view().into_any()
                }}
            </div>
        })
    };
    let body = move || {
        let Some(g) = graph.get() else {
            return view! { <p class="hint">"The graph appears once openodd reads the document."</p> }.into_any();
        };
        if g.nodes.is_empty() {
            return view! { <p class="hint">"No modules yet: add one in the outline."</p> }
                .into_any();
        }
        let pos: HashMap<String, (f64, f64, f64)> = g
            .nodes
            .iter()
            .map(|n| (n.id.clone(), (n.x, n.y, n.w)))
            .collect();
        let edge_views = g
            .edges
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let (x1, y1, w1) = *pos.get(&e.from)?;
                let (x2, y2, _) = *pos.get(&e.to)?;
                let (sx, sy) = (x1 + w1, y1 + NODE_H / 2.0);
                let (tx, ty) = (x2, y2 + NODE_H / 2.0);
                let d = format!(
                    "M {sx} {sy} C {} {sy}, {} {ty}, {tx} {ty}",
                    sx + (tx - sx) * 0.5,
                    tx - (tx - sx) * 0.5
                );
                let (from, to) = (e.from.clone(), e.to.clone());
                let dim = move || {
                    hot.get().is_some_and(|t| t != i)
                        || hovered.get().is_some_and(|h| h != from && h != to)
                };
                let is_hot = move || hot.get() == Some(i);
                let follow = move |ev: leptos::ev::MouseEvent| {
                    if let Some(el) = wrap.get() {
                        let rect = el.get_bounding_client_rect();
                        // Left of the pointer when it would run past the right edge.
                        let x = ev.client_x() as f64 - rect.left();
                        let x = if x + 360.0 > rect.width() { x - 370.0 } else { x };
                        tip_at.set((x.max(0.0), ev.client_y() as f64 - rect.top()));
                    }
                    if hot.get_untracked() != Some(i) {
                        hot.set(Some(i));
                    }
                };
                Some(view! {
                    <g class="edge-g">
                        <path
                            class=format!("edge {}", e.class)
                            class:dim=dim
                            class:hot=is_hot
                            d=d.clone()
                            marker-end=format!("url(#arrow-{})", e.class)
                        />
                        <path class="edge-hit" d=d on:mousemove=follow on:mouseleave=move |_| hot.set(None)/>
                    </g>
                })
            })
            .collect_view();
        let headings = g
            .headings
            .iter()
            .map(|(x, h)| view! { <text class="col-head" x=*x y="16">{*h}</text> })
            .collect_view();
        let node_views = g
            .nodes
            .into_iter()
            .map(|n| {
                let id = StoredValue::new(n.id.clone());
                let kind_class = match n.kind {
                    NodeKind::Root => "root",
                    NodeKind::Module => "module",
                    NodeKind::Label => "label",
                    NodeKind::Attribute => "attr",
                };
                let icon = match n.kind {
                    NodeKind::Root => "target",
                    NodeKind::Module => "module",
                    NodeKind::Label => "tag",
                    NodeKind::Attribute => concept_icon(&n.title, KindTag::Number),
                };
                let truth = move || {
                    ctx.evaluation.with(|e| {
                        e.as_ref()
                            .and_then(|e| id.with_value(|id| e.truth(id).map(|t| format!("truth-{t}"))))
                            .unwrap_or_default()
                    })
                };
                let is_attr = n.kind == NodeKind::Attribute;
                view! {
                    <g
                        class=move || format!("node {kind_class} {}", truth())
                        class:inactive=!n.active
                        class:dim=move || lit.with(|l| l.as_ref().is_some_and(|l| id.with_value(|id| !l.contains(id))))
                        transform=format!("translate({} {})", n.x, n.y)
                        on:mouseenter=move |_| hovered.set(Some(id.get_value()))
                        on:mouseleave=move |_| hovered.set(None)
                        on:click=move |_| on_select.run((id.get_value(), is_attr))
                    >
                        <title>{n.id}</title>
                        <rect width=n.w height=NODE_H rx=if is_attr { "19" } else { "9" }/>
                        <g transform="translate(9 9) scale(0.83)" class="node-icon">
                            <Icon name=icon/>
                        </g>
                        <text class="node-title" x="34" y="16">{n.title}</text>
                        <text class="node-sub" x="34" y="30">{n.sub}</text>
                    </g>
                }
            })
            .collect_view();
        let (width, height) = (g.width, g.height);
        view! {
            <svg class="graph" width="100%" style=format!("max-width: {width}px") viewBox=format!("0 0 {width} {height}")>
                <defs>
                    {["include", "exclude", "carries"].into_iter().map(|c| view! {
                        <marker id=format!("arrow-{c}") viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                            <path class=format!("arrowhead {c}") d="M 0 0 L 10 5 L 0 10 z"/>
                        </marker>
                    }).collect_view()}
                </defs>
                {headings}
                {edge_views}
                {node_views}
            </svg>
        }
        .into_any()
    };
    view! {
        <div class="graph-scroll" node_ref=wrap>
            {body}
            {tip_view}
        </div>
    }
}

/// The roots in words: what puts a situation inside the ODD, each module
/// named followed into its own sections.
#[component]
pub fn OddSentence() -> impl IntoView {
    let ctx = crate::app::ctx();
    let roots = Memo::new(move |_| {
        ctx.doc.with(|d| {
            let mut roots: Vec<String> = d
                .modules
                .iter()
                .filter(|m| m.root)
                .map(|m| m.name.clone())
                .collect();
            // No ODD section: every module bounds the ODD.
            if roots.is_empty() {
                roots = d.modules.iter().map(|m| m.name.clone()).collect();
            }
            roots
                .into_iter()
                .map(|r| {
                    let rules = d.explain(&r);
                    (r, rules)
                })
                .collect::<Vec<_>>()
        })
    });
    move || {
        roots
            .get()
            .into_iter()
            .map(|(root, rules)| {
                let r2 = root.clone();
                view! {
                    <div class="sentence">
                        <div class="sentence-head">
                            <Icon name="target"/>
                            "A situation is inside "
                            <button class="pill this" on:click=move |_| ctx.select_module(&r2)>{root}</button>
                        </div>
                        {rules_view(rules, true)}
                    </div>
                }
            })
            .collect_view()
    }
}

/// Rules as nested lists; `top` words them for the ODD itself.
fn rules_view(rules: Vec<Rule>, top: bool) -> AnyView {
    let ctx = crate::app::ctx();
    rules
        .into_iter()
        .map(|rule| {
            let n = if rule.all { "ALL" } else { "ANY" };
            let (class, head) = match (rule.kind, top) {
                (RuleKind::Include, true) => ("include", format!("when {n} of these hold:")),
                (RuleKind::Exclude, true) => ("exclude", format!("but it is outside when {n} of these hold:")),
                (RuleKind::Include, false) => ("include", format!("which holds when {n} of these hold:")),
                (RuleKind::Exclude, false) => ("exclude", format!("unless {n} of these hold:")),
                (RuleKind::Group, _) => ("group", format!("{n} of:")),
            };
            view! {
                <div class=format!("rule {class}")>
                    <div class="rule-head">{head}</div>
                    <ul>
                        {rule.clauses.into_iter().map(|c| {
                            let module = c.module.clone();
                            let text = (!c.text.is_empty()).then(|| match module {
                                Some(m) => {
                                    let rest = c.text.strip_prefix(m.as_str()).unwrap_or("").to_string();
                                    let m2 = m.clone();
                                    view! {
                                        <button class="pill" on:click=move |_| ctx.select_module(&m2)><Icon name="module"/>{m}</button>
                                        <span>{rest}</span>
                                    }.into_any()
                                }
                                None => view! { <span class="mono">{c.text.clone()}</span> }.into_any(),
                            });
                            let rules = (!c.rules.is_empty()).then(|| rules_view(c.rules, false));
                            view! { <li>{text}{rules}</li> }
                        }).collect_view()}
                    </ul>
                </div>
            }
        })
        .collect_view()
        .into_any()
}

// ---------------------------------------------------------------------------
// The ODD over two attributes
// ---------------------------------------------------------------------------

/// A value inside bucket `i` of an attribute: what a click on a cell tests.
pub fn representative(a: &Attribute, i: usize) -> String {
    if !a.is_number() {
        return a.values.get(i).cloned().unwrap_or_default();
    }
    let (Some(lo), Some(hi)) = (a.edges.get(i), a.edges.get(i + 1)) else {
        return String::new();
    };
    let s = Scale::around(&a.edges, &[], a.integer);
    let v = match (lo.is_finite(), hi.is_finite()) {
        (true, true) => (lo + hi) / 2.0,
        (false, true) => (s.lo.min(*hi) + hi) / 2.0,
        (true, false) => (lo + s.hi.max(*lo)) / 2.0,
        _ => (s.lo + s.hi) / 2.0,
    };
    let mut v = round_nice(v, s.step());
    if a.integer {
        v = v.floor();
        // A whole number inside [lo, hi): the lowest one.
        if v < *lo {
            v = lo.ceil();
        }
        if v >= *hi && hi.is_finite() && i + 2 < a.edges.len() {
            v = hi.ceil() - 1.0;
        }
    }
    fmt_num(v)
}

/// The bucket a test value (as typed) falls in, as openodd buckets it.
fn bucket_index(r: &Report, a: &Attribute, v: &str) -> Option<usize> {
    if !a.is_number() {
        return a.values.iter().position(|x| x == v);
    }
    let item = r.odd.0.attribute(&a.name)?.item.as_ref()?;
    let label = item
        .bucket_of(&openodd::Value::Float(parse_float(v)?))
        .ok()?;
    a.buckets.iter().position(|b| *b == label)
}

#[component]
pub fn OddMap() -> impl IntoView {
    let ctx = crate::app::ctx();
    let names = Memo::new(move |_| {
        ctx.report.with(|r| {
            r.as_ref()
                .map(|r| {
                    r.attributes
                        .iter()
                        .filter(|a| !a.buckets.is_empty())
                        .map(|a| a.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    // The two attributes the ODD restricts most, by default.
    let pick_default = move || -> (String, String) {
        ctx.report.with(|r| {
            let Ok(r) = r else {
                return (String::new(), String::new());
            };
            let mut ranked: Vec<(&Attribute, usize)> = r
                .attributes
                .iter()
                .filter(|a| !a.buckets.is_empty())
                .map(|a| (a, a.states.iter().filter(|s| **s == State::Outside).count()))
                .collect();
            ranked.sort_by_key(|a| std::cmp::Reverse(a.1));
            let name = |i: usize| ranked.get(i).map(|a| a.0.name.clone()).unwrap_or_default();
            (name(0), name(1))
        })
    };
    let x = RwSignal::new(String::new());
    let y = RwSignal::new(String::new());
    Effect::new(move |_| {
        let ns = names.get();
        if !ns.contains(&x.get_untracked()) || !ns.contains(&y.get_untracked()) {
            let (a, b) = pick_default();
            x.set(a);
            y.set(b);
        }
    });
    let grid = Memo::new(move |_| {
        ctx.report.with(|r| {
            let r = r.as_ref().ok()?;
            let (xn, yn) = (x.get(), y.get());
            let ax = r.attribute(&xn)?.clone();
            let ay = r.attribute(&yn)?.clone();
            Some((ax, ay, crate::check::matrix(r, &xn, &yn)))
        })
    });
    // The cell the test values fall in.
    let hit = Memo::new(move |_| {
        grid.with(|g| {
            let (ax, ay, _) = g.as_ref()?;
            ctx.report.with(|r| {
                let r = r.as_ref().ok()?;
                let at = |a: &Attribute| {
                    ctx.probe_value(&a.name)
                        .and_then(|v| bucket_index(r, a, &v))
                };
                Some((at(ax)?, at(ay)?))
            })
        })
    });
    let picker = move |which: RwSignal<String>, label: &'static str| {
        view! {
            <label class="axis-pick">
                <span>{label}</span>
                <select prop:value=move || which.get() on:change=move |ev| which.set(event_target_value(&ev))>
                    {move || names.get().into_iter().map(|n| view! { <option value=n.clone()>{n.clone()}</option> }).collect_view()}
                </select>
            </label>
        }
    };
    view! {
        <div class="map-controls">
            {picker(x, "x")}
            <button class="mini" title="Swap the axes" on:click=move |_| {
                let a = x.get();
                x.set(y.get());
                y.set(a);
            }>"⇄"</button>
            {picker(y, "y")}
        </div>
        {move || {
            let Some((ax, ay, rows)) = grid.get() else {
                return view! { <p class="hint">"Two attributes with buckets are needed."</p> }.into_any();
            };
            let cols = ax.buckets.len();
            let rep_x: Vec<String> = (0..cols).map(|j| representative(&ax, j)).collect();
            view! {
                <div class="odd-map" style=format!("grid-template-columns: minmax(70px, auto) repeat({cols}, minmax(34px, 1fr))")>
                    <div class="corner">
                        <span class="y">{tail(&ay.name).to_string()}" ↓"</span>
                        <span class="x">{tail(&ax.name).to_string()}" →"</span>
                    </div>
                    {ax.buckets.iter().map(|b| view! { <div class="col-label" title=b.clone()>{b.clone()}</div> }).collect_view()}
                    {rows.into_iter().enumerate().rev().map(|(i, row)| {
                        let ylab = ay.buckets.get(i).cloned().unwrap_or_default();
                        let rep_y = representative(&ay, i);
                        view! {
                            <div class="row-label" title=ylab.clone()>{ylab.clone()}</div>
                            {row.into_iter().enumerate().map(|(j, s)| {
                                let (xname, yname) = (ax.name.clone(), ay.name.clone());
                                let (rx, ry) = (rep_x[j].clone(), rep_y.clone());
                                let tip = format!("{} × {} — {}", ax.buckets[j], ylab, state_words(s));
                                let here = move || hit.get() == Some((j, i));
                                view! {
                                    <button
                                        class=format!("cell {}", s.class())
                                        class:hit=here
                                        title=tip
                                        on:click=move |_| {
                                            ctx.set_probe(&xname, rx.clone());
                                            ctx.set_probe(&yname, ry.clone());
                                        }
                                    >{move || if here() { "●" } else { "" }}</button>
                                }
                            }).collect_view()}
                        }
                    }).collect_view()}
                </div>
            }
            .into_any()
        }}
    }
}
