//! What the editor draws: an attribute's domain coloured by where the ODD
//! stands in each bucket (a number line one can drag, a strip of literals),
//! the graph of modules and what they test, and a map of the ODD over two
//! attributes.

use std::collections::HashMap;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::check::{Attribute, Evaluation, Report, State};
use crate::doc::KindTag;
use crate::icons::{concept_icon, Icon};

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
    /// Around the finite edges of an attribute and any other values to show.
    pub fn around(edges: &[f64], extra: &[f64]) -> Scale {
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
            integer: false,
        }
    }

    pub fn integral(self, integer: bool) -> Scale {
        Scale { integer, ..self }
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
    #[prop(optional)]
    module: Option<Signal<String>>,
    /// A condition's interval, its ends possibly infinite.
    #[prop(optional)]
    interval: Option<Signal<Option<(f64, f64)>>>,
    /// Draggable values (a condition's finite ends, a test value).
    #[prop(optional)]
    handles: Option<Signal<Vec<f64>>>,
    #[prop(optional)] on_drag: Option<Callback<(usize, f64)>>,
    /// A click on the line, at a value.
    #[prop(optional)]
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
        let edges = attr.with(|a| a.as_ref().map(|a| a.edges.clone()).unwrap_or_default());
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
        let integer = attr.with(|a| a.as_ref().is_some_and(|a| a.integer));
        Scale::around(&edges, &extra).integral(integer)
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
    // Where the ODD (or the module) stands along the line: openodd's
    // verdict at sampled values, the edges and either side of them included,
    // drawn as runs.
    let ctx = crate::app::ctx();
    let runs = Memo::new(move |_| {
        let s = scale.get();
        let Some(a) = attr.get() else {
            return vec![];
        };
        let Ok(report) = ctx.report.get() else {
            return vec![];
        };
        let n = 160;
        let mut xs: Vec<f64> = (0..=n)
            .map(|i| s.lo + (s.hi - s.lo) * i as f64 / n as f64)
            .collect();
        let eps = (s.hi - s.lo) * 1e-6;
        for e in a.edges.iter().filter(|e| e.is_finite()) {
            xs.extend([e - eps, *e, e + eps]);
        }
        if a.integer {
            xs = xs.into_iter().map(f64::round).collect();
        }
        xs.retain(|x| *x >= s.lo && *x <= s.hi);
        xs.sort_by(f64::total_cmp);
        xs.dedup();
        let m = module.map(|m| m.get());
        let states = crate::check::profile(&report, &a.name, m.as_deref(), &xs);
        // Runs of one state, split halfway between unlike neighbours (at the
        // value itself for an integer).
        let mut out: Vec<(f64, f64, State)> = vec![];
        for (i, (x, st)) in xs.iter().zip(&states).enumerate() {
            let start = if i == 0 {
                s.lo
            } else if a.integer {
                x - 0.5
            } else {
                (xs[i - 1] + x) / 2.0
            };
            match out.last_mut() {
                Some(last) if last.2 == *st => {}
                Some(last) => {
                    last.1 = start;
                    out.push((start, s.hi, *st));
                }
                None => out.push((s.lo, s.hi, *st)),
            }
        }
        out
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
    #[prop(optional)] states: Option<Signal<Vec<State>>>,
    #[prop(optional)] marker: Option<Signal<Option<String>>>,
    #[prop(optional)] on_pick: Option<Callback<String>>,
) -> impl IntoView {
    move || {
        attr.with(|a| {
            let Some(a) = a else { return ().into_any() };
            let st = states.map(|s| s.get()).unwrap_or_else(|| a.states.clone());
            let current = marker.and_then(|m| m.get());
            a.values
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let shown = match v.as_str() {
                        "True" => "true".to_string(),
                        "False" => "false".to_string(),
                        other => other.to_string(),
                    };
                    let state = st.get(i).copied().unwrap_or(State::Inside);
                    let on = current.as_deref() == Some(shown.as_str());
                    let pick = shown.clone();
                    view! {
                        <button
                            class=format!("cat {}", state.class())
                            class:picked=on
                            class:static=on_pick.is_none()
                            title=state_words(state)
                            on:click=move |_| if let Some(cb) = on_pick { cb.run(pick.clone()) }
                        >
                            <span class="dot"></span>
                            {shown}
                        </button>
                    }
                })
                .collect_view()
                .into_any()
        })
    }
}

/// The domain of an attribute, drawn as fits it, with the test value on it.
/// With `module`, what that module accepts instead of the ODD.
#[component]
pub fn Domain(
    #[prop(into)] attr: Signal<Option<Attribute>>,
    #[prop(optional)] module: Option<Signal<String>>,
    #[prop(optional)] compact: bool,
) -> impl IntoView {
    let ctx = crate::app::ctx();
    let name = move || attr.with(|a| a.as_ref().map(|a| a.name.clone()).unwrap_or_default());
    let probe = move || ctx.probe_value(&name());
    let number = Memo::new(move |_| attr.with(|a| a.as_ref().is_some_and(Attribute::is_number)));
    let set = move |v: String| ctx.set_probe(&name(), v);
    let states = module.map(|m| {
        Signal::derive(move || {
            let n = name();
            let m = m.get();
            ctx.report
                .get()
                .ok()
                .map(|r| crate::check::module_states(&r, &m, &n))
                .unwrap_or_default()
        })
    });
    move || {
        if number.get() {
            let marker = Signal::derive(move || probe().and_then(|t| t.parse::<f64>().ok()));
            let handles = Signal::derive(move || marker.get().into_iter().collect::<Vec<_>>());
            let drag = Callback::new(move |(_, v): (usize, f64)| set(fmt_num(v)));
            let pick = Callback::new(move |v: f64| set(fmt_num(v)));
            match module {
                Some(m) => view! {
                    <NumberLine attr=attr module=m marker=marker compact=compact/>
                }
                .into_any(),
                None => view! {
                    <NumberLine attr=attr marker=marker handles=handles on_drag=drag on_pick=pick compact=compact/>
                }
                .into_any(),
            }
        } else {
            let marker = Signal::derive(probe);
            let pick = Callback::new(move |v: String| {
                let same = probe().as_deref() == Some(v.as_str());
                set(if same { String::new() } else { v });
            });
            match states {
                Some(states) => view! {
                    <div class="cats"><CategoryStrip attr=attr states=states marker=marker/></div>
                }
                .into_any(),
                None => view! {
                    <div class="cats"><CategoryStrip attr=attr marker=marker on_pick=pick/></div>
                }
                .into_any(),
            }
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

const NODE_H: f64 = 38.0;
const ROW: f64 = 50.0;
const COL: f64 = 225.0;

fn layout(r: &Report) -> (Vec<Node>, Vec<Edge>, f64, f64) {
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
        let sub = match (&m.include, &m.exclude) {
            (Some((a, _)), Some((b, _))) => format!(
                "include {} · exclude {}",
                if *a { "all" } else { "any" },
                if *b { "all" } else { "any" }
            ),
            (Some((a, _)), None) => format!("include {}", if *a { "all" } else { "any" }),
            (None, Some((b, _))) => format!("exclude {}", if *b { "all" } else { "any" }),
            _ => String::new(),
        };
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
            title: a.rsplit('.').next().unwrap_or(a).to_string(),
            sub: attr
                .map(|x| {
                    let out = x.states.iter().filter(|s| **s == State::Outside).count();
                    format!("{} · {out}/{} out", x.kind, x.states.len())
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
    let height = tallest.max(1) as f64 * ROW + 20.0;
    for (c, col) in by_col.iter().enumerate() {
        let offset = (tallest - col.len()) as f64 * ROW / 2.0;
        for (row, i) in col.iter().enumerate() {
            nodes[*i].x = 10.0 + c as f64 * COL;
            nodes[*i].y = 10.0 + offset + row as f64 * ROW;
        }
    }
    let width = 10.0 + (cols.max(1) - 1) as f64 * COL + 220.0;
    (nodes, edges, width, height)
}

/// Roots, the modules and labels they read, and the attributes tested, as a
/// graph from left to right: include arrows blue, exclude arrows orange.
#[component]
pub fn ModuleGraph(
    #[prop(into)] report: Signal<Option<Report>>,
    #[prop(into)] evaluation: Signal<Option<Evaluation>>,
    #[prop(into)] selected: Signal<Option<String>>,
    on_select: Callback<(String, bool)>,
) -> impl IntoView {
    let hovered = RwSignal::new(None::<String>);
    let graph = Memo::new(move |_| report.with(|r| r.as_ref().map(layout)));
    move || {
        let Some((nodes, edges, width, height)) = graph.get() else {
            return view! { <p class="hint">"The graph appears once openodd reads the document."</p> }.into_any();
        };
        if nodes.is_empty() {
            return view! { <p class="hint">"No modules yet: add one in the outline."</p> }
                .into_any();
        }
        let pos: HashMap<String, (f64, f64, f64)> = nodes
            .iter()
            .map(|n| (n.id.clone(), (n.x, n.y, n.w)))
            .collect();
        let edge_views = edges
            .iter()
            .filter_map(|e| {
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
                let dim = move || hovered.get().is_some_and(|h| h != from && h != to);
                Some(view! {
                    <path
                        class=format!("edge {}", e.class)
                        class:dim=dim
                        d=d
                        marker-end=format!("url(#arrow-{})", e.class)
                    />
                })
            })
            .collect_view();
        let node_views = nodes
            .into_iter()
            .map(|n| {
                let id = n.id.clone();
                let id2 = n.id.clone();
                let id3 = n.id.clone();
                let id4 = n.id.clone();
                let id5 = n.id.clone();
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
                    evaluation.with(|e| {
                        e.as_ref()
                            .and_then(|e| e.truth(&id5).map(|t| format!("truth-{t}")))
                            .unwrap_or_default()
                    })
                };
                let is_attr = n.kind == NodeKind::Attribute;
                let title = n.title.clone();
                let full = n.id.clone();
                view! {
                    <g
                        class=move || format!("node {kind_class} {}", truth())
                        class:inactive=!n.active
                        class:selected=move || selected.get().as_deref() == Some(id.as_str())
                        class:dim=move || hovered.get().is_some_and(|h| h != id2 && !edges_touch(&graph.get(), &h, &id2))
                        transform=format!("translate({} {})", n.x, n.y)
                        on:mouseenter=move |_| hovered.set(Some(id3.clone()))
                        on:mouseleave=move |_| hovered.set(None)
                        on:click=move |_| on_select.run((id4.clone(), is_attr))
                    >
                        <title>{full}</title>
                        <rect width=n.w height=NODE_H rx=if is_attr { "19" } else { "9" }/>
                        <g transform="translate(9 9) scale(0.83)" class="node-icon">
                            <Icon name=icon/>
                        </g>
                        <text class="node-title" x="34" y="16">{title}</text>
                        <text class="node-sub" x="34" y="30">{n.sub.clone()}</text>
                    </g>
                }
            })
            .collect_view();
        view! {
            <div class="graph-scroll">
                <svg class="graph" width="100%" style=format!("max-width: {width}px") viewBox=format!("0 0 {width} {height}")>
                    <defs>
                        {["include", "exclude", "carries"].into_iter().map(|c| view! {
                            <marker id=format!("arrow-{c}") viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                                <path class=format!("arrowhead {c}") d="M 0 0 L 10 5 L 0 10 z"/>
                            </marker>
                        }).collect_view()}
                    </defs>
                    {edge_views}
                    {node_views}
                </svg>
            </div>
        }
        .into_any()
    }
}

fn edges_touch(graph: &Option<(Vec<Node>, Vec<Edge>, f64, f64)>, a: &str, b: &str) -> bool {
    graph.as_ref().is_some_and(|(_, edges, _, _)| {
        edges
            .iter()
            .any(|e| (e.from == a && e.to == b) || (e.from == b && e.to == a))
    })
}

// ---------------------------------------------------------------------------
// The ODD over two attributes
// ---------------------------------------------------------------------------

/// A value inside bucket `i` of an attribute: what a click on a cell tests.
pub fn representative(a: &Attribute, i: usize) -> String {
    if a.is_number() {
        let (Some(lo), Some(hi)) = (a.edges.get(i), a.edges.get(i + 1)) else {
            return String::new();
        };
        let s = Scale::around(&a.edges, &[]).integral(a.integer);
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
    } else {
        match a.values.get(i).map(String::as_str) {
            Some("True") => "true".into(),
            Some("False") => "false".into(),
            Some(v) => v.into(),
            None => String::new(),
        }
    }
}

#[component]
pub fn OddMap(#[prop(into)] report: Signal<Option<Report>>) -> impl IntoView {
    let ctx = crate::app::ctx();
    let names = Memo::new(move |_| {
        report.with(|r| {
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
        report.with(|r| {
            let Some(r) = r else {
                return (String::new(), String::new());
            };
            let mut ranked: Vec<(&Attribute, usize)> = r
                .attributes
                .iter()
                .filter(|a| !a.buckets.is_empty())
                .map(|a| (a, a.states.iter().filter(|s| **s != State::Inside).count()))
                .collect();
            ranked.sort_by_key(|a| std::cmp::Reverse(a.1));
            let first = ranked.first().map(|a| a.0.name.clone()).unwrap_or_default();
            let second = ranked.get(1).map(|a| a.0.name.clone()).unwrap_or_default();
            (first, second)
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
        report.with(|r| {
            let r = r.as_ref()?;
            let (xn, yn) = (x.get(), y.get());
            let ax = r.attribute(&xn)?.clone();
            let ay = r.attribute(&yn)?.clone();
            Some((ax, ay, crate::check::matrix(r, &xn, &yn)))
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
            let px = ctx.probe_value(&ax.name);
            let py = ctx.probe_value(&ay.name);
            let cols = ax.buckets.len();
            let ax2 = ax.clone();
            let ay2 = ay.clone();
            view! {
                <div class="odd-map" style=format!("grid-template-columns: minmax(70px, auto) repeat({cols}, minmax(34px, 1fr))")>
                    <div class="corner">
                        <span class="y">{short(&ay.name)}" ↓"</span>
                        <span class="x">{short(&ax.name)}" →"</span>
                    </div>
                    {ax.buckets.iter().map(|b| view! { <div class="col-label" title=b.clone()>{b.clone()}</div> }).collect_view()}
                    {rows.into_iter().enumerate().rev().map(|(i, row)| {
                        let ylab = ay.buckets.get(i).cloned().unwrap_or_default();
                        let rep_y = representative(&ay2, i);
                        let hit_y = py.as_deref().is_some_and(|v| in_bucket(&ay2, i, v));
                        let ax3 = ax2.clone();
                        let px = px.clone();
                        let yname = ay2.name.clone();
                        view! {
                            <div class="row-label" title=ylab.clone()>{ylab.clone()}</div>
                            {row.into_iter().enumerate().map(|(j, s)| {
                                let rep_x = representative(&ax3, j);
                                let hit = hit_y && px.as_deref().is_some_and(|v| in_bucket(&ax3, j, v));
                                let (xname, yname, ry) = (ax3.name.clone(), yname.clone(), rep_y.clone());
                                let tip = format!("{} × {} — {}", ax3.buckets[j], ylab, state_words(s));
                                view! {
                                    <button
                                        class=format!("cell {}", s.class())
                                        class:hit=hit
                                        title=tip
                                        on:click=move |_| {
                                            ctx.set_probe(&xname, rep_x.clone());
                                            ctx.set_probe(&yname, ry.clone());
                                        }
                                    >{if hit { "●" } else { "" }}</button>
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

fn short(name: &str) -> String {
    name.rsplit('.').next().unwrap_or(name).to_string()
}

/// Whether a test value (as typed) falls in bucket `i`.
pub fn in_bucket(a: &Attribute, i: usize, v: &str) -> bool {
    if a.is_number() {
        let Ok(x) = v.parse::<f64>() else {
            return false;
        };
        let (Some(lo), Some(hi)) = (a.edges.get(i), a.edges.get(i + 1)) else {
            return false;
        };
        let last = i + 2 == a.edges.len();
        x >= *lo && (x < *hi || (last && x <= *hi))
    } else {
        representative(a, i) == v
    }
}
