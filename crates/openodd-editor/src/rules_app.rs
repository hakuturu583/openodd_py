//! The rules editor: a backend's fuzzy rules ([docs/rules.md](../../../docs/rules.md))
//! edited as forms. The outline lists the quantities and their rule bases; a
//! rule base's form holds its inputs, each with its terms drawn and their
//! corners draggable, and its rules as a table of term pickers and outputs;
//! the panel tests the rules at chosen inputs and draws a quantity's
//! response. Every edit is read back by openodd's own reader.

use indexmap::IndexMap;
use leptos::prelude::*;
use leptos::task::spawn_local;
use openodd::rules::{Membership, RuleSet};
use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;

use crate::app::{event_target, unique, ModeSwitch};
use crate::doc::fresh_id;
use crate::icons::Icon;
use crate::io;
use crate::rules_doc::{
    self, clauses, num, tidy, when_of, BaseDoc, InputDoc, QuantityDoc, RuleDoc, RulesDoc, Shape,
    Term, SHAPES,
};

pub const SAMPLE: &str = include_str!("../../../examples/wasm-viewer/sample_rules.yaml");

/// What the middle shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RSel {
    File,
    Quantity(u64),
    Base(u64, u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Test,
    Yaml,
    Details,
}

/// The rules editor's state, shared by every component.
#[derive(Clone, Copy)]
pub struct RCtx {
    pub doc: RwSignal<RulesDoc>,
    pub name: RwSignal<String>,
    /// Files the document extends, by name.
    pub library: RwSignal<Vec<(String, String)>>,
    pub sel: RwSignal<RSel>,
    pub yaml: Memo<Result<String, String>>,
    /// openodd's reading of the document, or why it does not read.
    pub checked: Memo<Result<RuleSet, String>>,
    /// The last reading that succeeded: the views keep drawing it while an
    /// edit is half done.
    pub good: RwSignal<Option<RuleSet>>,
    /// The values being tested, by input.
    pub probe: RwSignal<Vec<(String, f64)>>,
    /// The quantity the panel tests, and the input its response moves.
    pub tested: RwSignal<Option<String>>,
    pub x: RwSignal<Option<String>>,
    pub notice: RwSignal<Option<(bool, String)>>,
    /// Name the inputs by the ODD attributes that set them, where the
    /// binding and the backend's manifest say which.
    pub odd_names: RwSignal<bool>,
}

pub fn rctx() -> RCtx {
    expect_context::<RCtx>()
}

impl RCtx {
    /// Replace the rules with those read from `text`.
    pub fn open_text(&self, name: &str, text: &str) -> bool {
        self.open(name, text)
    }

    fn open(&self, name: &str, text: &str) -> bool {
        match RulesDoc::from_yaml(text) {
            Ok(doc) => {
                let first = doc.quantities.first().map(|q| q.name.clone());
                self.doc.set(doc);
                self.name.set(name.to_string());
                self.sel.set(RSel::File);
                self.probe.set(vec![]);
                self.tested.set(first);
                self.x.set(None);
                self.notice.set(Some((true, format!("Opened {name}"))));
                true
            }
            Err(e) => {
                self.notice.set(Some((false, format!("{name}: {e}"))));
                false
            }
        }
    }

    fn base_update(&self, q: u64, b: u64, f: impl FnOnce(&mut BaseDoc)) {
        self.doc.update(|d| {
            if let Some(x) = d.base_mut(q, b) {
                f(x)
            }
        })
    }

    fn input_update(&self, q: u64, b: u64, i: u64, f: impl FnOnce(&mut InputDoc)) {
        self.base_update(q, b, |base| {
            if let Some(input) = base.inputs.iter_mut().flatten().find(|x| x.id == i) {
                f(input)
            }
        })
    }

    fn rule_update(&self, q: u64, b: u64, r: u64, f: impl FnOnce(&mut RuleDoc)) {
        self.base_update(q, b, |base| {
            if let Some(rule) = base.rules.iter_mut().flatten().find(|x| x.id == r) {
                f(rule)
            }
        })
    }

    fn probe_value(&self, name: &str) -> Option<f64> {
        self.probe
            .with(|p| p.iter().find(|(n, _)| n == name).map(|(_, v)| *v))
    }

    fn set_probe(&self, name: &str, value: f64) {
        self.probe.update(|p| {
            if let Some(e) = p.iter_mut().find(|(n, _)| n == name) {
                e.1 = value;
            } else {
                p.push((name.to_string(), value));
            }
        })
    }

    /// The names of quantity `q` and rule base `b`.
    fn names(&self, q: u64, b: u64) -> Option<(String, String)> {
        self.doc.with(|d| {
            let qd = d.quantity(q)?;
            let bd = qd.bases.iter().find(|x| x.id == b)?;
            Some((qd.name.clone(), bd.name.clone()))
        })
    }

    /// Rule base `b` of `q` as openodd read it last: its inputs and rules,
    /// the extended files' included.
    fn read_base(&self, q: u64, b: u64) -> Option<openodd::rules::RuleBase> {
        let (qn, bn) = self.names(q, b)?;
        self.good.with(|g| {
            g.as_ref()?
                .quantities
                .get(&qn)?
                .rule_bases
                .iter()
                .find(|x| x.name == bn)
                .cloned()
        })
    }

    /// How strongly each rule of `b` fires at the tested values.
    fn firing(&self, q: u64, b: u64) -> Option<Vec<f64>> {
        let (qn, bn) = self.names(q, b)?;
        let probe = self.probe.get();
        self.good.with(|g| {
            let set = g.as_ref()?;
            let quantity = set.quantities.get(&qn)?;
            let base = quantity.rule_bases.iter().find(|x| x.name == bn)?;
            let values = quantity
                .values_of(base, &rules_doc::values_at(set, &qn, &probe))
                .ok()?;
            Some(base.firing(&values))
        })
    }
}

/// How an input is named: by the ODD attribute that sets it, when asked and
/// known, else as the rules name it.
fn shown(name: &str) -> String {
    let r = rctx();
    if r.odd_names.get() {
        if let Some(a) = crate::binding_app::bctx().attribute_of_input(name) {
            return a;
        }
    }
    name.to_string()
}

/// The other name of an input: the rules' when the ODD's is shown, and back.
fn other_name(name: &str) -> Option<String> {
    let a = crate::binding_app::bctx().attribute_of_input(name)?;
    Some(if rctx().odd_names.get() {
        name.to_string()
    } else {
        a
    })
}

fn parse_num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

pub fn shape_of(m: &Membership) -> Shape {
    match *m {
        Membership::Triangle(p) => Shape::Triangle(p),
        Membership::Trapezoid(p) => Shape::Trapezoid(p),
        Membership::Gaussian { mean, sigma } => Shape::Gaussian { mean, sigma },
    }
}

/// An input as openodd read it, as the form holds one.
fn input_doc(i: &openodd::rules::Input) -> InputDoc {
    InputDoc {
        id: fresh_id(),
        name: i.name.clone(),
        low: i.low,
        high: i.high,
        default: i.default,
        terms: i
            .terms
            .iter()
            .map(|(n, m)| Term {
                id: fresh_id(),
                name: n.clone(),
                shape: shape_of(m),
            })
            .collect(),
    }
}

/// The rules editor's state, kept while another editor is shown.
pub fn rules_context() -> RCtx {
    let (name, text) =
        io::load_rules_draft().unwrap_or_else(|| ("sample_rules.yaml".into(), SAMPLE.into()));
    let doc = RulesDoc::from_yaml(&text)
        .or_else(|_| RulesDoc::from_yaml(SAMPLE))
        .unwrap_or_default();
    let first = doc.quantities.first().map(|q| q.name.clone());
    let doc = RwSignal::new(doc);
    let name = RwSignal::new(name);
    let library = RwSignal::new(Vec::<(String, String)>::new());
    let yaml = Memo::new(move |_| doc.with(RulesDoc::to_yaml));
    let checked = Memo::new(move |_| {
        let text = yaml.get()?;
        library.with(|l| rules_doc::check(&text, &name.get(), l))
    });
    let good = RwSignal::new(None::<RuleSet>);
    Effect::new(move |_| {
        if let Ok(set) = checked.get() {
            good.set(Some(set));
        }
    });
    let ctx = RCtx {
        doc,
        name,
        library,
        sel: RwSignal::new(RSel::File),
        yaml,
        checked,
        good,
        probe: RwSignal::new(vec![]),
        tested: RwSignal::new(first),
        x: RwSignal::new(None),
        notice: RwSignal::new(None),
        odd_names: RwSignal::new(false),
    };
    // Keep the draft between visits.
    Effect::new(move |_| {
        if let Ok(text) = yaml.get() {
            io::save_rules_draft(&name.get(), &text);
        }
    });
    // Test the quantity being edited.
    Effect::new(move |_| {
        let q = match ctx.sel.get() {
            RSel::Quantity(q) | RSel::Base(q, _) => Some(q),
            RSel::File => None,
        };
        if let Some(n) = q.and_then(|q| doc.with(|d| d.quantity(q).map(|x| x.name.clone()))) {
            ctx.tested.set(Some(n));
        }
    });
    ctx
}

#[component]
pub fn RulesStudio() -> impl IntoView {
    let ctx = rctx();
    view! {
        <div class="app">
            <RulesToolbar/>
            <main class="layout">
                <aside class="outline">
                    <RulesOutline/>
                </aside>
                <section class="center">
                    <ErrorCard/>
                    {move || match ctx.sel.get() {
                        RSel::File => view! { <FileView/> }.into_any(),
                        RSel::Quantity(q) => view! { <QuantityView q=q/> }.into_any(),
                        RSel::Base(q, b) => view! { <BaseView q=q b=b/> }.into_any(),
                    }}
                </section>
                <aside class="panel">
                    <RulesPanel/>
                </aside>
            </main>
        </div>
    }
}

#[component]
fn RulesToolbar() -> impl IntoView {
    let ctx = rctx();
    let open = move |ev: leptos::ev::Event| {
        let input: HtmlInputElement = event_target(&ev);
        spawn_local(async move {
            if let Some((name, text)) = io::read_files(&input).await.into_iter().next() {
                ctx.open(&name, &text);
            }
        });
    };
    let example = move |ev: leptos::ev::Event| {
        match event_target_value(&ev).as_str() {
            "weather" => {
                ctx.open("sample_rules.yaml", SAMPLE);
            }
            "empty" => {
                ctx.open("rules.yaml", "version: 1\nquantities: {}\n");
            }
            _ => {}
        }
        let select: web_sys::HtmlSelectElement = event_target(&ev);
        select.set_value("");
    };
    let save = move |_| match ctx.yaml.get() {
        Ok(text) => io::download(&ctx.name.get(), &text),
        Err(e) => ctx.notice.set(Some((false, e))),
    };
    let status = move || match ctx.checked.get() {
        Ok(_) => view! {
            <span class="badge ok" title="openodd reads these rules">
                <Icon name="check"/>"valid"
            </span>
        }
        .into_any(),
        Err(e) => {
            let title = e.clone();
            view! { <span class="badge bad" title=title><Icon name="warn"/>{e}</span> }.into_any()
        }
    };
    view! {
        <header class="toolbar">
            <div class="brand">
                <span class="logo"><Icon name="map"/></span>
                <span>"OpenODD Studio"</span>
            </div>
            <ModeSwitch/>
            <div class="file">
                <Icon name="doc"/>
                <input
                    class="filename"
                    aria-label="File name"
                    prop:value=move || ctx.name.get()
                    on:change=move |ev| ctx.name.set(event_target_value(&ev))
                />
            </div>
            <label class="button">
                <Icon name="upload"/>"Open"
                <input type="file" accept=".yaml,.yml" class="hidden" on:change=open/>
            </label>
            <select class="button" on:change=example aria-label="Examples">
                <option value="" selected>"Examples…"</option>
                <option value="weather">"Weather: grip and visibility"</option>
                <option value="empty">"Empty rule file"</option>
            </select>
            <button class="button primary" on:click=save><Icon name="download"/>"Download YAML"</button>
            <div class="segmented" title="Name the inputs as the rules do, or by the ODD attributes that set them (from the binding and the backend's manifest)">
                <button class:active=move || !ctx.odd_names.get() on:click=move |_| ctx.odd_names.set(false)>"Backend names"</button>
                <button class:active=move || ctx.odd_names.get()
                    disabled=move || crate::binding_app::bctx().links.with(|ls| ls.iter().all(|l| l.inputs.is_empty()))
                    on:click=move |_| ctx.odd_names.set(true)>"ODD names"</button>
            </div>
            <div class="status">{status}</div>
            <a class="source" href="https://github.com/hakuturu583/openodd_py/blob/main/docs/rules.md" target="_blank" rel="noopener">
                "Rule files"
            </a>
            {move || {
                ctx.notice.get().map(|(ok, text)| {
                    view! {
                        <div class=if ok { "notice ok" } else { "notice bad" } on:click=move |_| ctx.notice.set(None)>
                            {text}
                        </div>
                    }
                })
            }}
        </header>
    }
}

#[component]
fn ErrorCard() -> impl IntoView {
    let ctx = rctx();
    move || {
        ctx.checked.get().err().map(|e| {
            view! {
                <div class="card error-card">
                    <div class="card-title"><Icon name="warn"/>"openodd does not read these rules"</div>
                    <div class="mono">{e}</div>
                    <p class="hint">"The views below show the last rules that read."</p>
                </div>
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Outline
// ---------------------------------------------------------------------------

fn add_quantity() {
    let ctx = rctx();
    let mut sel = None;
    ctx.doc.update(|d| {
        let taken: Vec<String> = d.quantities.iter().map(|q| q.name.clone()).collect();
        let base = BaseDoc::new("base".into(), "x".into(), &[]);
        let q = QuantityDoc {
            id: fresh_id(),
            name: unique("quantity", &taken),
            combine: None,
            bases: vec![base],
        };
        sel = Some(RSel::Base(q.id, q.bases[0].id));
        d.quantities.push(q);
    });
    if let Some(s) = sel {
        ctx.sel.set(s);
    }
}

fn add_base(q: u64) {
    let ctx = rctx();
    let mut sel = None;
    ctx.doc.update(|d| {
        let Some(qd) = d.quantity_mut(q) else { return };
        let taken: Vec<String> = qd.bases.iter().map(|b| b.name.clone()).collect();
        let outputs = qd
            .bases
            .first()
            .map(|b| b.outputs.clone())
            .unwrap_or_default();
        let b = BaseDoc::new(unique("base", &taken), "x".into(), &outputs);
        sel = Some(RSel::Base(q, b.id));
        qd.bases.push(b);
    });
    if let Some(s) = sel {
        ctx.sel.set(s);
    }
}

#[component]
fn RulesOutline() -> impl IntoView {
    let ctx = rctx();
    let tree = Memo::new(move |_| {
        ctx.doc.with(|d| {
            d.quantities
                .iter()
                .map(|q| {
                    let bases: Vec<(u64, String)> =
                        q.bases.iter().map(|b| (b.id, b.name.clone())).collect();
                    (q.id, q.name.clone(), bases)
                })
                .collect::<Vec<_>>()
        })
    });
    let selected = move |s: RSel| ctx.sel.get() == s;
    view! {
        <nav>
            <ul class="tree">
                <li class="row big" class:selected=move || selected(RSel::File) on:click=move |_| ctx.sel.set(RSel::File)>
                    <span class="icon"><Icon name="doc"/></span>
                    <span class="name">"Rule file"</span>
                </li>
            </ul>
            <div class="heading">
                <span>"Quantities"</span>
                <button class="mini add" title="Add a quantity" on:click=move |_| add_quantity()><Icon name="plus"/></button>
            </div>
            <ul class="tree">
                {move || tree.get().into_iter().map(|(q, name, bases)| {
                    view! {
                        <li class="row group" class:selected=move || selected(RSel::Quantity(q)) on:click=move |_| ctx.sel.set(RSel::Quantity(q))>
                            <span class="icon"><Icon name="func"/></span>
                            <span class="name">{name}</span>
                            <button class="mini add" title="Add a rule base" on:click=move |ev| {
                                ev.stop_propagation();
                                add_base(q);
                            }><Icon name="plus"/></button>
                        </li>
                        {bases.into_iter().map(|(b, bn)| view! {
                            <li class="row" style="padding-left: 32px" class:selected=move || selected(RSel::Base(q, b)) on:click=move |_| ctx.sel.set(RSel::Base(q, b))>
                                <span class="icon"><Icon name="list"/></span>
                                <span class="name">{bn}</span>
                            </li>
                        }).collect_view()}
                    }
                }).collect_view()}
                {move || tree.with(Vec::is_empty).then(|| view! {
                    <li class="empty">"No quantity yet."</li>
                })}
            </ul>
        </nav>
    }
}

// ---------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------

#[component]
fn FileView() -> impl IntoView {
    let ctx = rctx();
    let extends_text = move || {
        ctx.doc.with(|d| {
            d.extends
                .as_ref()
                .map(|e| {
                    serde_norway::to_string(e)
                        .unwrap_or_default()
                        .trim_end()
                        .to_string()
                })
                .unwrap_or_default()
        })
    };
    let set_extends = move |ev: leptos::ev::Event| {
        let t = event_target_value(&ev);
        if t.trim().is_empty() {
            ctx.doc.update(|d| d.extends = None);
            return;
        }
        match serde_norway::from_str::<serde_norway::Value>(&t) {
            Ok(v) => ctx.doc.update(|d| d.extends = Some(v)),
            Err(e) => ctx.notice.set(Some((false, format!("extends: {e}")))),
        }
    };
    let load = move |ev: leptos::ev::Event| {
        let input: HtmlInputElement = event_target(&ev);
        spawn_local(async move {
            let files = io::read_files(&input).await;
            ctx.library.update(|l| {
                for (n, t) in files {
                    l.retain(|(m, _)| *m != n);
                    l.push((n, t));
                }
            });
        });
    };
    let files = move || {
        ctx.good
            .with(|g| g.as_ref().map(|s| s.files.clone()).unwrap_or_default())
    };
    let quantities = Memo::new(move |_| {
        ctx.doc.with(|d| {
            d.quantities
                .iter()
                .map(|q| (q.id, q.name.clone(), q.bases.len()))
                .collect::<Vec<_>>()
        })
    });
    view! {
        <div class="editor-head">
            <div class="big-icon root"><Icon name="doc"/></div>
            <div class="head-text">
                <div class="crumbs"><span class="last">{move || ctx.name.get()}</span></div>
                <div class="kind-line">"A backend's fuzzy rules: quantities, each made of rule bases"</div>
            </div>
        </div>
        <div class="card">
            <div class="card-title"><Icon name="func"/>"Quantities"</div>
            <p class="hint">"A quantity is what a backend reads a range of (the road's grip, how far drivers see). Its rule bases each read some inputs; " <code>"combine"</code> " joins their outputs."</p>
            <ul class="list">
                {move || quantities.get().into_iter().map(|(q, n, k)| view! {
                    <li class="line">
                        <button class="pill" on:click=move |_| ctx.sel.set(RSel::Quantity(q))><Icon name="func"/>{n}</button>
                        <span class="hint">{format!("{k} rule base{}", if k == 1 { "" } else { "s" })}</span>
                    </li>
                }).collect_view()}
            </ul>
            <div class="actions">
                <button class="button subtle" on:click=move |_| add_quantity()><Icon name="plus"/>"Add a quantity"</button>
            </div>
        </div>
        <div class="card">
            <div class="card-title"><Icon name="link"/>"Extends"</div>
            <p class="hint">"Other rule files this one is read over: it then names only what it changes. One reference or a list: a path, an https:// URL, " <code>"{url, sha256}"</code> " or " <code>"{git, rev, path}"</code> "."</p>
            <textarea class="code" style="min-height: 60px" spellcheck="false" placeholder="base_weather.yaml"
                prop:value=extends_text on:change=set_extends></textarea>
            <p class="hint">"The browser cannot fetch them: load the files here, found by their file name."</p>
            <ul class="list">
                {move || ctx.library.get().into_iter().map(|(n, _)| {
                    let n2 = n.clone();
                    view! {
                        <li class="line">
                            <span class="mono">{n}</span>
                            <button class="mini danger" title="Forget" on:click=move |_| ctx.library.update(|l| l.retain(|(m, _)| *m != n2))><Icon name="x"/></button>
                        </li>
                    }
                }).collect_view()}
            </ul>
            <label class="button subtle">
                <Icon name="upload"/>"Load files it extends"
                <input type="file" multiple accept=".yaml,.yml" class="hidden" on:change=load/>
            </label>
        </div>
        <div class="card">
            <div class="card-title"><Icon name="doc"/>"Files read"</div>
            <p class="hint">"What a run records of the rules it followed: every file, the extended ones first, with its SHA-256."</p>
            <ul class="list">
                {move || files().into_iter().map(|f| view! {
                    <li class="line"><span class="mono">{f.label}</span><span class="tag">{f.sha256[..12].to_string()}</span></li>
                }).collect_view()}
            </ul>
        </div>
    }
}

// ---------------------------------------------------------------------------
// A quantity
// ---------------------------------------------------------------------------

#[component]
fn QuantityView(q: u64) -> impl IntoView {
    let ctx = rctx();
    let quantity = Memo::new(move |_| ctx.doc.with(|d| d.quantity(q).cloned()));
    let rename = move |ev: leptos::ev::Event| {
        let n = event_target_value(&ev).trim().to_string();
        if n.is_empty() {
            return;
        }
        ctx.doc.update(|d| {
            if let Some(x) = d.quantity_mut(q) {
                x.name = n.clone();
            }
        });
        ctx.tested.set(Some(n));
    };
    let set_combine = move |c: Option<&'static str>| {
        ctx.doc.update(|d| {
            if let Some(x) = d.quantity_mut(q) {
                x.combine = c.map(str::to_string);
            }
        })
    };
    let remove = move |_| {
        ctx.doc.update(|d| d.quantities.retain(|x| x.id != q));
        ctx.sel.set(RSel::File);
    };
    let combine_button = move |c: Option<&'static str>, label: &'static str| {
        view! {
            <button
                class:active=move || quantity.with(|x| x.as_ref().map(|x| x.combine.as_deref()) == Some(c))
                on:click=move |_| set_combine(c)
            >{label}</button>
        }
    };
    let outputs_at = Memo::new(move |_| {
        let name = quantity.with(|x| x.as_ref().map(|x| x.name.clone()))?;
        let probe = ctx.probe.get();
        ctx.good.with(|g| {
            let set = g.as_ref()?;
            let qd = set.quantities.get(&name)?;
            let values = rules_doc::values_at(set, &name, &probe);
            let each = qd.evaluate_each(&values).ok()?;
            let joined = qd.evaluate(&values).ok()?;
            Some((qd.outputs.clone(), each, joined))
        })
    });
    move || {
        let Some(qd) = quantity.get() else {
            return view! { <p class="hint">"This quantity is gone."</p> }.into_any();
        };
        view! {
            <div class="editor-head">
                <div class="big-icon"><Icon name="func"/></div>
                <div class="head-text">
                    <div class="crumbs"><span class="last">{qd.name.clone()}</span></div>
                    <div class="kind-line">{format!("quantity · {} rule base{}", qd.bases.len(), if qd.bases.len() == 1 { "" } else { "s" })}</div>
                </div>
                <div class="actions">
                    <button class="mini danger" on:click=remove><Icon name="trash"/>"Remove"</button>
                </div>
            </div>
            <div class="card">
                <div class="grid2">
                    <label class="field">
                        <span>"Name"</span>
                        <input prop:value=qd.name.clone() on:change=rename/>
                    </label>
                    <div class="field">
                        <span>"Combine the rule bases' outputs by"</span>
                        <div class="segmented">
                            {combine_button(None, "min (default)")}
                            {combine_button(Some("min"), "min")}
                            {combine_button(Some("max"), "max")}
                        </div>
                    </div>
                </div>
                <p class="hint">"Output by output: " <b>"min"</b> " takes the lowest of the rule bases (the shortest view of fog, rain and night), " <b>"max"</b> " the highest."</p>
            </div>
            <div class="card">
                <div class="card-title"><Icon name="list"/>"Rule bases at the tested values"</div>
                {move || match outputs_at.get() {
                    None => view! { <p class="hint">"Not read yet."</p> }.into_any(),
                    Some((outputs, each, joined)) => view! {
                        <table class="rules-table">
                            <thead><tr><th>"Rule base"</th>{outputs.iter().map(|o| view! { <th class="num">{o.clone()}</th> }).collect_view()}</tr></thead>
                            <tbody>
                                {each.into_iter().map(|(bn, out)| {
                                    let b = qd.bases.iter().find(|x| x.name == bn).map(|x| x.id);
                                    let cells = outputs.iter().map(|o| {
                                        let v = out.get(o).copied().unwrap_or(f64::NAN);
                                        let wins = joined.get(o).is_some_and(|j| (*j - v).abs() < 1e-12);
                                        view! { <td class="num" class:wins=wins>{num(v)}</td> }
                                    }).collect_view();
                                    view! {
                                        <tr>
                                            <td>{match b {
                                                Some(b) => view! { <button class="pill" on:click=move |_| ctx.sel.set(RSel::Base(q, b))><Icon name="list"/>{bn.clone()}</button> }.into_any(),
                                                None => view! { <span>{bn.clone()}</span> }.into_any(),
                                            }}</td>
                                            {cells}
                                        </tr>
                                    }
                                }).collect_view()}
                                <tr class="total">
                                    <td>"combined"</td>
                                    {outputs.iter().map(|o| view! { <td class="num">{num(joined.get(o).copied().unwrap_or(f64::NAN))}</td> }).collect_view()}
                                </tr>
                            </tbody>
                        </table>
                        <p class="hint">"Bold: the rule base " <code>"combine"</code> " takes. Set the values in the panel's test drive."</p>
                    }.into_any(),
                }}
                <div class="actions">
                    <button class="button subtle" on:click=move |_| add_base(q)><Icon name="plus"/>"Add a rule base"</button>
                </div>
            </div>
        }
        .into_any()
    }
}

// ---------------------------------------------------------------------------
// A rule base
// ---------------------------------------------------------------------------

#[component]
fn BaseView(q: u64, b: u64) -> impl IntoView {
    let ctx = rctx();
    let base = Memo::new(move |_| ctx.doc.with(|d| d.base(q, b).cloned()));
    let qname = Memo::new(move |_| {
        ctx.doc
            .with(|d| d.quantity(q).map(|x| x.name.clone()).unwrap_or_default())
    });
    let input_ids = Memo::new(move |_| {
        base.with(|x| {
            x.as_ref()
                .and_then(|x| x.inputs.as_ref())
                .map(|is| is.iter().map(|i| i.id).collect::<Vec<_>>())
        })
    });
    let rename = move |ev: leptos::ev::Event| {
        let n = event_target_value(&ev).trim().to_string();
        if !n.is_empty() {
            ctx.base_update(q, b, |x| x.name = n);
        }
    };
    let remove = move |_| {
        ctx.doc.update(|d| {
            if let Some(x) = d.quantity_mut(q) {
                x.bases.retain(|y| y.id != b);
            }
        });
        ctx.sel.set(RSel::Quantity(q));
    };
    let add_input = move |_| {
        ctx.base_update(q, b, |x| {
            let inputs = x.inputs.get_or_insert_with(Vec::new);
            let taken: Vec<String> = inputs.iter().map(|i| i.name.clone()).collect();
            inputs.push(InputDoc::new(unique("input", &taken)));
        })
    };
    let take_inputs = move |_| {
        if let Some(read) = ctx.read_base(q, b) {
            let inputs: Vec<InputDoc> = read.inputs.iter().map(input_doc).collect();
            ctx.base_update(q, b, |x| x.inputs = Some(inputs));
        }
    };
    let name_value = move || base.with(|x| x.as_ref().map(|x| x.name.clone()).unwrap_or_default());
    view! {
        <div class="editor-head">
            <div class="big-icon"><Icon name="list"/></div>
            <div class="head-text">
                <div class="crumbs">
                    <span>{move || qname.get()}</span>
                    <span class="sep">"›"</span>
                    <span class="last">{name_value}</span>
                </div>
                <div class="kind-line">"rule base: inputs sorted into terms, and rules giving the outputs"</div>
            </div>
            <div class="actions">
                <button class="mini danger" on:click=remove><Icon name="trash"/>"Remove"</button>
            </div>
        </div>
        <div class="card">
            <label class="field">
                <span>"Name"</span>
                <input prop:value=name_value on:change=rename/>
            </label>
        </div>
        <h3>"Inputs"</h3>
        {move || match input_ids.get() {
            None => view! {
                <div class="card">
                    <p class="hint">"This file does not name the inputs: the file it extends gives them."</p>
                    <button class="button subtle" on:click=take_inputs><Icon name="plus"/>"Write them here to change them"</button>
                </div>
            }.into_any(),
            Some(_) => view! {
                <For
                    each=move || input_ids.get().unwrap_or_default()
                    key=|id| *id
                    children=move |i| view! { <InputCard q=q b=b i=i/> }
                />
                <div class="actions">
                    <button class="button subtle" on:click=add_input><Icon name="plus"/>"Add an input"</button>
                </div>
            }.into_any(),
        }}
        <h3>"Rules"</h3>
        <RulesCard q=q b=b/>
    }
}

#[component]
fn InputCard(q: u64, b: u64, i: u64) -> impl IntoView {
    let ctx = rctx();
    let input = Memo::new(move |_| {
        ctx.doc.with(|d| {
            d.base(q, b)?
                .inputs
                .as_ref()?
                .iter()
                .find(|x| x.id == i)
                .cloned()
        })
    });
    let get = move |f: fn(&InputDoc) -> String| {
        move || input.with(|x| x.as_ref().map(f).unwrap_or_default())
    };
    let rename = move |ev: leptos::ev::Event| {
        let new = event_target_value(&ev).trim().to_string();
        let Some(old) = input.with_untracked(|x| x.as_ref().map(|x| x.name.clone())) else {
            return;
        };
        if new.is_empty() || new == old {
            return;
        }
        ctx.base_update(q, b, |x| x.rename_input(&old, &new));
        if let Some(v) = ctx.probe_value(&old) {
            ctx.set_probe(&new, v);
        }
    };
    let set_low = move |ev: leptos::ev::Event| {
        if let Some(v) = parse_num(&event_target_value(&ev)) {
            ctx.input_update(q, b, i, |x| x.low = v);
        }
    };
    let set_high = move |ev: leptos::ev::Event| {
        if let Some(v) = parse_num(&event_target_value(&ev)) {
            ctx.input_update(q, b, i, |x| x.high = v);
        }
    };
    let set_default = move |ev: leptos::ev::Event| {
        let t = event_target_value(&ev);
        let v = parse_num(&t);
        ctx.input_update(q, b, i, |x| x.default = v);
    };
    let remove = move |_| {
        ctx.base_update(q, b, |x| {
            if let Some(is) = &mut x.inputs {
                is.retain(|y| y.id != i)
            }
        })
    };
    let add_term = move |_| {
        ctx.input_update(q, b, i, |x| {
            let taken: Vec<String> = x.terms.iter().map(|t| t.name.clone()).collect();
            let mid = (x.low + x.high) / 2.0;
            let w = (x.high - x.low) / 4.0;
            x.terms.push(Term {
                id: fresh_id(),
                name: unique("term", &taken),
                shape: Shape::Triangle([tidy(mid - w), tidy(mid), tidy(mid + w)]),
            });
        })
    };
    let spread = move |_| ctx.input_update(q, b, i, InputDoc::spread);
    let term_ids = Memo::new(move |_| {
        input.with(|x| {
            x.as_ref()
                .map(|x| x.terms.iter().map(|t| t.id).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    view! {
        <div class="card input-card">
            <div class="card-head">
                <div class="card-title"><Icon name="ruler"/>{move || shown(&get(|x| x.name.clone())())}
                    {move || other_name(&get(|x| x.name.clone())()).map(|o| view! { <span class="hint mono">{o}</span> })}
                </div>
                <div class="actions">
                    <button class="mini" title="Lay the terms out evenly, each peaking where its neighbours end" on:click=spread><Icon name="sparkle"/>"Spread evenly"</button>
                    <button class="mini danger" on:click=remove><Icon name="trash"/></button>
                </div>
            </div>
            <div class="grid4">
                <label class="field"><span>"Name"</span><input prop:value=get(|x| x.name.clone()) on:change=rename/></label>
                <label class="field"><span>"Universe from"</span><input inputmode="decimal" prop:value=get(|x| num(x.low)) on:change=set_low/></label>
                <label class="field"><span>"to"</span><input inputmode="decimal" prop:value=get(|x| num(x.high)) on:change=set_high/></label>
                <label class="field"><span>"Default"</span><input inputmode="decimal" placeholder="none" prop:value=get(|x| x.default.map(num).unwrap_or_default()) on:change=set_default/></label>
            </div>
            <MembershipChart q=q b=b i=i input=input/>
            <AlignmentNote input=input/>
            <p class="hint">"Drag a term's corners. Click the chart to test a value."</p>
            <table class="terms-table">
                <thead><tr><th></th><th>"Term"</th><th>"Shape"</th><th>"Corners"</th><th></th></tr></thead>
                <tbody>
                    <For each=move || term_ids.get() key=|id| *id children=move |t| view! { <TermRow q=q b=b i=i t=t input=input/> }/>
                </tbody>
            </table>
            <div class="actions">
                <button class="button subtle" on:click=add_term><Icon name="plus"/>"Add a term"</button>
            </div>
        </div>
    }
}

/// How the input's terms meet the ODD's buckets, where the binding and the
/// manifest say which attribute sets the input.
#[component]
fn AlignmentNote(input: Memo<Option<InputDoc>>) -> impl IntoView {
    move || {
        let x = input.get()?;
        let link = crate::binding_app::bctx().link_of_input(&x.name)?;
        let spans = link.spans();
        if spans.is_empty() {
            return None;
        }
        let terms: Vec<(String, Shape)> =
            x.terms.iter().map(|t| (t.name.clone(), t.shape)).collect();
        let rows = crate::binding::alignment(&spans, &terms);
        let same = rows.iter().all(|r| r.2);
        Some(view! {
            <div class="alignment">
                <span class=if same { "tag ok" } else { "tag bad" }>
                    {if same { "terms are the ODD's buckets" } else { "terms differ from the ODD's buckets" }}
                </span>
                <span class="hint">{format!("{} sets it through {}:", link.attribute, link.knob.map(|k| k.key).unwrap_or_default())}</span>
                {rows.into_iter().map(|(label, best, ok)| view! {
                    <span class="pair" class:bad=!ok>
                        <b>{label}</b>" → "{best.unwrap_or_else(|| "none".into())}
                    </span>
                }).collect_view()}
            </div>
        })
    }
}

#[component]
fn TermRow(q: u64, b: u64, i: u64, t: u64, input: Memo<Option<InputDoc>>) -> impl IntoView {
    let ctx = rctx();
    let term =
        Memo::new(move |_| input.with(|x| x.as_ref()?.terms.iter().find(|y| y.id == t).cloned()));
    let index = Memo::new(move |_| {
        input.with(|x| {
            x.as_ref()
                .and_then(|x| x.terms.iter().position(|y| y.id == t))
                .unwrap_or(0)
        })
    });
    let update = move |f: Box<dyn FnOnce(&mut Term)>| {
        ctx.input_update(q, b, i, |x| {
            if let Some(term) = x.terms.iter_mut().find(|y| y.id == t) {
                f(term)
            }
        })
    };
    let rename = move |ev: leptos::ev::Event| {
        let new = event_target_value(&ev).trim().to_string();
        let Some((input_name, old)) = input.with_untracked(|x| {
            let x = x.as_ref()?;
            Some((
                x.name.clone(),
                x.terms.iter().find(|y| y.id == t)?.name.clone(),
            ))
        }) else {
            return;
        };
        if !new.is_empty() && new != old {
            ctx.base_update(q, b, |x| x.rename_term(&input_name, &old, &new));
        }
    };
    let set_kind = move |ev: leptos::ev::Event| {
        let k = event_target_value(&ev);
        update(Box::new(move |x| x.shape = x.shape.as_kind(&k)));
    };
    let remove = move |_| {
        ctx.input_update(q, b, i, |x| x.terms.retain(|y| y.id != t));
    };
    let params = move || {
        let Some(term) = term.get() else {
            return ().into_any();
        };
        let names: &[&str] = match term.shape {
            Shape::Triangle(_) => &["a", "peak", "c"],
            Shape::Trapezoid(_) => &["a", "b", "c", "d"],
            Shape::Gaussian { .. } => &["mean", "sigma"],
        };
        term.shape
            .params()
            .into_iter()
            .enumerate()
            .map(|(k, v)| {
                let label = names[k];
                view! {
                    <input class="corner" title=label aria-label=label inputmode="decimal" prop:value=num(v)
                        on:change=move |ev| {
                            if let Some(v) = parse_num(&event_target_value(&ev)) {
                                update(Box::new(move |x| x.shape = x.shape.with_param(k, v)));
                            }
                        }/>
                }
            })
            .collect_view()
            .into_any()
    };
    view! {
        <tr>
            <td><span class=move || format!("swatch s{}", index.get() % 8)></span></td>
            <td><input class="term-name" prop:value=move || term.with(|x| x.as_ref().map(|x| x.name.clone()).unwrap_or_default()) on:change=rename/></td>
            <td>
                <select on:change=set_kind prop:value=move || term.with(|x| x.as_ref().map(|x| x.shape.kind()).unwrap_or("triangle"))>
                    {SHAPES.iter().map(|s| view! { <option value=*s>{*s}</option> }).collect_view()}
                </select>
            </td>
            <td class="corners">{params}</td>
            <td><button class="mini danger" title="Remove the term" on:click=remove><Icon name="x"/></button></td>
        </tr>
    }
}

const CW: f64 = 600.0;
const CTOP: f64 = 24.0;
const CBOT: f64 = 128.0;

/// Ticks at round steps across `[lo, hi]`.
fn ticks(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return vec![lo];
    }
    let raw = (hi - lo) / n as f64;
    let p = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * p)
        .find(|s| *s >= raw)
        .unwrap_or(raw);
    let mut out = vec![];
    let mut v = (lo / step).ceil() * step;
    while v <= hi + step * 1e-9 {
        out.push(tidy(v));
        v += step;
    }
    out
}

/// An input's terms over its universe, their corners draggable, and the
/// value tested.
#[component]
fn MembershipChart(q: u64, b: u64, i: u64, input: Memo<Option<InputDoc>>) -> impl IntoView {
    let ctx = rctx();
    let svg_ref = NodeRef::<leptos::svg::Svg>::new();
    let dragging = RwSignal::new(None::<(u64, usize)>);
    let range = move || input.with(|x| x.as_ref().map(|x| (x.low, x.high)).unwrap_or((0.0, 1.0)));
    let sx = move |v: f64| {
        let (lo, hi) = range();
        if hi > lo {
            (v - lo) / (hi - lo) * CW
        } else {
            0.0
        }
    };
    let sy = move |d: f64| CBOT - d * (CBOT - CTOP);
    let to_value = move |client_x: f64| -> Option<f64> {
        let el = svg_ref.get()?;
        let el: &web_sys::Element = el.unchecked_ref();
        let rect = el.get_bounding_client_rect();
        if rect.width() <= 0.0 {
            return None;
        }
        let x = (client_x - rect.left()) / rect.width() * (CW + 40.0) - 20.0;
        let (lo, hi) = range();
        let v = lo + x / CW * (hi - lo);
        // Snap to a round step: a few hundred across the universe.
        let step = 10f64.powf(((hi - lo) / 200.0).log10().floor());
        Some(tidy((v / step).round() * step))
    };
    let name = move || input.with(|x| x.as_ref().map(|x| x.name.clone()).unwrap_or_default());
    let tested = move || {
        let n = name();
        ctx.probe_value(&n).or_else(|| {
            input.with(|x| {
                x.as_ref()
                    .map(|x| x.default.unwrap_or((x.low + x.high) / 2.0))
            })
        })
    };
    let pointer_down = move |ev: web_sys::PointerEvent| {
        let target: web_sys::Element = ev.target().unwrap().unchecked_into();
        if let Some((t, k)) = target.get_attribute("data-handle").and_then(|s| {
            let (t, k) = s.split_once(':')?;
            Some((t.parse().ok()?, k.parse().ok()?))
        }) {
            dragging.set(Some((t, k)));
            if let Some(el) = svg_ref.get() {
                let el: &web_sys::Element = el.unchecked_ref();
                let _ = el.set_pointer_capture(ev.pointer_id());
            }
            ev.prevent_default();
        } else if let Some(v) = to_value(ev.client_x() as f64) {
            let (lo, hi) = range();
            ctx.set_probe(&name(), v.clamp(lo, hi));
        }
    };
    let pointer_move = move |ev: web_sys::PointerEvent| {
        if let (Some((t, k)), Some(v)) = (dragging.get_untracked(), to_value(ev.client_x() as f64))
        {
            ctx.input_update(q, b, i, |x| {
                let (lo, hi) = (x.low, x.high);
                if let Some(term) = x.terms.iter_mut().find(|y| y.id == t) {
                    term.shape = term.shape.drag(k, v, lo, hi);
                }
            });
        }
    };
    let pointer_up = move |_ev: web_sys::PointerEvent| dragging.set(None);
    let curves = move || {
        let Some(x) = input.get() else {
            return ().into_any();
        };
        let (lo, hi) = (x.low, x.high);
        x.terms
            .iter()
            .enumerate()
            .map(|(k, t)| {
                let mut xs: Vec<f64> = (0..=120).map(|j| lo + (hi - lo) * j as f64 / 120.0).collect();
                xs.extend(t.shape.handles().iter().map(|(v, _)| *v).filter(|v| *v >= lo && *v <= hi));
                xs.sort_by(f64::total_cmp);
                xs.dedup();
                let line: Vec<String> = xs
                    .iter()
                    .map(|v| format!("{:.1},{:.1}", sx(*v), sy(t.shape.degree(*v))))
                    .collect();
                let area = format!("M{:.1},{CBOT} L{} L{:.1},{CBOT} Z", sx(lo), line.join(" L"), sx(hi));
                let label_x = t.shape.handles().iter().map(|(v, d)| (*v, *d)).filter(|(_, d)| *d >= 0.99).map(|(v, _)| v.clamp(lo, hi)).next().unwrap_or((lo + hi) / 2.0);
                view! {
                    <g class=format!("term s{}", k % 8)>
                        <path class="area" d=area/>
                        <path class="curve" d=format!("M{}", line.join(" L"))/>
                        <text class="term-label" x=sx(label_x) y=CTOP - 10.0 text-anchor="middle">{t.name.clone()}</text>
                    </g>
                }
            })
            .collect_view()
            .into_any()
    };
    let handles = move || {
        let Some(x) = input.get() else {
            return ().into_any();
        };
        let (lo, hi) = (x.low, x.high);
        x.terms
            .iter()
            .enumerate()
            .flat_map(|(k, t)| {
                let id = t.id;
                t.shape
                    .handles()
                    .into_iter()
                    .enumerate()
                    .map(move |(j, (v, d))| {
                        let active = move || dragging.get() == Some((id, j));
                        let title = num(v);
                        view! {
                            <circle class=format!("handle-dot s{}", k % 8) class:active=active
                                cx=sx(v.clamp(lo, hi)) cy=sy(d) r="6" data-handle=format!("{id}:{j}")>
                                <title>{title}</title>
                            </circle>
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect_view()
            .into_any()
    };
    let axis = move || {
        let (lo, hi) = range();
        ticks(lo, hi, 6)
            .into_iter()
            .map(|t| {
                let x = sx(t);
                view! {
                    <line class="grid-line" x1=x x2=x y1=CTOP y2=CBOT/>
                    <text class="tick-label" x=x y=CBOT + 16.0 text-anchor="middle">{num(t)}</text>
                }
            })
            .collect_view()
    };
    // The ODD's buckets, where the binding and the manifest say which
    // attribute sets this input: what each label writes into it.
    let bands = Memo::new(move |_| {
        let n = name();
        let link = crate::binding_app::bctx().link_of_input(&n)?;
        let spans = link.spans();
        (!spans.is_empty()).then_some((link.attribute, spans))
    });
    let band_view = move || {
        let (_, spans) = bands.get()?;
        let (lo, hi) = range();
        let y = CBOT + 26.0;
        let placed: Vec<(String, f64, f64)> = spans
            .into_iter()
            .map(|(label, a, b)| {
                let x0 = sx(a.clamp(lo, hi));
                (label, x0, (sx(b.clamp(lo, hi)) - x0).max(3.0))
            })
            .collect();
        // The bands first, then their labels over them: a label too long
        // for its band sits beside it, over the next.
        let rects = placed
            .iter()
            .enumerate()
            .map(|(k, (_, x0, w))| view! { <rect class=format!("band b{}", k % 2) x=*x0 y=y width=*w height="16" rx="3"/> })
            .collect_view();
        let labels = placed
            .into_iter()
            .map(|(label, x0, w)| {
                let fits = w >= 7.0 * label.len() as f64 + 8.0;
                let (tx, anchor) = if fits {
                    (x0 + w / 2.0, "middle")
                } else {
                    (x0 + w + 3.0, "start")
                };
                view! { <text class="band-label" x=tx y=y + 12.0 text-anchor=anchor>{label}</text> }
            })
            .collect_view();
        Some(view! { <g class="bands">{rects}{labels}</g> })
    };
    let marker = move || {
        let v = tested()?;
        let x = sx(v);
        Some(view! {
            <g class="probe">
                <line x1=x x2=x y1=CTOP - 2.0 y2=CBOT/>
                <text x=x + 4.0 y=CBOT - 4.0>{num(v)}</text>
            </g>
        })
    };
    view! {
        <svg
            node_ref=svg_ref
            class="membership"
            viewBox=move || format!("-20 0 {} {}", CW + 40.0, if bands.with(Option::is_some) { 182.0 } else { 154.0 })
            preserveAspectRatio="xMidYMid meet"
            on:pointerdown=pointer_down
            on:pointermove=pointer_move
            on:pointerup=pointer_up
            on:pointercancel=pointer_up
        >
            <rect class="plot-bg" x="0" y=CTOP width=CW height=CBOT - CTOP/>
            {axis}
            {band_view}
            {curves}
            {marker}
            {handles}
        </svg>
    }
}

#[component]
fn RulesCard(q: u64, b: u64) -> impl IntoView {
    let ctx = rctx();
    let base = Memo::new(move |_| ctx.doc.with(|d| d.base(q, b).cloned()));
    // The inputs the rules may name: the file's, else those it extends.
    let inputs = Memo::new(move |_| {
        let own = base.with(|x| x.as_ref().and_then(|x| x.inputs.clone()));
        let list: Vec<(String, Vec<String>)> = match own {
            Some(is) => is
                .iter()
                .map(|i| {
                    (
                        i.name.clone(),
                        i.terms.iter().map(|t| t.name.clone()).collect(),
                    )
                })
                .collect(),
            None => ctx
                .read_base(q, b)
                .map(|r| {
                    r.inputs
                        .iter()
                        .map(|i| (i.name.clone(), i.terms.keys().cloned().collect()))
                        .collect()
                })
                .unwrap_or_default(),
        };
        list
    });
    let outputs = Memo::new(move |_| {
        base.with(|x| x.as_ref().map(|x| x.outputs.clone()).unwrap_or_default())
    });
    let rule_ids = Memo::new(move |_| {
        base.with(|x| {
            x.as_ref()
                .and_then(|x| x.rules.as_ref())
                .map(|rs| rs.iter().map(|r| r.id).collect::<Vec<_>>())
        })
    });
    let firing = Memo::new(move |_| ctx.firing(q, b));
    let add_rule = move |_| {
        let first = inputs.with(|is| {
            is.first()
                .and_then(|(n, ts)| ts.first().map(|t| (n.clone(), t.clone())))
        });
        ctx.base_update(q, b, |x| {
            let n = x.outputs.len();
            x.rules.get_or_insert_with(Vec::new).push(RuleDoc {
                id: fresh_id(),
                when: first.map(|c| when_of(&[c])).unwrap_or_default(),
                then: vec![Some(0.0); n],
            });
        })
    };
    let add_each = move |_| {
        let Some((input, terms)) = inputs.with(|is| is.first().cloned()) else {
            return;
        };
        ctx.base_update(q, b, |x| {
            let n = x.outputs.len();
            let rules = x.rules.get_or_insert_with(Vec::new);
            for t in terms {
                let c = vec![(input.clone(), t)];
                if !rules.iter().any(|r| clauses(&r.when).as_ref() == Some(&c)) {
                    rules.push(RuleDoc {
                        id: fresh_id(),
                        when: when_of(&c),
                        then: vec![Some(0.0); n],
                    });
                }
            }
        })
    };
    let add_output = move |_| {
        ctx.base_update(q, b, |x| {
            let o = unique("output", &x.outputs);
            x.outputs.push(o);
            for r in x.rules.iter_mut().flatten() {
                r.then.push(Some(0.0));
            }
        })
    };
    let take_rules = move |_| {
        if let Some(read) = ctx.read_base(q, b) {
            let outputs = read.outputs.clone();
            let rules: Vec<RuleDoc> = read
                .rules
                .iter()
                .map(|r| RuleDoc {
                    id: fresh_id(),
                    when: r.antecedent.clone(),
                    then: outputs.iter().map(|o| r.then.get(o).copied()).collect(),
                })
                .collect();
            ctx.base_update(q, b, |x| {
                x.outputs = outputs;
                x.rules = Some(rules);
            });
        }
    };
    view! {
        <div class="card">
            {move || match rule_ids.get() {
                None => view! {
                    <p class="hint">"This file does not name the rules: the file it extends gives them."</p>
                    <button class="button subtle" on:click=take_rules><Icon name="plus"/>"Write them here to change them"</button>
                }.into_any(),
                Some(_) => view! {
                    <p class="hint">"A rule fires as strongly as its antecedent holds; each output is the rules' values averaged by how strongly each fires. Pick one term per input (blank: any), or write the antecedent with " <code>"AND"</code> ", " <code>"OR"</code> ", " <code>"NOT"</code> "."</p>
                    <div class="table-scroll">
                        <table class="rules-table">
                            <thead>
                                <tr>
                                    <th>"#"</th>
                                    {move || inputs.get().into_iter().map(|(n, _)| view! { <th>{shown(&n)}</th> }).collect_view()}
                                    {move || outputs.get().into_iter().enumerate().map(|(k, o)| view! { <OutputHead q=q b=b k=k name=o/> }).collect_view()}
                                    <th><button class="mini" title="Add an output" on:click=add_output><Icon name="plus"/></button></th>
                                    <th>"Fires"</th>
                                    <th></th>
                                </tr>
                            </thead>
                            <tbody>
                                <For
                                    each=move || rule_ids.get().unwrap_or_default()
                                    key=|id| *id
                                    children=move |r| view! { <RuleRow q=q b=b r=r inputs=inputs firing=firing/> }
                                />
                            </tbody>
                        </table>
                    </div>
                    <div class="actions">
                        <button class="button subtle" on:click=add_rule><Icon name="plus"/>"Add a rule"</button>
                        <Show when=move || inputs.with(|i| i.len() == 1)>
                            <button class="button subtle" on:click=add_each><Icon name="sparkle"/>"A rule per term"</button>
                        </Show>
                    </div>
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn OutputHead(q: u64, b: u64, k: usize, name: String) -> impl IntoView {
    let ctx = rctx();
    let rename = move |ev: leptos::ev::Event| {
        let n = event_target_value(&ev).trim().to_string();
        if !n.is_empty() {
            ctx.base_update(q, b, |x| {
                if let Some(o) = x.outputs.get_mut(k) {
                    *o = n;
                }
            });
        }
    };
    let remove = move |_| {
        ctx.base_update(q, b, |x| {
            if k < x.outputs.len() {
                x.outputs.remove(k);
                for r in x.rules.iter_mut().flatten() {
                    if k < r.then.len() {
                        r.then.remove(k);
                    }
                }
            }
        })
    };
    view! {
        <th class="output-head">
            <input class="output-name" prop:value=name on:change=rename aria-label="Output name"/>
            <button class="mini danger" title="Remove the output" on:click=remove><Icon name="x"/></button>
        </th>
    }
}

#[component]
fn RuleRow(
    q: u64,
    b: u64,
    r: u64,
    inputs: Memo<Vec<(String, Vec<String>)>>,
    firing: Memo<Option<Vec<f64>>>,
) -> impl IntoView {
    let ctx = rctx();
    let rule = Memo::new(move |_| {
        ctx.doc.with(|d| {
            d.base(q, b)?
                .rules
                .as_ref()?
                .iter()
                .find(|x| x.id == r)
                .cloned()
        })
    });
    let index = Memo::new(move |_| {
        ctx.doc.with(|d| {
            d.base(q, b)
                .and_then(|x| x.rules.as_ref())
                .and_then(|rs| rs.iter().position(|x| x.id == r))
                .unwrap_or(0)
        })
    });
    let as_text = RwSignal::new(false);
    // The rule's clauses, when term pickers show them.
    let picked = Memo::new(move |_| {
        let when = rule.with(|x| x.as_ref().map(|x| x.when.clone()).unwrap_or_default());
        let cs = clauses(&when)?;
        inputs.with(|is| {
            cs.iter()
                .all(|(i, t)| is.iter().any(|(n, ts)| n == i && ts.contains(t)))
                .then_some(cs)
        })
    });
    let pick = move |input: String, term: String| {
        let order: Vec<String> = inputs.with(|is| is.iter().map(|(n, _)| n.clone()).collect());
        let mut cs = picked.get_untracked().unwrap_or_default();
        cs.retain(|(i, _)| *i != input);
        if !term.is_empty() {
            cs.push((input, term));
        }
        cs.sort_by_key(|(i, _)| order.iter().position(|o| o == i));
        let when = when_of(&cs);
        ctx.rule_update(q, b, r, |x| x.when = when);
    };
    let set_when = move |ev: leptos::ev::Event| {
        let w = event_target_value(&ev);
        ctx.rule_update(q, b, r, |x| x.when = w);
    };
    let remove = move |_| {
        ctx.base_update(q, b, |x| {
            if let Some(rs) = &mut x.rules {
                rs.retain(|y| y.id != r)
            }
        })
    };
    let fires = move || firing.with(|f| f.as_ref().and_then(|f| f.get(index.get()).copied()));
    let condition = move || {
        let n = inputs.with(Vec::len).max(1);
        match (picked.get(), as_text.get()) {
            (Some(cs), false) => inputs
                .get()
                .into_iter()
                .map(|(name, terms)| {
                    let chosen = cs.iter().find(|(i, _)| *i == name).map(|(_, t)| t.clone()).unwrap_or_default();
                    let n2 = name.clone();
                    view! {
                        <td>
                            <select class="term-pick" class:any=chosen.is_empty() prop:value=chosen.clone()
                                on:change=move |ev| pick(n2.clone(), event_target_value(&ev))>
                                <option value="">"—"</option>
                                {terms.into_iter().map(|t| view! { <option value=t.clone()>{t.clone()}</option> }).collect_view()}
                            </select>
                        </td>
                    }
                })
                .collect_view()
                .into_any(),
            _ => view! {
                <td colspan=n.to_string()>
                    <input class="when mono" prop:value=move || rule.with(|x| x.as_ref().map(|x| x.when.clone()).unwrap_or_default()) on:change=set_when aria-label="Antecedent"/>
                </td>
            }
            .into_any(),
        }
    };
    let values = move || {
        let then = rule.with(|x| x.as_ref().map(|x| x.then.clone()).unwrap_or_default());
        then.into_iter()
            .enumerate()
            .map(|(k, v)| {
                view! {
                    <td class="num">
                        <input class="value" inputmode="decimal" placeholder="—" prop:value=v.map(num).unwrap_or_default()
                            on:change=move |ev| {
                                let v = parse_num(&event_target_value(&ev));
                                ctx.rule_update(q, b, r, |x| {
                                    if let Some(slot) = x.then.get_mut(k) {
                                        *slot = v;
                                    }
                                });
                            }/>
                    </td>
                }
            })
            .collect_view()
    };
    view! {
        <tr class:firing=move || fires().is_some_and(|f| f > 0.0)>
            <td class="hint">{move || index.get() + 1}</td>
            {condition}
            {values}
            <td>
                <button class="mini" title=move || if as_text.get() { "Show as term pickers" } else { "Edit as text" }
                    on:click=move |_| as_text.update(|t| *t = !*t)><Icon name="code"/></button>
            </td>
            <td class="fires">
                {move || fires().map(|f| view! {
                    <span class="bar"><span style=format!("width: {:.0}%", f * 100.0)></span></span>
                    <span class="mono">{format!("{f:.2}")}</span>
                })}
            </td>
            <td><button class="mini danger" title="Remove the rule" on:click=remove><Icon name="x"/></button></td>
        </tr>
    }
}

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

#[component]
fn RulesPanel() -> impl IntoView {
    let tab = RwSignal::new(Tab::Test);
    let button = move |t: Tab, label: &'static str, icon: &'static str| {
        view! {
            <button class="tab" class:active=move || tab.get() == t on:click=move |_| tab.set(t)>
                <Icon name=icon/>{label}
            </button>
        }
    };
    view! {
        <div class="tabs">
            {button(Tab::Test, "Test drive", "flask")}
            {button(Tab::Yaml, "YAML", "code")}
            {button(Tab::Details, "Details", "doc")}
        </div>
        <div class="tab-body">
            {move || match tab.get() {
                Tab::Test => view! { <TestTab/> }.into_any(),
                Tab::Yaml => view! { <YamlTab/> }.into_any(),
                Tab::Details => view! { <DetailsTab/> }.into_any(),
            }}
        </div>
    }
}

#[component]
fn TestTab() -> impl IntoView {
    let ctx = rctx();
    let names = Memo::new(move |_| {
        ctx.good.with(|g| {
            g.as_ref()
                .map(|s| s.quantities.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    let quantity = Memo::new(move |_| {
        let all = names.get();
        ctx.tested
            .get()
            .filter(|t| all.contains(t))
            .or_else(|| all.first().cloned())
    });
    // (name, low, high) of each input the quantity reads.
    let inputs = Memo::new(move |_| {
        let Some(qn) = quantity.get() else {
            return vec![];
        };
        ctx.good.with(|g| {
            let Some(q) = g.as_ref().and_then(|s| s.quantities.get(&qn)) else {
                return vec![];
            };
            let mut out: Vec<(String, f64, f64, f64)> = vec![];
            for b in &q.rule_bases {
                for i in &b.inputs {
                    if !out.iter().any(|(n, ..)| *n == i.name) {
                        out.push((
                            i.name.clone(),
                            i.low,
                            i.high,
                            i.default.unwrap_or((i.low + i.high) / 2.0),
                        ));
                    }
                }
            }
            out
        })
    });
    let x = Memo::new(move |_| {
        let all = inputs.get();
        ctx.x
            .get()
            .filter(|x| all.iter().any(|(n, ..)| n == x))
            .or_else(|| all.first().map(|i| i.0.clone()))
    });
    let result = Memo::new(move |_| {
        let qn = quantity.get()?;
        let probe = ctx.probe.get();
        ctx.good.with(|g| {
            let set = g.as_ref()?;
            let q = set.quantities.get(&qn)?;
            let values = rules_doc::values_at(set, &qn, &probe);
            Some((
                q.combine,
                q.outputs.clone(),
                q.evaluate_each(&values).map_err(|e| e.to_string()),
                q.evaluate(&values).map_err(|e| e.to_string()),
            ))
        })
    });
    view! {
        <div class="field">
            <span>"Quantity"</span>
            <select prop:value=move || quantity.get().unwrap_or_default() on:change=move |ev| ctx.tested.set(Some(event_target_value(&ev)))>
                {move || names.get().into_iter().map(|n| view! { <option value=n.clone()>{n.clone()}</option> }).collect_view()}
            </select>
        </div>
        <h3>"Inputs"</h3>
        {move || inputs.get().into_iter().map(|(n, lo, hi, dflt)| {
            let n1 = n.clone();
            let n2 = n.clone();
            let value = Signal::derive(move || ctx.probe_value(&n1).unwrap_or(dflt));
            let step = 10f64.powf(((hi - lo) / 200.0).log10().floor());
            view! {
                <div class="test-row">
                    <div class="line"><span class="mono">{shown(&n)}</span><span class="spacer"></span><span class="mono">{move || num(value.get())}</span></div>
                    <input type="range" min=lo max=hi step=step prop:value=move || value.get().to_string()
                        on:input=move |ev| {
                            if let Some(v) = parse_num(&event_target_value(&ev)) {
                                ctx.set_probe(&n2, v);
                            }
                        }/>
                </div>
            }
        }).collect_view()}
        <h3>"Outputs"</h3>
        {move || match result.get() {
            None => view! { <p class="hint">"No rules that read yet."</p> }.into_any(),
            Some((combine, outputs, each, joined)) => match (each, joined) {
                (Ok(each), Ok(joined)) => view! {
                    <table class="rules-table compact">
                        <thead><tr><th></th>{outputs.iter().map(|o| view! { <th class="num">{o.clone()}</th> }).collect_view()}</tr></thead>
                        <tbody>
                            {each.into_iter().map(|(bn, out)| view! {
                                <tr><td>{bn}</td>{outputs.iter().map(|o| {
                                    let v = out.get(o).copied().unwrap_or(f64::NAN);
                                    let wins = joined.get(o).is_some_and(|j| (*j - v).abs() < 1e-12);
                                    view! { <td class="num" class:wins=wins>{num(v)}</td> }
                                }).collect_view()}</tr>
                            }).collect_view()}
                            <tr class="total"><td>{format!("{} →", if combine == openodd::rules::Combine::Max { "max" } else { "min" })}</td>
                                {outputs.iter().map(|o| view! { <td class="num">{num(joined.get(o).copied().unwrap_or(f64::NAN))}</td> }).collect_view()}
                            </tr>
                        </tbody>
                    </table>
                }.into_any(),
                (Err(e), _) | (_, Err(e)) => view! { <div class="error">{e}</div> }.into_any(),
            },
        }}
        <h3>"Response"</h3>
        <div class="field">
            <span>"As this input moves, the others held"</span>
            <select prop:value=move || x.get().unwrap_or_default() on:change=move |ev| ctx.x.set(Some(event_target_value(&ev)))>
                {move || inputs.get().into_iter().map(|(n, ..)| view! { <option value=n.clone()>{shown(&n)}</option> }).collect_view()}
            </select>
        </div>
        <ResponseChart quantity=quantity x=x/>
    }
}

/// A quantity's outputs as one input moves across its universe: the
/// combined output solid, each rule base's dashed.
#[component]
fn ResponseChart(quantity: Memo<Option<String>>, x: Memo<Option<String>>) -> impl IntoView {
    let ctx = rctx();
    let data = Memo::new(move |_| {
        let qn = quantity.get()?;
        let xn = x.get()?;
        let probe = ctx.probe.get();
        ctx.good.with(|g| {
            let set = g.as_ref()?;
            let q = set.quantities.get(&qn)?;
            let input = q
                .rule_bases
                .iter()
                .flat_map(|b| &b.inputs)
                .find(|i| i.name == xn)?;
            let (lo, hi) = (input.low, input.high);
            let base: IndexMap<String, f64> = rules_doc::values_at(set, &qn, &probe);
            let at = base.get(&xn).copied().unwrap_or((lo + hi) / 2.0);
            let n = 80;
            let mut xs = vec![];
            let mut joined: Vec<IndexMap<String, f64>> = vec![];
            let mut each: Vec<Vec<(String, IndexMap<String, f64>)>> = vec![];
            for k in 0..=n {
                let v = lo + (hi - lo) * k as f64 / n as f64;
                let mut values = base.clone();
                values.insert(xn.clone(), v);
                let (Ok(j), Ok(e)) = (q.evaluate(&values), q.evaluate_each(&values)) else {
                    continue;
                };
                xs.push(v);
                joined.push(j);
                each.push(e);
            }
            Some((
                q.outputs.clone(),
                lo,
                hi,
                at,
                xs,
                joined,
                each,
                q.rule_bases.len(),
            ))
        })
    });
    move || {
        let Some((outputs, lo, hi, at, xs, joined, each, bases)) = data.get() else {
            return view! { <p class="hint">"Nothing to draw."</p> }.into_any();
        };
        if xs.is_empty() {
            return view! { <p class="hint">"No rule fires along this input."</p> }.into_any();
        }
        let (w, h, top, bot) = (320.0, 190.0, 10.0, 160.0);
        let mut ymin = f64::INFINITY;
        let mut ymax = f64::NEG_INFINITY;
        for j in joined
            .iter()
            .chain(each.iter().flat_map(|e| e.iter().map(|(_, o)| o)))
        {
            for v in j.values() {
                ymin = ymin.min(*v);
                ymax = ymax.max(*v);
            }
        }
        if ymax.partial_cmp(&ymin) != Some(std::cmp::Ordering::Greater) {
            ymin -= 1.0;
            ymax += 1.0;
        }
        let pad = (ymax - ymin) * 0.06;
        let (ymin, ymax) = (ymin - pad, ymax + pad);
        let sx = move |v: f64| {
            if hi > lo {
                (v - lo) / (hi - lo) * w
            } else {
                0.0
            }
        };
        let sy = move |v: f64| bot - (v - ymin) / (ymax - ymin) * (bot - top);
        let path = |ys: Vec<f64>| {
            xs.iter()
                .zip(ys)
                .enumerate()
                .map(|(k, (x, y))| {
                    format!(
                        "{}{:.1},{:.1}",
                        if k == 0 { "M" } else { "L" },
                        sx(*x),
                        sy(y)
                    )
                })
                .collect::<String>()
        };
        let mut lines = vec![];
        if bases > 1 {
            for bi in 0..bases {
                for (k, o) in outputs.iter().enumerate() {
                    let ys: Vec<f64> = each.iter().map(|e| e[bi].1[o]).collect();
                    lines.push(
                        view! { <path class=format!("resp base o{}", k % 4) d=path(ys)/> }
                            .into_any(),
                    );
                }
            }
        }
        for (k, o) in outputs.iter().enumerate() {
            let ys: Vec<f64> = joined.iter().map(|j| j[o]).collect();
            lines.push(view! { <path class=format!("resp o{}", k % 4) d=path(ys)/> }.into_any());
        }
        let xticks = ticks(lo, hi, 4);
        let yticks = ticks(ymin, ymax, 4);
        view! {
            <svg class="response" viewBox=format!("-42 0 {} {h}", w + 54.0)>
                {yticks.into_iter().map(|t| view! {
                    <line class="grid-line" x1="0" x2=w y1=sy(t) y2=sy(t)/>
                    <text class="tick-label" x="-6" y=sy(t) + 4.0 text-anchor="end">{num(t)}</text>
                }).collect_view()}
                {xticks.into_iter().map(|t| view! {
                    <text class="tick-label" x=sx(t) y=bot + 16.0 text-anchor="middle">{num(t)}</text>
                }).collect_view()}
                {lines}
                <line class="probe-line" x1=sx(at) x2=sx(at) y1=top y2=bot/>
            </svg>
            <div class="legend-line">
                {outputs.iter().enumerate().map(|(k, o)| view! { <span><i class=format!("key o{}", k % 4)></i>{o.clone()}</span> }).collect_view()}
                {(bases > 1).then(|| view! { <span class="hint">"dashed: each rule base"</span> })}
            </div>
        }
        .into_any()
    }
}

#[component]
fn YamlTab() -> impl IntoView {
    let ctx = rctx();
    let draft = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let text = move || {
        draft
            .get()
            .unwrap_or_else(|| ctx.yaml.get().unwrap_or_else(|e| format!("# {e}")))
    };
    let apply = move |_| {
        if let Some(t) = draft.get() {
            match RulesDoc::from_yaml(&t) {
                Ok(doc) => {
                    ctx.doc.set(doc);
                    ctx.sel.set(RSel::File);
                    draft.set(None);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
        }
    };
    view! {
        <div class="yaml">
            <div class="hint">
                {move || if draft.with(Option::is_some) {
                    "Edited: apply to read it back into the editor."
                } else {
                    "Written by the editor. Type here to edit the YAML directly (comments are not kept)."
                }}
            </div>
            <textarea spellcheck="false" prop:value=text on:input=move |ev| draft.set(Some(event_target_value(&ev)))></textarea>
            <Show when=move || draft.with(Option::is_some)>
                <div class="actions">
                    <button class="button primary" on:click=apply>"Apply"</button>
                    <button class="button" on:click=move |_| { draft.set(None); error.set(None); }>"Discard"</button>
                </div>
            </Show>
            {move || error.get().map(|e| view! { <div class="error">{e}</div> })}
        </div>
    }
}

#[component]
fn DetailsTab() -> impl IntoView {
    let ctx = rctx();
    move || {
        match ctx.checked.get() {
        Err(e) => view! { <div class="error">{e}</div> }.into_any(),
        Ok(set) => view! {
            <p class="hint">"openodd's reading, the files it extends worked in."</p>
            {set.quantities.values().map(|q| view! {
                <div class="detail">
                    <div class="card-title"><Icon name="func"/>{q.name.clone()}
                        <span class="tag">{format!("combine {}", if q.combine == openodd::rules::Combine::Max { "max" } else { "min" })}</span>
                    </div>
                    <div class="hint">{format!("outputs: {}", q.outputs.join(", "))}</div>
                    <ul>
                        {q.rule_bases.iter().map(|b| view! {
                            <li><span class="mono">{b.name.clone()}</span>": "
                                {b.inputs.iter().map(|i| format!("{} [{}, {}] ({})", i.name, num(i.low), num(i.high), i.terms.keys().cloned().collect::<Vec<_>>().join(", "))).collect::<Vec<_>>().join("; ")}
                                {format!(" · {} rules", b.rules.len())}
                            </li>
                        }).collect_view()}
                    </ul>
                </div>
            }).collect_view()}
            <h3>"Files read"</h3>
            <ul>{set.files.iter().map(|f| view! { <li class="mono">{format!("{} {}", &f.sha256[..12], f.label)}</li> }).collect_view()}</ul>
        }.into_any(),
    }
    }
}
