//! The editor's shell: the toolbar, the outline of the document, the map or
//! the form of what is selected, and the panel to test values, read the YAML
//! and see what openodd made of it.

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;

use crate::check::{self, Evaluation, Report, State};
use crate::doc::{tail, Concept, Document, Kind, KindTag, Module};
use crate::forms::{ConceptEditor, ModuleEditor, Settings};
use crate::icons::{concept_icon, Icon};
use crate::io;
use crate::viz::{Domain, Legend, ModuleGraph, OddMap, OddSentence, StateBar, StructureTree};

pub const URBAN: &str = include_str!("../examples/urban.yaml");
pub const DEFAULT: &str = include_str!("../../../python/openodd/odds/default/default.yaml");

/// What the middle shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sel {
    Map,
    Settings,
    Concept(u64),
    Module(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Test,
    Yaml,
    Details,
}

/// The editor's state, shared by every component.
#[derive(Clone, Copy)]
pub struct Ctx {
    pub doc: RwSignal<Document>,
    pub name: RwSignal<String>,
    /// Files the document imports, by name.
    pub library: RwSignal<Vec<(String, String)>>,
    pub sel: RwSignal<Sel>,
    /// The document as YAML, or why it cannot be written.
    pub yaml: Memo<Result<String, String>>,
    pub report: Memo<Result<Report, String>>,
    /// The values being tested (attribute -> text), drawn on every view.
    pub probe: RwSignal<Vec<(String, String)>>,
    pub evaluation: Memo<Option<Evaluation>>,
    /// A message for the toolbar (what was opened, why it was not).
    pub notice: RwSignal<Option<(bool, String)>>,
}

pub fn ctx() -> Ctx {
    expect_context::<Ctx>()
}

impl Ctx {
    /// Replace the document with one read from `text`.
    pub fn open(&self, name: &str, text: &str) -> bool {
        match Document::from_yaml(text) {
            Ok(doc) => {
                self.doc.set(doc);
                self.name.set(name.to_string());
                self.sel.set(Sel::Map);
                self.probe.set(vec![]);
                self.notice.set(Some((true, format!("Opened {name}"))));
                true
            }
            Err(e) => {
                self.notice.set(Some((false, format!("{name}: {e}"))));
                false
            }
        }
    }

    /// One attribute of the report, as a memo: it fires only when that
    /// attribute changes.
    pub fn attribute(&self, name: String) -> Memo<Option<check::Attribute>> {
        let report = self.report;
        Memo::new(move |_| {
            report.with(|r| r.as_ref().ok().and_then(|r| r.attribute(&name).cloned()))
        })
    }

    pub fn probe_value(&self, name: &str) -> Option<String> {
        self.probe.with(|p| {
            p.iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .filter(|v| !v.is_empty())
        })
    }

    pub fn set_probe(&self, name: &str, value: String) {
        self.probe.update(|p| {
            if let Some(e) = p.iter_mut().find(|(n, _)| n == name) {
                e.1 = value;
            } else {
                p.push((name.to_string(), value));
            }
        })
    }

    /// Select the concept a dotted name names.
    pub fn select_attribute(&self, name: &str) {
        if let Some(id) = self.doc.with(|d| d.concept_id(name)) {
            self.sel.set(Sel::Concept(id));
        }
    }

    pub fn select_module(&self, name: &str) {
        if let Some(id) = self
            .doc
            .with(|d| d.modules.iter().find(|m| m.name == name).map(|m| m.id))
        {
            self.sel.set(Sel::Module(id));
        }
    }
}

/// Which editor the studio shows: the ODD, or a backend's fuzzy rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Odd,
    Binding,
    Rules,
}

impl Mode {
    /// The mode the page's address names (`#rules`).
    fn from_location() -> Mode {
        let hash = web_sys::window()
            .and_then(|w| w.location().hash().ok())
            .unwrap_or_default();
        match hash.as_str() {
            "#rules" => Mode::Rules,
            "#binding" => Mode::Binding,
            _ => Mode::Odd,
        }
    }
}

/// The studio: an ODD, the binding that measures it, and a backend's fuzzy
/// rules, each with its own editor and all three held at once, so that each
/// can show how it meets the others.
#[component]
pub fn App() -> impl IntoView {
    let mode = RwSignal::new(Mode::from_location());
    provide_context(mode);
    provide_context(odd_context());
    provide_context(crate::rules_app::rules_context());
    provide_context(crate::binding_app::binding_context());
    move || match mode.get() {
        Mode::Odd => view! { <OddStudio/> }.into_any(),
        Mode::Binding => view! { <crate::binding_app::BindingStudio/> }.into_any(),
        Mode::Rules => view! { <crate::rules_app::RulesStudio/> }.into_any(),
    }
}

/// The switch between the ODD editor and the rules editor.
#[component]
pub fn ModeSwitch() -> impl IntoView {
    let mode = expect_context::<RwSignal<Mode>>();
    let go = move |m: Mode| {
        mode.set(m);
        if let Some(w) = web_sys::window() {
            let _ = w
                .location()
                .set_hash(if m == Mode::Rules { "rules" } else { "" });
        }
    };
    view! {
        <div class="segmented mode" role="tablist" aria-label="Editor">
            <button class:active=move || mode.get() == Mode::Odd on:click=move |_| go(Mode::Odd)>
                <Icon name="map"/>"ODD"
            </button>
            <button class:active=move || mode.get() == Mode::Binding on:click=move |_| go(Mode::Binding)>
                <Icon name="link"/>"Binding"
            </button>
            <button class:active=move || mode.get() == Mode::Rules on:click=move |_| go(Mode::Rules)>
                <Icon name="func"/>"Fuzzy rules"
            </button>
        </div>
    }
}

/// The ODD editor's state, kept while another editor is shown.
fn odd_context() -> Ctx {
    let (name, text) = io::load_draft().unwrap_or_else(|| ("urban.yaml".into(), URBAN.into()));
    let doc = Document::from_yaml(&text)
        .or_else(|_| Document::from_yaml(URBAN))
        .unwrap_or_else(|_| Document::empty());
    let doc = RwSignal::new(doc);
    let name = RwSignal::new(name);
    let library = RwSignal::new(Vec::<(String, String)>::new());
    let yaml = Memo::new(move |_| doc.with(Document::to_yaml));
    let report = Memo::new(move |_| {
        let text = yaml.get()?;
        check::check(&check::Files {
            entry: name.get(),
            text,
            library: library.get(),
        })
    });
    let probe = RwSignal::new(Vec::<(String, String)>::new());
    let evaluation = Memo::new(move |_| {
        let r = report.get().ok()?;
        let p = probe.get();
        p.iter()
            .any(|(_, v)| !v.is_empty())
            .then(|| check::evaluate(&r, &p))
    });
    let ctx = Ctx {
        doc,
        name,
        library,
        sel: RwSignal::new(Sel::Map),
        yaml,
        report,
        probe,
        evaluation,
        notice: RwSignal::new(None),
    };
    // Keep the draft between visits.
    Effect::new(move |_| {
        if let Ok(text) = yaml.get() {
            io::save_draft(&name.get(), &text);
        }
    });
    ctx
}

#[component]
fn OddStudio() -> impl IntoView {
    let ctx = ctx();
    view! {
        <div class="app">
            <Toolbar/>
            <main class="layout">
                <aside class="outline">
                    <Outline/>
                </aside>
                <section class="center">
                    {move || match ctx.sel.get() {
                        Sel::Map => view! { <MapView/> }.into_any(),
                        Sel::Settings => view! { <Settings/> }.into_any(),
                        Sel::Concept(id) => view! { <ConceptEditor id=id/> }.into_any(),
                        Sel::Module(id) => view! { <ModuleEditor id=id/> }.into_any(),
                    }}
                </section>
                <aside class="panel">
                    <Panel/>
                </aside>
            </main>
        </div>
    }
}

#[component]
fn Toolbar() -> impl IntoView {
    let ctx = ctx();
    let open = move |ev: leptos::ev::Event| {
        let input: HtmlInputElement = event_target(&ev);
        spawn_local(async move {
            if let Some((name, text)) = io::read_files(&input).await.into_iter().next() {
                ctx.open(&name, &text);
            }
        });
    };
    let example = move |ev: leptos::ev::Event| {
        let which = event_target_value(&ev);
        match which.as_str() {
            "urban" => {
                ctx.open("urban.yaml", URBAN);
            }
            "default" => {
                ctx.open("default.yaml", DEFAULT);
            }
            "empty" => {
                ctx.open("odd.yaml", "");
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
    let status = move || match ctx.report.get() {
        Ok(_) => view! {
            <span class="badge ok" title="openodd reads this ODD">
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
                <option value="urban">"Urban shuttle (modules)"</option>
                <option value="default">"openodd default (taxonomy)"</option>
                <option value="empty">"Empty document"</option>
            </select>
            <button class="button primary" on:click=save><Icon name="download"/>"Download YAML"</button>
            <div class="status">{status}</div>
            <a class="source" href="https://github.com/hakuturu583/openodd_py" target="_blank" rel="noopener">
                "GitHub"
            </a>
            {move || {
                ctx.notice
                    .get()
                    .map(|(ok, text)| {
                        view! {
                            <div
                                class=if ok { "notice ok" } else { "notice bad" }
                                on:click=move |_| ctx.notice.set(None)
                            >
                                {text}
                            </div>
                        }
                    })
            }}
        </header>
    }
}

/// A row of the outline: a concept, at its depth.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    id: u64,
    depth: usize,
    name: String,
    path: String,
    tag: KindTag,
}

fn rows(cs: &[Concept], depth: usize, prefix: &str, out: &mut Vec<Row>) {
    for c in cs {
        let path = if prefix.is_empty() {
            c.name.clone()
        } else {
            format!("{prefix}.{}", c.name)
        };
        out.push(Row {
            id: c.id,
            depth,
            name: c.name.clone(),
            path: path.clone(),
            tag: c.kind.tag(),
        });
        if let Kind::Group(children) = &c.kind {
            rows(children, depth + 1, &path, out);
        }
    }
}

/// A name not yet among `taken`: `base`, else `base_2`, ...
pub fn unique(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    (2..)
        .map(|i| format!("{base}_{i}"))
        .find(|n| !taken.iter().any(|t| t == n))
        .unwrap()
}

/// Add a concept to a group (`None`: the taxonomy) and select it.
pub fn add_concept(parent: Option<u64>, tag: KindTag) {
    let ctx = ctx();
    let mut new_id = None;
    ctx.doc.update(|d| {
        if let Some(list) = d.siblings_mut(parent) {
            let taken: Vec<String> = list.iter().map(|c| c.name.clone()).collect();
            let base = if tag == KindTag::Group {
                "group"
            } else {
                "concept"
            };
            let c = Concept {
                id: crate::doc::fresh_id(),
                name: unique(base, &taken),
                kind: Kind::new(tag),
            };
            new_id = Some(c.id);
            list.push(c);
        }
    });
    if let Some(id) = new_id {
        ctx.sel.set(Sel::Concept(id));
    }
}

pub fn add_module(root: bool) {
    let ctx = ctx();
    let mut id = 0;
    ctx.doc.update(|d| {
        let taken: Vec<String> = d.modules.iter().map(|m| m.name.clone()).collect();
        let m = Module::new(unique(if root { "odd" } else { "module" }, &taken), root);
        id = m.id;
        d.modules.push(m);
    });
    ctx.sel.set(Sel::Module(id));
}

/// How much of an attribute the ODD leaves out: a ring for the outline.
fn restriction(r: &Report, path: &str) -> Option<(usize, usize)> {
    let a = r.attribute(path)?;
    let out = a.states.iter().filter(|s| **s == State::Outside).count();
    Some((out, a.states.len()))
}

#[component]
fn Outline() -> impl IntoView {
    let ctx = ctx();
    let concepts = Memo::new(move |_| {
        let mut out = vec![];
        ctx.doc.with(|d| rows(&d.taxonomy, 0, "", &mut out));
        out
    });
    let modules = Memo::new(move |_| {
        ctx.doc.with(|d| {
            d.modules
                .iter()
                .map(|m| (m.id, m.name.clone(), m.root, m.active))
                .collect::<Vec<_>>()
        })
    });
    let selected = move |s: Sel| ctx.sel.get() == s;
    let module_list = move |root: bool| {
        move || {
            modules
                .get()
                .into_iter()
                .filter(|(_, _, r, _)| *r == root)
                .map(|(id, name, _, active)| {
                    let n2 = name.clone();
                    let truth = move || {
                        ctx.evaluation.with(|e| {
                            e.as_ref()
                                .and_then(|e| e.truth(&n2).map(|t| format!("truth-dot {t}")))
                        })
                    };
                    view! {
                        <li
                            class="row"
                            class:selected=move || selected(Sel::Module(id))
                            class:inactive=!active
                            on:click=move |_| ctx.sel.set(Sel::Module(id))
                        >
                            <span class="icon"><Icon name=if root { "target" } else { "module" }/></span>
                            <span class="name">{name}</span>
                            {move || truth().map(|c| view! { <span class=c></span> })}
                        </li>
                    }
                })
                .collect_view()
        }
    };
    view! {
        <nav>
            <ul class="tree">
                <li class="row big" class:selected=move || selected(Sel::Map) on:click=move |_| ctx.sel.set(Sel::Map)>
                    <span class="icon"><Icon name="map"/></span>
                    <span class="name">"ODD map"</span>
                </li>
                <li class="row big" class:selected=move || selected(Sel::Settings) on:click=move |_| ctx.sel.set(Sel::Settings)>
                    <span class="icon"><Icon name="doc"/></span>
                    <span class="name">"Document & imports"</span>
                </li>
            </ul>
            <div class="heading">
                <span>"Taxonomy"</span>
                <button class="mini" title="Add a concept" on:click=move |_| add_concept(None, KindTag::Categorical)>
                    <Icon name="plus"/>"concept"
                </button>
                <button class="mini" title="Add a group" on:click=move |_| add_concept(None, KindTag::Group)>
                    <Icon name="folder"/>
                </button>
            </div>
            <ul class="tree">
                {move || {
                    concepts
                        .get()
                        .into_iter()
                        .map(|r| {
                            let id = r.id;
                            let group = r.tag == KindTag::Group;
                            let path = r.path.clone();
                            let ring = move || {
                                ctx.report.with(|rep| rep.as_ref().ok().and_then(|rep| restriction(rep, &path)))
                            };
                            view! {
                                <li
                                    class="row"
                                    class:group=group
                                    class:selected=move || selected(Sel::Concept(id))
                                    style=format!("padding-left: {}px", 10 + r.depth * 16)
                                    on:click=move |_| ctx.sel.set(Sel::Concept(id))
                                >
                                    <span class=format!("icon kind-{}", r.tag.label().replace(' ', "-"))>
                                        <Icon name=concept_icon(&r.name, r.tag)/>
                                    </span>
                                    <span class="name">{r.name}</span>
                                    {move || ring().map(|(out, n)| view! { <Ring out=out total=n/> })}
                                    {group.then(|| view! {
                                        <button
                                            class="mini add"
                                            title="Add a concept to this group"
                                            on:click=move |ev| {
                                                ev.stop_propagation();
                                                add_concept(Some(id), KindTag::Categorical);
                                            }
                                        >
                                            <Icon name="plus"/>
                                        </button>
                                    })}
                                </li>
                            }
                        })
                        .collect_view()
                }}
                {move || concepts.with(Vec::is_empty).then(|| view! {
                    <li class="empty">"No concepts yet."</li>
                })}
            </ul>
            <div class="heading">
                <span>"ODD roots"</span>
                <button class="mini" title="Add a root module" on:click=move |_| add_module(true)><Icon name="plus"/>"root"</button>
            </div>
            <ul class="tree">{module_list(true)}</ul>
            <div class="heading">
                <span>"Modules"</span>
                <button class="mini" title="Add a module" on:click=move |_| add_module(false)><Icon name="plus"/>"module"</button>
            </div>
            <ul class="tree">{module_list(false)}</ul>
        </nav>
    }
}

/// A small ring: how many of an attribute's buckets the ODD restricts.
#[component]
fn Ring(out: usize, total: usize) -> impl IntoView {
    let frac = if total == 0 {
        0.0
    } else {
        out as f64 / total as f64
    };
    let c = 2.0 * std::f64::consts::PI * 6.0;
    view! {
        <svg class="ring" viewBox="0 0 16 16" width="16" height="16">
            <title>{format!("{out} of {total} buckets restricted")}</title>
            <circle class="ring-bg" cx="8" cy="8" r="6"/>
            <circle
                class="ring-fg"
                cx="8"
                cy="8"
                r="6"
                stroke-dasharray=format!("{} {}", c * frac, c)
                transform="rotate(-90 8 8)"
            />
        </svg>
    }
}

// ---------------------------------------------------------------------------
// The map
// ---------------------------------------------------------------------------

#[component]
fn MapView() -> impl IntoView {
    let ctx = ctx();
    // A phone shows the structure as a tree: a graph does not fit it.
    let narrow = crate::viz::narrow_screen();
    let stats = move || {
        ctx.report.with(|r| {
            let r = r.as_ref().ok()?;
            let mut counts = [0usize; 2];
            for s in r.attributes.iter().flat_map(|a| &a.states) {
                counts[(*s == State::Outside) as usize] += 1;
            }
            let restricted = r
                .attributes
                .iter()
                .filter(|a| a.states.contains(&State::Outside))
                .count();
            Some((
                r.attributes.len(),
                restricted,
                r.modules.len(),
                r.roots.len(),
                counts,
                r.version.clone(),
            ))
        })
    };
    view! {
        <div class="map-view">
            {move || match ctx.report.get() {
                Err(e) => view! {
                    <div class="card error-card">
                        <div class="card-title"><Icon name="warn"/>"openodd cannot read this ODD"</div>
                        <p class="mono">{e}</p>
                        <p class="hint">"The map comes back once the document reads. The outline and forms still edit it."</p>
                    </div>
                }.into_any(),
                Ok(_) => ().into_any(),
            }}
            {move || stats().map(|(attrs, restricted, modules, roots, counts, version)| {
                let total = counts.iter().sum::<usize>().max(1);
                view! {
                    <div class="stats">
                        <div class="stat">
                            <div class="stat-icon"><Icon name="list"/></div>
                            <div><div class="stat-n">{attrs}</div><div class="stat-l">"attributes"</div></div>
                        </div>
                        <div class="stat">
                            <div class="stat-icon warn"><Icon name="ban"/></div>
                            <div><div class="stat-n">{restricted}</div><div class="stat-l">"attributes restricted"</div></div>
                        </div>
                        <div class="stat">
                            <div class="stat-icon"><Icon name="module"/></div>
                            <div><div class="stat-n">{modules}</div><div class="stat-l">{format!("modules · {roots} root{}", if roots == 1 { "" } else { "s" })}</div></div>
                        </div>
                        <div class="stat wide">
                            <div class="stat-l">{format!("buckets: {} allowed · {} excluded", counts[0], counts[1])}</div>
                            <StateBar counts=counts/>
                            <div class="stat-l">{format!("{}% of the domain excluded", counts[1] * 100 / total)}{version.map(|v| format!(" · version {v}"))}</div>
                        </div>
                    </div>
                }
            })}
            <div class="card">
                <div class="card-title"><Icon name="graph"/>"Structure"<span class="hint">{move || if narrow.get() { "roots → modules → attributes · tap to edit" } else { "roots → modules → attributes · hover to trace, click to edit" }}</span></div>
                <details class="sentence-box" open>
                    <summary><Icon name="doc"/>"In words"</summary>
                    <OddSentence/>
                </details>
                <div class="graph-legend">
                    <span><i class="ln include"></i>"include"</span>
                    <span><i class="ln exclude"></i>"exclude"</span>
                    <span><i class="ln carries"></i>"label"</span>
                    <span class="hint">{move || if narrow.get() {
                        "Each row says the condition that puts it there. Tap a name to edit it."
                    } else {
                        "Hover or tap an arrow for the conditions behind it. With test values set, nodes show each module's verdict."
                    }}</span>
                </div>
                {move || if narrow.get() {
                    view! { <StructureTree/> }.into_any()
                } else {
                    view! {
                <ModuleGraph
                            on_select=Callback::new(move |(id, attr): (String, bool)| {
                                if attr { ctx.select_attribute(&id) } else { ctx.select_module(&id) }
                            })
                        />
                    }.into_any()
                }}
            </div>
            <div class="card">
                <div class="card-title"><Icon name="grid"/>"ODD map"<span class="hint">"two attributes at once · click a cell to test it"</span></div>
                <Legend/>
                <OddMap/>
            </div>
            <div class="card">
                <div class="card-title"><Icon name="list"/>"Attributes"<span class="hint">"click a value or drag on a line to test it"</span></div>
                <Legend/>
                <AttributeCards/>
            </div>
        </div>
    }
}

/// Every attribute, grouped by the taxonomy's top level, with its domain.
#[component]
fn AttributeCards() -> impl IntoView {
    let ctx = ctx();
    let groups = Memo::new(move |_| {
        let Some(r) = ctx.report.get().ok() else {
            return vec![];
        };
        let mut out: Vec<(String, Vec<String>)> = vec![];
        for a in &r.attributes {
            let (group, _) = a.name.split_once('.').unwrap_or(("", &a.name));
            match out.iter_mut().find(|(g, _)| g == group) {
                Some((_, v)) => v.push(a.name.clone()),
                None => out.push((group.to_string(), vec![a.name.clone()])),
            }
        }
        out
    });
    move || {
        groups
            .get()
            .into_iter()
            .map(|(group, names)| {
                view! {
                    <div class="attr-group">
                        {(!group.is_empty()).then(|| view! {
                            <div class="group-title"><Icon name=concept_icon(&group, KindTag::Group)/>{group.clone()}</div>
                        })}
                        <div class="attr-grid">
                            {names.into_iter().map(|n| view! { <AttributeCard name=n/> }).collect_view()}
                        </div>
                    </div>
                }
            })
            .collect_view()
    }
}

/// One attribute: its icon, its domain coloured by state, and the modules testing it.
#[component]
pub fn AttributeCard(name: String) -> impl IntoView {
    let ctx = ctx();
    let attr = ctx.attribute(name.clone());
    let n2 = name.clone();
    let testers = Memo::new(move |_| {
        ctx.report.with(|r| {
            r.as_ref()
                .map(|r| {
                    r.modules
                        .iter()
                        .filter(|m| m.links.iter().any(|l| !l.module && l.target == n2))
                        .map(|m| m.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let short = tail(&name).to_string();
    let n3 = StoredValue::new(name);
    let icon = move || {
        let tag = attr.with(|a| a.as_ref().map_or(KindTag::Other, |a| a.kind));
        n3.with_value(|n| view! { <Icon name=concept_icon(tail(n), tag)/> })
    };
    view! {
        <div class="attr-card">
            <div class="attr-head" on:click=move |_| n3.with_value(|n| ctx.select_attribute(n))>
                <span class="attr-icon">{icon}</span>
                <span class="attr-name">{short}</span>
                <span class="tag">{move || attr.with(|a| a.as_ref().map(|a| if a.unit.is_empty() { a.kind.label().to_string() } else { format!("{} · {}", a.kind.label(), a.unit) }).unwrap_or_default())}</span>
            </div>
            <Domain attr=attr compact=true/>
            <div class="testers">
                {move || {
                    let t = testers.get();
                    if t.is_empty() {
                        view! { <span class="hint">"not tested by any module"</span> }.into_any()
                    } else {
                        t.into_iter().map(|m| {
                            let m2 = m.clone();
                            view! { <button class="pill" on:click=move |_| ctx.select_module(&m2)><Icon name="module"/>{m}</button> }
                        }).collect_view().into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

#[component]
fn Panel() -> impl IntoView {
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

/// Values a run would measure, set by hand, judged as they change.
#[component]
fn TestTab() -> impl IntoView {
    let ctx = ctx();
    let attributes = Memo::new(move |_| {
        ctx.report
            .get()
            .map(|r| {
                r.attributes
                    .into_iter()
                    .filter(|a| !a.derived())
                    .map(|a| a.name)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    let fill = move |_| {
        // A point that may be inside: each attribute's first allowed bucket.
        if let Ok(r) = ctx.report.get() {
            let mut p = vec![];
            for a in r.attributes.iter().filter(|a| !a.derived()) {
                let i = a.states.iter().position(|s| *s == State::Inside);
                if let Some(i) = i {
                    p.push((a.name.clone(), crate::viz::representative(a, i)));
                }
            }
            ctx.probe.set(p);
        }
    };
    view! {
        <div class="test">
            {move || {
                match ctx.evaluation.get() {
                    None => view! {
                        <div class="verdict idle">
                            <Icon name="flask"/>
                            <div>
                                <div class="v-title">"Test a situation"</div>
                                <div class="hint">"Set values below, drag a marker on any line, or click a cell of the map."</div>
                            </div>
                        </div>
                    }.into_any(),
                    Some(e) => {
                        let (class, icon, title) = if !e.inside {
                            ("verdict outside", "ban", "Outside the ODD")
                        } else if e.assumed {
                            ("verdict depends", "warn", "Inside, if the unknowns allow")
                        } else {
                            ("verdict inside", "check", "Inside the ODD")
                        };
                        let derived = e.derived.clone();
                        view! {
                            <div class=class>
                                <Icon name=icon/>
                                <div>
                                    <div class="v-title">{title}</div>
                                    <div class="truths">
                                        {e.modules.into_iter().map(|(m, t)| {
                                            let m2 = m.clone();
                                            view! { <button class=format!("truth {t}") on:click=move |_| ctx.select_module(&m2)>{m}</button> }
                                        }).collect_view()}
                                    </div>
                                    {(!derived.is_empty()).then(|| view! {
                                        <div class="truths">
                                            {derived.into_iter().map(|(n, v)| view! { <span class="truth derived"><Icon name="func"/>{format!("{} = {v}", tail(&n))}</span> }).collect_view()}
                                        </div>
                                    })}
                                </div>
                            </div>
                        }.into_any()
                    }
                }
            }}
            <div class="actions">
                <button class="button" on:click=fill><Icon name="sparkle"/>"Fill a point inside"</button>
                <button class="button" on:click=move |_| ctx.probe.set(vec![])><Icon name="x"/>"Clear"</button>
            </div>
            <For each=move || attributes.get() key=|n| n.clone() let:name>
                <TestRow name=name/>
            </For>
        </div>
    }
}

#[component]
fn TestRow(name: String) -> impl IntoView {
    let ctx = ctx();
    let attr = ctx.attribute(name.clone());
    let short = tail(&name).to_string();
    let n2 = name.clone();
    let n3 = name.clone();
    let number = move || attr.with(|a| a.as_ref().is_some_and(|a| a.is_number()));
    let icon_name = short.clone();
    view! {
        <div class="test-row">
            <div class="test-head">
                <Icon name=concept_icon(&icon_name, KindTag::Number)/>
                <span class="mono">{short}</span>
                {move || number().then(|| {
                    let n4 = n2.clone();
                    let n5 = n2.clone();
                    let unit = attr.with(|a| a.as_ref().map(|a| a.unit.clone()).unwrap_or_default());
                    view! {
                        <input
                            class="num"
                            type="number"
                            step="any"
                            placeholder="unknown"
                            prop:value=move || ctx.probe_value(&n4).unwrap_or_default()
                            on:input=move |ev| ctx.set_probe(&n5, event_target_value(&ev))
                        />
                        <span class="unit">{unit}</span>
                    }
                })}
                {move || ctx.probe_value(&n3).map(|_| {
                    let n6 = n3.clone();
                    view! { <button class="mini" title="Unknown" on:click=move |_| ctx.set_probe(&n6, String::new())><Icon name="x"/></button> }
                })}
            </div>
            <Domain attr=attr compact=true/>
        </div>
    }
}

/// The YAML the document writes; edited here, it is read back into the forms.
#[component]
fn YamlTab() -> impl IntoView {
    let ctx = ctx();
    let draft = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let text = move || {
        draft
            .get()
            .unwrap_or_else(|| ctx.yaml.get().unwrap_or_else(|e| format!("# {e}")))
    };
    let apply = move |_| {
        if let Some(t) = draft.get() {
            match Document::from_yaml(&t) {
                Ok(doc) => {
                    ctx.doc.set(doc);
                    ctx.sel.set(Sel::Map);
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
            <textarea
                spellcheck="false"
                prop:value=text
                on:input=move |ev| draft.set(Some(event_target_value(&ev)))
            ></textarea>
            <Show when=move || draft.with(Option::is_some)>
                <div class="actions">
                    <button class="button primary" on:click=apply>"Apply"</button>
                    <button class="button" on:click=move |_| {
                        draft.set(None);
                        error.set(None);
                    }>"Discard"</button>
                </div>
            </Show>
            {move || error.get().map(|e| view! { <div class="error">{e}</div> })}
        </div>
    }
}

/// What openodd's reader makes of the document, in its words.
#[component]
fn DetailsTab() -> impl IntoView {
    let ctx = ctx();
    move || match ctx.report.get() {
        Err(e) => view! {
            <div class="error big">
                <strong>"openodd cannot read this ODD"</strong>
                <p>{e}</p>
            </div>
        }
        .into_any(),
        Ok(r) => {
            let short = r.revision.chars().take(12).collect::<String>();
            view! {
                <div class="check">
                    <dl class="facts">
                        <dt>"Name"</dt><dd>{r.name.clone()}</dd>
                        <dt>"Revision"</dt><dd class="mono" title=r.revision.clone()>{short}</dd>
                        <dt>"Version"</dt><dd>{r.version.clone().unwrap_or("—".into())}</dd>
                        <dt>"Roots"</dt><dd>{if r.roots.is_empty() { "— (no ODD section: every module)".to_string() } else { r.roots.join(", ") }}</dd>
                    </dl>
                    <h3>"Modules, as openodd reads them"</h3>
                    {r.modules.iter().map(|m| {
                        let sec = |label: &'static str, s: &Option<(bool, Vec<String>)>| s.clone().map(|(all, cs)| view! {
                            <div class="section">
                                <span class="op">{format!("{label}_{}", if all { "AND" } else { "OR" })}</span>
                                <ul>{cs.into_iter().map(|c| view! { <li class="mono">{c}</li> }).collect_view()}</ul>
                            </div>
                        });
                        view! {
                            <div class="module-card" class:inactive=!m.active>
                                <div class="title">
                                    <Icon name=if m.root { "target" } else { "module" }/>
                                    <span>{m.name.clone()}</span>
                                    {(!m.labels.is_empty()).then(|| view! { <span class="tag">{m.labels.join(", ")}</span> })}
                                    {(!m.active).then(|| view! { <span class="tag">"inactive"</span> })}
                                </div>
                                {sec("INCLUDE", &m.include)}
                                {sec("EXCLUDE", &m.exclude)}
                            </div>
                        }
                    }).collect_view()}
                    <h3>"Buckets"</h3>
                    <table class="attrs">
                        <tbody>
                            {r.attributes.iter().map(|a| view! {
                                <tr>
                                    <td class="mono">{a.name.clone()}</td>
                                    <td class="buckets">
                                        {a.buckets.iter().zip(&a.states).map(|(b, s)| view! {
                                            <span class=format!("bucket {}", s.class())>{b.clone()}</span>
                                        }).collect_view()}
                                    </td>
                                </tr>
                            }).collect_view()}
                        </tbody>
                    </table>
                </div>
            }
            .into_any()
        }
    }
}

pub fn event_target<T: JsCast>(ev: &leptos::ev::Event) -> T {
    ev.target().unwrap().unchecked_into::<T>()
}
