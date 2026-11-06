//! The rules editor: a backend's rules ([docs/rules.md](../../../docs/rules.md)),
//! conditional distributions, edited as forms. The outline lists the
//! quantities and their parts; a part's form holds its inputs and the points
//! written on them, and the distribution at each point: drawn over the input
//! with each value draggable, and as a table. The panel tests the rules at
//! chosen inputs and a quantile, and draws a quantity's response. Every edit
//! is read back by openodd's own reader.

use indexmap::IndexMap;
use leptos::prelude::*;
use leptos::task::spawn_local;
use openodd::rules::{Family, Joined, RuleSet};
use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;

use crate::app::{event_target, unique, ModeSwitch};
use crate::doc::fresh_id;
use crate::icons::Icon;
use crate::io;
use crate::rules_doc::{
    self, num, short, tidy, AxisDoc, DistDoc, FixedDoc, InputDoc, ParamDoc, PartDoc, QuantityDoc,
    RulesDoc,
};

pub const SAMPLE: &str = include_str!("../../../examples/wasm-viewer/sample_rules.yaml");
/// The weather, and what drivers make of it: quantities reading quantities.
pub const CHAIN_SAMPLE: &str = include_str!("../../../examples/wasm-viewer/chain_rules.yaml");

/// What the middle shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RSel {
    File,
    Quantity(u64),
    Part(u64, u64),
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
    /// The quantile being tested.
    pub p: RwSignal<f64>,
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

    fn part_update(&self, q: u64, p: u64, f: impl FnOnce(&mut PartDoc)) {
        self.doc.update(|d| {
            if let Some(x) = d.part_mut(q, p) {
                f(x)
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

    /// The names of quantity `q` and part `p`.
    fn names(&self, q: u64, p: u64) -> Option<(String, String)> {
        self.doc.with(|d| {
            let qd = d.quantity(q)?;
            let pd = qd.parts.iter().find(|x| x.id == p)?;
            Some((qd.name.clone(), pd.name.clone()))
        })
    }

    /// Part `p` of `q` as openodd read it last, the extended files' included.
    fn read_part(&self, q: u64, p: u64) -> Option<openodd::rules::Part> {
        let (qn, pn) = self.names(q, p)?;
        self.good.with(|g| {
            g.as_ref()?
                .quantities
                .get(&qn)?
                .parts
                .iter()
                .find(|x| x.name == pn)
                .cloned()
        })
    }

    /// Quantity `name`'s distribution at the tested values.
    fn joined(&self, name: &str) -> Option<Joined> {
        let probe = self.probe.get();
        self.good.with(|g| {
            let set = g.as_ref()?;
            let at = rules_doc::values_at(set, name, &probe);
            set.joined(name, &at).ok()
        })
    }

    /// The value input `name` is tested at, for quantity `quantity`.
    fn tested_value(&self, quantity: &str, name: &str) -> f64 {
        self.probe_value(name).unwrap_or_else(|| {
            self.good.with(|g| {
                g.as_ref()
                    .and_then(|s| rules_doc::values_at(s, quantity, &[]).get(name).copied())
                    .unwrap_or(0.0)
            })
        })
    }
}

pub(crate) fn parse_num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn parse_points(s: &str) -> Option<Vec<f64>> {
    let mut out = vec![];
    for t in s.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        out.push(parse_num(t)?);
    }
    (!out.is_empty()).then_some(out)
}

/// How an input is named: by the ODD attribute that sets it, when asked and
/// known, else as the rules name it.
fn shown(name: &str) -> String {
    if rctx().odd_names.get() {
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

/// A part as openodd read it, as the form holds one.
fn part_doc(p: &openodd::rules::Part) -> PartDoc {
    PartDoc {
        id: fresh_id(),
        name: p.name.clone(),
        axes: Some(
            p.axes
                .iter()
                .map(|a| AxisDoc {
                    id: fresh_id(),
                    input: a.input.clone(),
                    points: a.points.clone(),
                })
                .collect(),
        ),
        dist: Some(DistDoc {
            family: p.family,
            params: p
                .family
                .params()
                .iter()
                .zip(&p.values)
                .map(|(n, v)| ParamDoc {
                    name: n.to_string(),
                    values: v.clone(),
                    constant: false,
                })
                .collect(),
            shape: None,
        }),
        text: p.text.clone(),
        fixed: if p.fixed.contains(&openodd::rules::Hold::ALL) {
            FixedDoc::All
        } else if p.fixed.is_empty() {
            FixedDoc::None
        } else {
            // Each but `Hold::ALL` (taken above) has a spec.
            FixedDoc::Some(
                p.fixed
                    .iter()
                    .filter_map(|h| h.spec(p.family, &p.axes))
                    .collect(),
            )
        },
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
        p: RwSignal::new(0.5),
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
            RSel::Quantity(q) | RSel::Part(q, _) => Some(q),
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
                        RSel::Part(q, p) => view! { <PartView q=q p=p/> }.into_any(),
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
pub fn RulesToolbar() -> impl IntoView {
    let ctx = rctx();
    let open = move |ev: leptos::ev::Event| {
        let input: HtmlInputElement = event_target(&ev);
        spawn_local(async move {
            if let Some((name, text)) = io::read_files(&input).await.into_iter().next() {
                ctx.open_text(&name, &text);
            }
        });
    };
    let example = move |ev: leptos::ev::Event| {
        match event_target_value(&ev).as_str() {
            "weather" => {
                ctx.open_text("sample_rules.yaml", SAMPLE);
            }
            "chain" => {
                ctx.open_text("chain_rules.yaml", CHAIN_SAMPLE);
            }
            "empty" => {
                ctx.open_text("rules.yaml", "version: 2\nquantities: {}\n");
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
                <option value="chain">"Chain: weather, then drivers' speed and gap"</option>
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
pub fn ErrorCard() -> impl IntoView {
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

pub fn add_quantity() {
    let ctx = rctx();
    let mut sel = None;
    ctx.doc.update(|d| {
        let taken: Vec<String> = d.quantities.iter().map(|q| q.name.clone()).collect();
        let part = PartDoc::new("part".into(), "x".into());
        let q = QuantityDoc {
            id: fresh_id(),
            name: unique("quantity", &taken),
            combine: None,
            parts: vec![part],
            text: String::new(),
        };
        sel = Some(RSel::Part(q.id, q.parts[0].id));
        d.quantities.push(q);
    });
    if let Some(s) = sel {
        ctx.sel.set(s);
    }
}

pub fn add_part(q: u64) {
    let ctx = rctx();
    let mut sel = None;
    ctx.doc.update(|d| {
        let Some(qd) = d.quantity_mut(q) else { return };
        let taken: Vec<String> = qd.parts.iter().map(|b| b.name.clone()).collect();
        let p = PartDoc::new(unique("part", &taken), "x".into());
        sel = Some(RSel::Part(q, p.id));
        qd.parts.push(p);
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
                    let parts: Vec<(u64, String)> =
                        q.parts.iter().map(|b| (b.id, b.name.clone())).collect();
                    (q.id, q.name.clone(), parts)
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
                {move || tree.get().into_iter().map(|(q, name, parts)| {
                    view! {
                        <li class="row group" class:selected=move || selected(RSel::Quantity(q)) on:click=move |_| ctx.sel.set(RSel::Quantity(q))>
                            <span class="icon"><Icon name="func"/></span>
                            <span class="name">{name}</span>
                            <button class="mini add" title="Add a part" on:click=move |ev| {
                                ev.stop_propagation();
                                add_part(q);
                            }><Icon name="plus"/></button>
                        </li>
                        {parts.into_iter().map(|(p, pn)| view! {
                            <li class="row" style="padding-left: 32px" class:selected=move || selected(RSel::Part(q, p)) on:click=move |_| ctx.sel.set(RSel::Part(q, p))>
                                <span class="icon"><Icon name="graph"/></span>
                                <span class="name">{pn}</span>
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
                .map(|q| (q.id, q.name.clone(), q.parts.len()))
                .collect::<Vec<_>>()
        })
    });
    let input_ids = Memo::new(move |_| {
        ctx.doc
            .with(|d| d.inputs.iter().map(|i| i.id).collect::<Vec<_>>())
    });
    let add_input = move |_| {
        ctx.doc.update(|d| {
            let taken: Vec<String> = d.inputs.iter().map(|i| i.name.clone()).collect();
            d.inputs.push(InputDoc {
                id: fresh_id(),
                name: unique("input", &taken),
                default: None,
                text: String::new(),
                concept: None,
                unit: String::new(),
            });
        })
    };
    view! {
        <div class="editor-head">
            <div class="big-icon root"><Icon name="doc"/></div>
            <div class="head-text">
                <div class="crumbs"><span class="last">{move || ctx.name.get()}</span></div>
                <div class="kind-line">"A backend's rules: quantities, each one or more conditional distributions"</div>
            </div>
        </div>
        <div class="card">
            <div class="card-title"><Icon name="func"/>"Quantities"</div>
            <p class="hint">"A quantity is what a backend reads a value of (the road's grip, how far drivers see). Each of its parts gives a distribution of it, given some inputs; a run draws the value at a quantile, and " <code>"combine"</code> " joins the parts' values."</p>
            <ul class="list">
                {move || quantities.get().into_iter().map(|(q, n, k)| view! {
                    <li class="line">
                        <button class="pill" on:click=move |_| ctx.sel.set(RSel::Quantity(q))><Icon name="func"/>{n}</button>
                        <span class="hint">{format!("{k} part{}", if k == 1 { "" } else { "s" })}</span>
                    </li>
                }).collect_view()}
            </ul>
            <div class="actions">
                <button class="button subtle" on:click=move |_| add_quantity()><Icon name="plus"/>"Add a quantity"</button>
            </div>
        </div>
        <div class="card">
            <div class="card-title"><Icon name="ruler"/>"Inputs"</div>
            <p class="hint">"The value an input reads when the caller gives none, and a line saying what it is. An input not listed here must be given."</p>
            <table class="terms-table">
                <thead><tr><th>"Input"</th><th>"Default"</th><th>"Says"</th><th></th></tr></thead>
                <tbody>
                    <For each=move || input_ids.get() key=|id| *id children=move |i| view! { <InputRow i=i/> }/>
                </tbody>
            </table>
            <div class="actions">
                <button class="button subtle" on:click=add_input><Icon name="plus"/>"Describe an input"</button>
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

#[component]
fn InputRow(i: u64) -> impl IntoView {
    let ctx = rctx();
    let input = Memo::new(move |_| {
        ctx.doc
            .with(|d| d.inputs.iter().find(|x| x.id == i).cloned())
    });
    let update = move |f: Box<dyn FnOnce(&mut InputDoc)>| {
        ctx.doc.update(|d| {
            if let Some(x) = d.inputs.iter_mut().find(|x| x.id == i) {
                f(x)
            }
        })
    };
    view! {
        <tr>
            <td><input class="term-name" prop:value=move || input.with(|x| x.as_ref().map(|x| x.name.clone()).unwrap_or_default())
                on:change=move |ev| {
                    let n = event_target_value(&ev).trim().to_string();
                    if n.is_empty() { return; }
                    ctx.doc.update(|d| {
                        if let Some(old) = d.inputs.iter().find(|x| x.id == i).map(|x| x.name.clone()) {
                            d.rename_input(&old, &n);
                        }
                    });
                }/></td>
            <td><input class="corner" inputmode="decimal" placeholder="none" prop:value=move || input.with(|x| x.as_ref().and_then(|x| x.default).map(num).unwrap_or_default())
                on:change=move |ev| { let v = parse_num(&event_target_value(&ev)); update(Box::new(move |x| x.default = v)); }/></td>
            <td><input prop:value=move || input.with(|x| x.as_ref().map(|x| x.text.clone()).unwrap_or_default())
                on:change=move |ev| { let t = event_target_value(&ev); update(Box::new(move |x| x.text = t)); }/></td>
            <td><button class="mini danger" title="Forget the input" on:click=move |_| ctx.doc.update(|d| d.inputs.retain(|x| x.id != i))><Icon name="x"/></button></td>
        </tr>
    }
}

// ---------------------------------------------------------------------------
// A quantity
// ---------------------------------------------------------------------------

/// A table of each part's and the quantity's values at the tested inputs.
fn summary_table(name: &str, j: &Joined, p: f64, link: Option<u64>) -> AnyView {
    let ctx = rctx();
    let deciding = j.deciding(p).to_string();
    let rows = j
        .parts
        .iter()
        .map(|(pn, d)| {
            let (lo, hi) = d.support();
            let id = link.and_then(|q| {
                ctx.doc.with(|doc| {
                    doc.quantity(q)?
                        .parts
                        .iter()
                        .find(|x| x.name == *pn)
                        .map(|x| x.id)
                })
            });
            let wins = *pn == deciding && j.parts.len() > 1;
            let label = pn.clone();
            view! {
                <tr>
                    <td>{match (link, id) {
                        (Some(q), Some(pid)) => view! { <button class="pill" on:click=move |_| ctx.sel.set(RSel::Part(q, pid))><Icon name="graph"/>{label.clone()}</button> }.into_any(),
                        _ => view! { <span>{label.clone()}</span> }.into_any(),
                    }}</td>
                    <td class="num">{short(lo)}</td>
                    <td class="num">{short(hi)}</td>
                    <td class="num">{short(d.quantile(0.5))}</td>
                    <td class="num" class:wins=wins>{short(d.quantile(p))}</td>
                </tr>
            }
        })
        .collect_view();
    let (lo, hi) = j.support();
    view! {
        <table class="rules-table">
            <thead><tr><th>"Part"</th><th class="num">"low"</th><th class="num">"high"</th><th class="num">"median"</th><th class="num">{format!("at p = {}", num(p))}</th></tr></thead>
            <tbody>
                {rows}
                <tr class="total">
                    <td>{format!("{name} ({})", j.combine.name())}</td>
                    <td class="num">{short(lo)}</td>
                    <td class="num">{short(hi)}</td>
                    <td class="num">{short(j.quantile(0.5))}</td>
                    <td class="num">{short(j.quantile(p))}</td>
                </tr>
            </tbody>
        </table>
    }
    .into_any()
}

#[component]
pub fn QuantityView(q: u64) -> impl IntoView {
    let ctx = rctx();
    let quantity = Memo::new(move |_| ctx.doc.with(|d| d.quantity(q).cloned()));
    let rename = move |ev: leptos::ev::Event| {
        let n = event_target_value(&ev).trim().to_string();
        if n.is_empty() {
            return;
        }
        ctx.doc.update(|d| {
            if let Some(old) = d.quantity(q).map(|x| x.name.clone()) {
                d.rename_quantity(&old, &n);
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
    move || {
        let Some(qd) = quantity.get() else {
            return view! { <p class="hint">"This quantity is gone."</p> }.into_any();
        };
        let name = qd.name.clone();
        let n2 = name.clone();
        view! {
            <div class="editor-head">
                <div class="big-icon"><Icon name="func"/></div>
                <div class="head-text">
                    <div class="crumbs"><span class="last">{qd.name.clone()}</span></div>
                    <div class="kind-line">{format!("quantity · {} part{}", qd.parts.len(), if qd.parts.len() == 1 { "" } else { "s" })}</div>
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
                        <span>"Join the parts' values by"</span>
                        <div class="segmented">
                            {combine_button(None, "min (default)")}
                            {combine_button(Some("min"), "min")}
                            {combine_button(Some("max"), "max")}
                        </div>
                    </div>
                </div>
                <label class="field"><span>"Says"</span>
                    <input prop:value=qd.text.clone() on:change=move |ev| {
                        let t = event_target_value(&ev);
                        ctx.doc.update(|d| if let Some(x) = d.quantity_mut(q) { x.text = t });
                    }/>
                </label>
                <p class="hint">"The parts move together: each gives its value at the same quantile, and " <b>"min"</b> " takes the least (the shortest view of fog, rain and night), " <b>"max"</b> " the greatest."</p>
            </div>
            <div class="card">
                <div class="card-title"><Icon name="graph"/>"Parts at the tested values"</div>
                {move || match ctx.joined(&n2) {
                    None => view! { <p class="hint">"Not read yet."</p> }.into_any(),
                    Some(j) => view! {
                        {summary_table(&name, &j, ctx.p.get(), Some(q))}
                        <p class="hint">"Bold: the part that gives the value at the tested quantile. Set the values and the quantile in the panel's test drive."</p>
                    }.into_any(),
                }}
                <div class="actions">
                    <button class="button subtle" on:click=move |_| add_part(q)><Icon name="plus"/>"Add a part"</button>
                </div>
            </div>
        }
        .into_any()
    }
}

// ---------------------------------------------------------------------------
// A part
// ---------------------------------------------------------------------------

#[component]
pub fn PartView(q: u64, p: u64) -> impl IntoView {
    let ctx = rctx();
    let part = Memo::new(move |_| ctx.doc.with(|d| d.part(q, p).cloned()));
    let qname = Memo::new(move |_| {
        ctx.doc
            .with(|d| d.quantity(q).map(|x| x.name.clone()).unwrap_or_default())
    });
    let name_value = move || part.with(|x| x.as_ref().map(|x| x.name.clone()).unwrap_or_default());
    let rename = move |ev: leptos::ev::Event| {
        let n = event_target_value(&ev).trim().to_string();
        if !n.is_empty() {
            ctx.part_update(q, p, |x| x.name = n);
        }
    };
    let remove = move |_| {
        ctx.doc.update(|d| {
            if let Some(x) = d.quantity_mut(q) {
                x.parts.retain(|y| y.id != p);
            }
        });
        ctx.sel.set(RSel::Quantity(q));
    };
    let has_axes = Memo::new(move |_| part.with(|x| x.as_ref().map(|x| x.axes.is_some())));
    let has_dist = Memo::new(move |_| part.with(|x| x.as_ref().map(|x| x.dist.is_some())));
    let take = move |_| {
        if let Some(read) = ctx.read_part(q, p) {
            let full = part_doc(&read);
            ctx.part_update(q, p, |x| {
                if x.axes.is_none() {
                    x.axes = full.axes.clone();
                }
                if x.dist.is_none() {
                    x.dist = full.dist.clone();
                }
            });
        }
    };
    view! {
        <div class="editor-head">
            <div class="big-icon"><Icon name="graph"/></div>
            <div class="head-text">
                <div class="crumbs">
                    <span>{move || qname.get()}</span>
                    <span class="sep">"›"</span>
                    <span class="last">{name_value}</span>
                </div>
                <div class="kind-line">"part: a distribution of the quantity, given its inputs"</div>
            </div>
            <div class="actions">
                <button class="mini danger" on:click=remove><Icon name="trash"/>"Remove"</button>
            </div>
        </div>
        <div class="card">
            <div class="grid2">
                <label class="field"><span>"Name"</span><input prop:value=name_value on:change=rename/></label>
                <label class="field"><span>"Says"</span>
                    <input prop:value=move || part.with(|x| x.as_ref().map(|x| x.text.clone()).unwrap_or_default())
                        on:change=move |ev| { let t = event_target_value(&ev); ctx.part_update(q, p, |x| x.text = t); }/>
                </label>
            </div>
        </div>
        {move || (has_axes.get() == Some(false) || has_dist.get() == Some(false)).then(|| view! {
            <div class="card">
                <p class="hint">{move || match (has_axes.get(), has_dist.get()) {
                    (Some(false), Some(false)) => "This file names neither the points nor the distribution: the file it extends gives them.",
                    (Some(false), _) => "This file does not name the points: the file it extends gives them.",
                    _ => "This file does not name the distribution: the file it extends gives it.",
                }}</p>
                <button class="button subtle" on:click=take><Icon name="plus"/>"Write them here to change them"</button>
            </div>
        })}
        <Show when=move || has_axes.get() == Some(true)>
            <h3>"Inputs and points"</h3>
            <AxesCard q=q p=p part=part/>
        </Show>
        <Show when=move || has_axes.get() == Some(true) && has_dist.get() == Some(true)>
            <h3>"Distribution"</h3>
            <DistCard q=q p=p part=part/>
        </Show>
        <h3>"Fit holds"</h3>
        <HoldsCard q=q p=p part=part/>
    }
}

/// What a fit to data holds of the part as written (`fixed:`): all of it,
/// a parameter at every point, or values at points.
#[component]
fn HoldsCard(q: u64, p: u64, part: Memo<Option<PartDoc>>) -> impl IntoView {
    let ctx = rctx();
    // The family: the document's, else the one an extended file gives.
    let family = Memo::new(move |_| {
        part.with(|x| x.as_ref().and_then(|x| x.dist.as_ref().map(|d| d.family)))
            .or_else(|| ctx.read_part(q, p).map(|x| x.family))
    });
    let fixed =
        Memo::new(move |_| part.with(|x| x.as_ref().map(|x| x.fixed.clone()).unwrap_or_default()));
    let set_fixed = move |f: FixedDoc| ctx.part_update(q, p, |x| x.fixed = f);
    let list = move || match fixed.get() {
        FixedDoc::Some(l) => l,
        _ => vec![],
    };
    let toggle_all = move |ev: leptos::ev::Event| {
        // Off, it holds nothing: "all" replaced whatever it held before.
        set_fixed(if event_target_checked(&ev) {
            FixedDoc::All
        } else {
            FixedDoc::None
        });
    };
    let toggle = move |name: &'static str, on: bool| {
        let mut l = list();
        l.retain(|s| s != name);
        if on {
            l.push(name.to_string());
        }
        set_fixed(l.into());
    };
    // The entries at points: those that are not a parameter alone.
    let at_points = move || {
        list()
            .into_iter()
            .filter(|s| s.contains('@'))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let set_points = move |ev: leptos::ev::Event| {
        let mut l: Vec<String> = list().into_iter().filter(|s| !s.contains('@')).collect();
        l.extend(
            event_target_value(&ev)
                .split_whitespace()
                .map(str::to_string),
        );
        set_fixed(l.into());
    };
    let params = move || {
        let Some(family) = family.get() else {
            return vec![];
        };
        let all = fixed.get() == FixedDoc::All;
        let held = list();
        family
            .params()
            .iter()
            .enumerate()
            .map(|(k, &name)| {
                // What a fit never estimates (a normal's truncation) is held.
                let always = !family.fitted_params().contains(&k);
                let checked = all || always || held.iter().any(|s| s == name);
                view! {
                    <label class="check" title=if always { "A fit always holds it: a normal's truncation is the physical range" } else { "Hold it at every point" }>
                        <input type="checkbox" prop:checked=checked disabled=all || always
                            on:change=move |ev| toggle(name, event_target_checked(&ev))/>
                        {name}
                    </label>
                }
            })
            .collect::<Vec<_>>()
    };
    view! {
        <div class="card holds-card">
            <p class="hint">
                "A fit to data estimates every parameter at every point; what is held here stays as written, "
                "and the rest is fitted given it."
            </p>
            <label class="check">
                <input type="checkbox" prop:checked=move || fixed.get() == FixedDoc::All on:change=toggle_all/>
                <b>"Hold all of it"</b>
            </label>
            <div class="holds-params">{params}</div>
            <label class="field">
                <span>"At points (space-separated: mean@water=0, @water=100)"</span>
                <input prop:value=at_points on:change=set_points
                    disabled=move || fixed.get() == FixedDoc::All
                    placeholder="mean@water=0"/>
            </label>
        </div>
    }
}

#[component]
fn AxesCard(q: u64, p: u64, part: Memo<Option<PartDoc>>) -> impl IntoView {
    let ctx = rctx();
    let axes = move || part.with(|x| x.as_ref().and_then(|x| x.axes.clone()).unwrap_or_default());
    let add_axis = move |_| {
        ctx.part_update(q, p, |x| {
            if let Some(axes) = &mut x.axes {
                let taken: Vec<String> = axes.iter().map(|a| a.input.clone()).collect();
                axes.push(AxisDoc {
                    id: fresh_id(),
                    input: unique("input", &taken),
                    points: vec![0.0],
                });
            }
        })
    };
    let remove_axis = move |k: usize| {
        ctx.part_update(q, p, |x| {
            // Keep the values at the axis' first point.
            if let Some(first) = x.axes.as_ref().and_then(|a| a.get(k)).map(|a| a.points[0]) {
                x.set_points(k, vec![first]);
            }
            if let Some(axes) = &mut x.axes {
                axes.remove(k);
            }
        })
    };
    view! {
        <div class="card">
            <p class="hint">"The inputs the part reads, and the points of each the distribution is written at, increasing. Between the points each value is interpolated linearly; beyond them it holds. Changing the points reads the values at the new points from the old ones."</p>
            <table class="terms-table">
                <thead><tr><th>"Input"</th><th>"Points"</th><th></th></tr></thead>
                <tbody>
                    {move || {
                        let all = axes();
                        let count = all.len();
                        all.into_iter().enumerate().map(|(k, a)| {
                            let other = other_name(&a.input);
                            let points_text = a.points.iter().map(|v| num(*v)).collect::<Vec<String>>().join(", ");
                            view! {
                                <tr>
                                    <td>
                                        <input class="term-name" prop:value=a.input.clone() on:change=move |ev| {
                                            let n = event_target_value(&ev).trim().to_string();
                                            if !n.is_empty() {
                                                ctx.part_update(q, p, |x| {
                                                    if let Some(axes) = &mut x.axes {
                                                        axes[k].input = n;
                                                    }
                                                });
                                            }
                                        }/>
                                        {other.map(|o| view! { <div class="hint mono">{o}</div> })}
                                    </td>
                                    <td>
                                        <input class="points mono" prop:value=points_text
                                            on:change=move |ev| {
                                                match parse_points(&event_target_value(&ev)) {
                                                    Some(points) if points.windows(2).all(|w| w[0] < w[1]) => {
                                                        ctx.part_update(q, p, |x| x.set_points(k, points));
                                                    }
                                                    _ => ctx.notice.set(Some((false, "Points are numbers, increasing, separated by commas".into()))),
                                                }
                                            }/>
                                    </td>
                                    <td>
                                        {(count > 1).then(|| view! {
                                            <button class="mini danger" title="Stop reading this input" on:click=move |_| remove_axis(k)><Icon name="x"/></button>
                                        })}
                                    </td>
                                </tr>
                            }
                        }).collect_view()
                    }}
                </tbody>
            </table>
            <div class="actions">
                <button class="button subtle" on:click=add_axis><Icon name="plus"/>"Read another input"</button>
            </div>
        </div>
    }
}

/// A dragged parameter kept in order with the others at its point: `low` not
/// above `mode` or `high`, and so on.
fn clamp_param(family: Family, values: &[f64], j: usize, v: f64) -> f64 {
    let names = family.params();
    let get = |n: &str| names.iter().position(|x| *x == n).map(|i| values[i]);
    match names[j] {
        "low" => v
            .min(get("mode").unwrap_or(f64::INFINITY))
            .min(get("high").unwrap_or(f64::INFINITY)),
        "high" => v
            .max(get("mode").unwrap_or(f64::NEG_INFINITY))
            .max(get("low").unwrap_or(f64::NEG_INFINITY)),
        "mode" => v.clamp(
            get("low").unwrap_or(v),
            get("high").unwrap_or(v).max(get("low").unwrap_or(v)),
        ),
        "sd" => v.max(f64::EPSILON),
        _ => v,
    }
}

#[component]
fn DistCard(q: u64, p: u64, part: Memo<Option<PartDoc>>) -> impl IntoView {
    let ctx = rctx();
    let family = Memo::new(move |_| {
        part.with(|x| x.as_ref().and_then(|x| x.dist.as_ref()).map(|d| d.family))
    });
    let family_button = move |f: Family| {
        view! {
            <button class:active=move || family.get() == Some(f) on:click=move |_| ctx.part_update(q, p, |x| x.set_family(f))>{f.name()}</button>
        }
    };
    view! {
        <div class="card">
            <div class="card-head">
                <div class="segmented">
                    {family_button(Family::Uniform)}
                    {family_button(Family::Triangular)}
                    {family_button(Family::Normal)}
                </div>
                <span class="hint">{move || match family.get() {
                    Some(Family::Uniform) => "uniform: any value from low to high, equally",
                    Some(Family::Triangular) => "triangular: from low to high, most often at mode",
                    Some(Family::Normal) => "normal: around mean, sd wide, cut to low..high",
                    None => "",
                }}</span>
            </div>
            <PartChart q=q p=p part=part/>
            <BucketNote part=part/>
            <ValuesTable q=q p=p part=part/>
        </div>
    }
}

const CW: f64 = 600.0;
const CTOP: f64 = 14.0;
const CBOT: f64 = 176.0;

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

/// One sample of a part over its first input: (x, low, median, at p, high).
type Sample = (f64, f64, f64, f64, f64);

/// A part over its first input, the others held at the tested values: the
/// range low..high, the median and the value at the tested quantile, and,
/// for a part over one input, each value written at a point as a handle to
/// drag up or down.
#[component]
fn PartChart(q: u64, p: u64, part: Memo<Option<PartDoc>>) -> impl IntoView {
    let ctx = rctx();
    let svg_ref = NodeRef::<leptos::svg::Svg>::new();
    let dragging = RwSignal::new(None::<(usize, usize)>);
    let frozen = RwSignal::new(None::<(f64, f64)>);
    let qname = Memo::new(move |_| {
        ctx.doc
            .with(|d| d.quantity(q).map(|x| x.name.clone()).unwrap_or_default())
    });
    // The part as openodd holds it, from the form when it agrees in size.
    let core = Memo::new(move |_| part.with(|x| x.as_ref().and_then(PartDoc::core)));
    let input = Memo::new(move |_| {
        core.with(|c| {
            c.as_ref()
                .and_then(|c| c.axes.first().map(|a| a.input.clone()))
        })
    });
    let x_range = Memo::new(move |_| {
        core.with(|c| {
            let a = c.as_ref()?.axes.first()?;
            let (lo, hi) = (a.points[0], *a.points.last()?);
            Some(if hi > lo {
                let pad = (hi - lo) * 0.08;
                (lo - pad, hi + pad)
            } else {
                (lo - 1.0, hi + 1.0)
            })
        })
    });
    let samples = Memo::new(move |_| -> Vec<Sample> {
        let Some((lo, hi)) = x_range.get() else {
            return vec![];
        };
        let qn = qname.get();
        let pq = ctx.p.get();
        core.with(|c| {
            let Some(c) = c else { return vec![] };
            let base: Vec<f64> = c
                .axes
                .iter()
                .map(|a| ctx.tested_value(&qn, &a.input))
                .collect();
            let mut xs: Vec<f64> = (0..=120)
                .map(|k| lo + (hi - lo) * k as f64 / 120.0)
                .collect();
            xs.extend(c.axes[0].points.iter().copied());
            xs.sort_by(f64::total_cmp);
            xs.dedup();
            xs.into_iter()
                .map(|x| {
                    let mut v = base.clone();
                    v[0] = x;
                    let d = c.dist(&v);
                    let (l, h) = d.support();
                    (x, l, d.quantile(0.5), d.quantile(pq), h)
                })
                .collect()
        })
    });
    let y_range = Memo::new(move |_| {
        if let Some(r) = frozen.get() {
            return r;
        }
        let s = samples.get();
        let mut lo = s.iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
        let mut hi = s.iter().map(|x| x.4).fold(f64::NEG_INFINITY, f64::max);
        if !lo.is_finite() || !hi.is_finite() {
            return (0.0, 1.0);
        }
        if hi <= lo {
            lo -= 1.0;
            hi += 1.0;
        }
        let pad = (hi - lo) * 0.12;
        (lo - pad, hi + pad)
    });
    let sx = move |v: f64| {
        let (lo, hi) = x_range.get().unwrap_or((0.0, 1.0));
        (v - lo) / (hi - lo) * CW
    };
    let sy = move |v: f64| {
        let (lo, hi) = y_range.get();
        CBOT - (v - lo) / (hi - lo) * (CBOT - CTOP)
    };
    let one_axis = Memo::new(move |_| core.with(|c| c.as_ref().map(|c| c.axes.len()) == Some(1)));
    let to_value = move |client_y: f64| -> Option<f64> {
        let el = svg_ref.get()?;
        let el: &web_sys::Element = el.unchecked_ref();
        let rect = el.get_bounding_client_rect();
        if rect.height() <= 0.0 {
            return None;
        }
        let y = (client_y - rect.top()) / rect.height() * (CBOT + 50.0);
        let (lo, hi) = y_range.get_untracked();
        let v = lo + (CBOT - y) / (CBOT - CTOP) * (hi - lo);
        let step = 10f64.powf(((hi - lo) / 300.0).log10().floor());
        Some(tidy((v / step).round() * step))
    };
    let pointer_down = move |ev: web_sys::PointerEvent| {
        let target: web_sys::Element = ev.target().unwrap().unchecked_into();
        if let Some((j, k)) = target.get_attribute("data-handle").and_then(|s| {
            let (j, k) = s.split_once(':')?;
            Some((j.parse().ok()?, k.parse().ok()?))
        }) {
            frozen.set(Some(y_range.get_untracked()));
            dragging.set(Some((j, k)));
            if let Some(el) = svg_ref.get() {
                let el: &web_sys::Element = el.unchecked_ref();
                let _ = el.set_pointer_capture(ev.pointer_id());
            }
            ev.prevent_default();
        } else if let (Some(name), Some(el), Some((lo, hi))) = (
            input.get_untracked(),
            svg_ref.get(),
            x_range.get_untracked(),
        ) {
            // A click on the chart tests that value of the input.
            let el: &web_sys::Element = el.unchecked_ref();
            let rect = el.get_bounding_client_rect();
            if rect.width() > 0.0 {
                let x = (ev.client_x() as f64 - rect.left()) / rect.width() * (CW + 70.0) - 50.0;
                let v = (lo + x / CW * (hi - lo)).clamp(lo, hi);
                let step = 10f64.powf(((hi - lo) / 200.0).log10().floor());
                ctx.set_probe(&name, tidy((v / step).round() * step));
            }
        }
    };
    let pointer_move = move |ev: web_sys::PointerEvent| {
        if let (Some((j, k)), Some(v)) = (dragging.get_untracked(), to_value(ev.client_y() as f64))
        {
            ctx.part_update(q, p, |x| {
                if let Some(d) = &mut x.dist {
                    let at: Vec<f64> = d.params.iter().map(|p| p.values[k]).collect();
                    let v = clamp_param(d.family, &at, j, v);
                    let param = &mut d.params[j];
                    param.values[k] = v;
                    param.constant = false;
                }
            });
        }
    };
    let pointer_up = move |_ev: web_sys::PointerEvent| {
        dragging.set(None);
        frozen.set(None);
    };
    let band = move || {
        let s = samples.get();
        if s.is_empty() {
            return ().into_any();
        }
        let top: Vec<String> = s
            .iter()
            .map(|x| format!("{:.1},{:.1}", sx(x.0), sy(x.4)))
            .collect();
        let bottom: Vec<String> = s
            .iter()
            .rev()
            .map(|x| format!("{:.1},{:.1}", sx(x.0), sy(x.1)))
            .collect();
        let line = |f: fn(&Sample) -> f64| {
            s.iter()
                .enumerate()
                .map(|(i, x)| {
                    format!(
                        "{}{:.1},{:.1}",
                        if i == 0 { "M" } else { "L" },
                        sx(x.0),
                        sy(f(x))
                    )
                })
                .collect::<String>()
        };
        view! {
            <path class="band-area" d=format!("M{} L{} Z", top.join(" L"), bottom.join(" L"))/>
            <path class="band-edge" d=line(|x| x.4)/>
            <path class="band-edge" d=line(|x| x.1)/>
            <path class="median-line" d=line(|x| x.2)/>
            <path class="atp-line" d=line(|x| x.3)/>
        }
        .into_any()
    };
    let handles = move || {
        if !one_axis.get() {
            return ().into_any();
        }
        let Some(doc) = part.get() else {
            return ().into_any();
        };
        let (Some(axes), Some(d)) = (doc.axes, doc.dist) else {
            return ().into_any();
        };
        let points = axes[0].points.clone();
        d.params
            .iter()
            .enumerate()
            .filter(|(_, prm)| prm.name != "sd")
            .flat_map(|(j, prm)| {
                let cls = format!("handle-dot p-{}", prm.name);
                let name = prm.name.clone();
                points
                    .iter()
                    .zip(prm.values.clone())
                    .enumerate()
                    .map(move |(k, (x, v))| {
                        let active = move || dragging.get() == Some((j, k));
                        let title = format!("{name} at {}: {}", num(*x), num(v));
                        view! {
                            <circle class=cls.clone() class:active=active cx=sx(*x) cy=sy(v) r="6" data-handle=format!("{j}:{k}")>
                                <title>{title}</title>
                            </circle>
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect_view()
            .into_any()
    };
    let grid_view = move || {
        let (xlo, xhi) = x_range.get().unwrap_or((0.0, 1.0));
        let (ylo, yhi) = y_range.get();
        let xt = ticks(xlo, xhi, 6)
            .into_iter()
            .map(|t| {
                view! {
                    <line class="grid-line" x1=sx(t) x2=sx(t) y1=CTOP y2=CBOT/>
                    <text class="tick-label" x=sx(t) y=CBOT + 15.0 text-anchor="middle">{num(t)}</text>
                }
            })
            .collect_view();
        let yt = ticks(ylo, yhi, 5)
            .into_iter()
            .map(|t| {
                view! {
                    <line class="grid-line" x1="0" x2=CW y1=sy(t) y2=sy(t)/>
                    <text class="tick-label" x="-6" y=sy(t) + 4.0 text-anchor="end">{num(t)}</text>
                }
            })
            .collect_view();
        view! { {xt}{yt} }
    };
    let marker = move || {
        let name = input.get()?;
        let v = ctx.tested_value(&qname.get(), &name);
        let x = sx(v);
        Some(view! {
            <g class="probe">
                <line x1=x x2=x y1=CTOP y2=CBOT/>
                <text x=x + 4.0 y=CTOP + 10.0>{num(v)}</text>
            </g>
        })
    };
    // The ODD's buckets, where the binding and the manifest say which
    // attribute sets this input: what each label writes into it.
    let bands = Memo::new(move |_| {
        let n = input.get()?;
        let link = crate::binding_app::bctx().link_of_input(&n)?;
        let spans = link.spans();
        (!spans.is_empty()).then_some(spans)
    });
    let band_view = move || {
        let spans = bands.get()?;
        let (lo, hi) = x_range.get()?;
        let y = CBOT + 26.0;
        let placed: Vec<(String, f64, f64)> = spans
            .into_iter()
            .map(|(label, a, b)| {
                let x0 = sx(a.clamp(lo, hi));
                (label, x0, (sx(b.clamp(lo, hi)) - x0).max(3.0))
            })
            .collect();
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
    view! {
        <svg
            node_ref=svg_ref
            class="membership"
            viewBox=format!("-50 0 {} {}", CW + 70.0, CBOT + 50.0)
            preserveAspectRatio="xMidYMid meet"
            on:pointerdown=pointer_down
            on:pointermove=pointer_move
            on:pointerup=pointer_up
            on:pointercancel=pointer_up
        >
            <rect class="plot-bg" x="0" y=CTOP width=CW height=CBOT - CTOP/>
            {grid_view}
            {band_view}
            {band}
            {marker}
            {handles}
        </svg>
        <div class="legend-line">
            <span><i class="key band-key"></i>"low – high"</span>
            <span><i class="key median-key"></i>"median"</span>
            <span><i class="key atp-key"></i>{move || format!("at the tested quantile ({})", num(ctx.p.get()))}</span>
            <span class="hint">{move || if one_axis.get() {
                "Drag a point's value up or down; click the chart to test an input value."
            } else {
                "Over its first input, the others held at the tested values."
            }}</span>
        </div>
    }
}

/// What each of the ODD's buckets gives through this part, where the
/// binding and the manifest say which attribute sets its first input.
#[component]
fn BucketNote(part: Memo<Option<PartDoc>>) -> impl IntoView {
    move || {
        let core = part.with(|x| x.as_ref().and_then(PartDoc::core))?;
        let input = core.axes.first()?.input.clone();
        let link = crate::binding_app::bctx().link_of_input(&input)?;
        let spans = link.spans();
        if spans.is_empty() {
            return None;
        }
        let held: Vec<f64> = core
            .axes
            .iter()
            .map(|a| (a.points[0] + a.points[a.points.len() - 1]) / 2.0)
            .collect();
        let rows = crate::binding::bucket_ranges(&spans, &core, &input, &held);
        let key = link.knob.map(|k| k.key).unwrap_or_default();
        Some(view! {
            <div class="alignment">
                <span class="hint">{format!("{} sets {input} through {key}; each bucket gives:", link.attribute)}</span>
                {rows.into_iter().map(|(label, a, b, lo, hi)| {
                    let span = if a == b { num(a) } else { format!("{}–{}", num(a), num(b)) };
                    view! {
                        <span class="pair"><b>{label}</b>{format!(" ({span}) → {} – {}", short(lo), short(hi))}</span>
                    }
                }).collect_view()}
            </div>
        })
    }
}

fn doc_size(axes: &[AxisDoc]) -> usize {
    axes.iter().map(|a| a.points.len()).product()
}

/// The values written at each point: one row per point for a part over one
/// input, a grid per parameter over two.
#[component]
fn ValuesTable(q: u64, p: u64, part: Memo<Option<PartDoc>>) -> impl IntoView {
    let ctx = rctx();
    let set = move |j: usize, k: usize, v: f64| {
        ctx.part_update(q, p, |x| {
            if let Some(d) = &mut x.dist {
                d.params[j].values[k] = v;
                d.params[j].constant = false;
            }
        })
    };
    let cell = move |j: usize, k: usize, v: f64| {
        view! {
            <td class="num"><input class="value" inputmode="decimal" prop:value=num(v)
                on:change=move |ev| if let Some(v) = parse_num(&event_target_value(&ev)) { set(j, k, v) }/></td>
        }
    };
    move || {
        let Some(doc) = part.get() else {
            return ().into_any();
        };
        let (Some(axes), Some(d)) = (doc.axes, doc.dist) else {
            return ().into_any();
        };
        if d.params.iter().any(|x| x.values.len() != doc_size(&axes)) {
            return view! { <p class="hint">"The values do not match the points: edit them in the YAML."</p> }.into_any();
        }
        match axes.len() {
            0 => view! {
                <table class="rules-table"><tbody><tr>
                    {d.params.iter().enumerate().map(|(j, prm)| view! { <th>{prm.name.clone()}</th>{cell(j, 0, prm.values[0])} }).collect_view()}
                </tr></tbody></table>
            }
            .into_any(),
            1 => {
                let a = &axes[0];
                view! {
                    <div class="table-scroll">
                        <table class="rules-table">
                            <thead><tr><th>{shown(&a.input)}</th>{d.params.iter().map(|prm| view! { <th class="num">{prm.name.clone()}</th> }).collect_view()}</tr></thead>
                            <tbody>
                                {a.points.iter().enumerate().map(|(k, x)| view! {
                                    <tr><td class="mono">{num(*x)}</td>{d.params.iter().enumerate().map(|(j, prm)| cell(j, k, prm.values[k])).collect_view()}</tr>
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
                .into_any()
            }
            2 => {
                let (a, b) = (&axes[0], &axes[1]);
                let nb = b.points.len();
                d.params
                    .iter()
                    .enumerate()
                    .map(|(j, prm)| {
                        view! {
                            <h3>{format!("{} by {} (rows) and {} (columns)", prm.name, shown(&a.input), shown(&b.input))}</h3>
                            <div class="table-scroll">
                                <table class="rules-table">
                                    <thead><tr><th></th>{b.points.iter().map(|y| view! { <th class="num mono">{num(*y)}</th> }).collect_view()}</tr></thead>
                                    <tbody>
                                        {a.points.iter().enumerate().map(|(r, x)| view! {
                                            <tr><td class="mono">{num(*x)}</td>{(0..nb).map(|c| cell(j, r * nb + c, prm.values[r * nb + c])).collect_view()}</tr>
                                        }).collect_view()}
                                    </tbody>
                                </table>
                            </div>
                        }
                    })
                    .collect_view()
                    .into_any()
            }
            _ => view! { <p class="hint">"A part over more than two inputs is edited in the YAML."</p> }.into_any(),
        }
    }
}

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

#[component]
pub fn RulesPanel() -> impl IntoView {
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
            let Some(set) = g.as_ref() else {
                return vec![];
            };
            let Some(q) = set.quantities.get(&qn) else {
                return vec![];
            };
            q.inputs()
                .into_iter()
                .filter_map(|n| {
                    let (lo, hi) = rules_doc::input_range(set, &qn, &n)?;
                    let pad = (hi - lo) * 0.1;
                    Some((n, lo - pad, hi + pad))
                })
                .collect::<Vec<_>>()
        })
    });
    let x = Memo::new(move |_| {
        let all = inputs.get();
        ctx.x
            .get()
            .filter(|x| all.iter().any(|(n, ..)| n == x))
            .or_else(|| all.first().map(|i| i.0.clone()))
    });
    view! {
        <div class="field">
            <span>"Quantity"</span>
            <select prop:value=move || quantity.get().unwrap_or_default() on:change=move |ev| ctx.tested.set(Some(event_target_value(&ev)))>
                {move || names.get().into_iter().map(|n| view! { <option value=n.clone()>{n.clone()}</option> }).collect_view()}
            </select>
        </div>
        <h3>"Inputs"</h3>
        {move || inputs.get().into_iter().map(|(n, lo, hi)| {
            let n1 = n.clone();
            let n2 = n.clone();
            let value = Signal::derive(move || ctx.tested_value(&quantity.get().unwrap_or_default(), &n1));
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
        <div class="test-row">
            <div class="line"><span class="mono">"quantile p"</span><span class="spacer"></span><span class="mono">{move || num(ctx.p.get())}</span></div>
            <input type="range" min="0" max="1" step="0.01" prop:value=move || ctx.p.get().to_string()
                on:input=move |ev| if let Some(v) = parse_num(&event_target_value(&ev)) { ctx.p.set(v) }/>
        </div>
        <h3>"Values"</h3>
        {move || {
            let Some(qn) = quantity.get() else {
                return view! { <p class="hint">"No rules that read yet."</p> }.into_any();
            };
            match ctx.joined(&qn) {
                Some(j) => summary_table(&qn, &j, ctx.p.get(), None),
                None => view! { <p class="hint">"No rules that read yet."</p> }.into_any(),
            }
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

/// What the response chart draws: the input's range, the tested value, the
/// samples, the quantity's (low, at p, high) at each, and each part's value
/// at p.
type Response = (
    f64,
    f64,
    f64,
    Vec<f64>,
    Vec<(f64, f64, f64)>,
    Vec<(String, Vec<f64>)>,
);

/// A quantity's range and its value at the tested quantile as one input
/// moves, each part's value at the quantile dashed.
#[component]
fn ResponseChart(quantity: Memo<Option<String>>, x: Memo<Option<String>>) -> impl IntoView {
    let ctx = rctx();
    let data = Memo::new(move |_| -> Option<Response> {
        let qn = quantity.get()?;
        let xn = x.get()?;
        let probe = ctx.probe.get();
        let pq = ctx.p.get();
        ctx.good.with(|g| {
            let set = g.as_ref()?;
            let (lo, hi) = rules_doc::input_range(set, &qn, &xn)?;
            let pad = (hi - lo) * 0.1;
            let (lo, hi) = (lo - pad, hi + pad);
            let base: IndexMap<String, f64> = rules_doc::values_at(set, &qn, &probe);
            let at = base.get(&xn).copied().unwrap_or((lo + hi) / 2.0);
            let n = 80;
            let mut xs = vec![];
            let mut joined = vec![];
            let mut parts: Vec<(String, Vec<f64>)> = vec![];
            for k in 0..=n {
                let v = lo + (hi - lo) * k as f64 / n as f64;
                let mut values = base.clone();
                values.insert(xn.clone(), v);
                let Ok(j) = set.joined(&qn, &values) else {
                    continue;
                };
                xs.push(v);
                let (l, h) = j.support();
                joined.push((l, j.quantile(pq), h));
                if parts.is_empty() {
                    parts = j.parts.iter().map(|(n, _)| (n.clone(), vec![])).collect();
                }
                for ((_, d), (_, ys)) in j.parts.iter().zip(parts.iter_mut()) {
                    ys.push(d.quantile(pq));
                }
            }
            Some((lo, hi, at, xs, joined, parts))
        })
    });
    move || {
        let Some((lo, hi, at, xs, joined, parts)) = data.get() else {
            return view! { <p class="hint">"Nothing to draw."</p> }.into_any();
        };
        if xs.is_empty() {
            return view! { <p class="hint">"Nothing to draw."</p> }.into_any();
        }
        let (w, h, top, bot) = (320.0, 190.0, 10.0, 160.0);
        let mut ymin = joined.iter().map(|x| x.0).fold(f64::INFINITY, f64::min);
        let mut ymax = joined.iter().map(|x| x.2).fold(f64::NEG_INFINITY, f64::max);
        for (_, ys) in &parts {
            for y in ys {
                ymin = ymin.min(*y);
                ymax = ymax.max(*y);
            }
        }
        if ymax.partial_cmp(&ymin) != Some(std::cmp::Ordering::Greater) {
            ymin -= 1.0;
            ymax += 1.0;
        }
        let pad = (ymax - ymin) * 0.06;
        let (ymin, ymax) = (ymin - pad, ymax + pad);
        let sx = move |v: f64| (v - lo) / (hi - lo) * w;
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
        let top_edge: Vec<String> = xs
            .iter()
            .zip(&joined)
            .map(|(x, j)| format!("{:.1},{:.1}", sx(*x), sy(j.2)))
            .collect();
        let bottom_edge: Vec<String> = xs
            .iter()
            .zip(&joined)
            .rev()
            .map(|(x, j)| format!("{:.1},{:.1}", sx(*x), sy(j.0)))
            .collect();
        let multi = parts.len() > 1;
        let part_lines = if multi {
            parts
                .iter()
                .enumerate()
                .map(|(k, (_, ys))| view! { <path class=format!("resp base o{}", k % 4) d=path(ys.clone())/> })
                .collect_view()
                .into_any()
        } else {
            ().into_any()
        };
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
                <path class="band-area" d=format!("M{} L{} Z", top_edge.join(" L"), bottom_edge.join(" L"))/>
                {part_lines}
                <path class="atp-line" d=path(joined.iter().map(|j| j.1).collect())/>
                <line class="probe-line" x1=sx(at) x2=sx(at) y1=top y2=bot/>
            </svg>
            <div class="legend-line">
                <span><i class="key band-key"></i>"low – high"</span>
                <span><i class="key atp-key"></i>"at the tested quantile"</span>
                {multi.then(|| view! { <span class="hint">"dashed: each part at the quantile"</span> })}
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
                        <span class="tag">{format!("combine {}", q.combine.name())}</span>
                    </div>
                    <ul>
                        {q.parts.iter().map(|p| {
                            let over = p
                                .axes
                                .iter()
                                .map(|a| format!("{} [{}]", a.input, a.points.iter().map(|v| num(*v)).collect::<Vec<_>>().join(", ")))
                                .collect::<Vec<_>>()
                                .join(" × ");
                            view! {
                                <li><span class="mono">{p.name.clone()}</span>": "
                                    {format!("{} over {over} ({} points)", p.family.name(), p.size())}
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                </div>
            }).collect_view()}
            {(!set.inputs.is_empty()).then(|| view! {
                <h3>"Inputs"</h3>
                <ul>{set.inputs.values().map(|i| view! { <li class="mono">{format!("{} default {}", i.name, i.default.map(num).unwrap_or_else(|| "none".into()))}</li> }).collect_view()}</ul>
            })}
            <h3>"Files read"</h3>
            <ul>{set.files.iter().map(|f| view! { <li class="mono">{format!("{} {}", &f.sha256[..12], f.label)}</li> }).collect_view()}</ul>
        }.into_any(),
    }
    }
}
