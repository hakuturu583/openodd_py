//! The editor's shell: the toolbar, the outline of the document, the form of
//! what is selected, and the panel with the YAML, the check and an evaluator.

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;

use crate::check::{self, Files, Report};
use crate::doc::{Concept, Document, Kind, KindTag, Module};
use crate::forms::{ConceptEditor, ModuleEditor, Overview};
use crate::io;

pub const URBAN: &str = include_str!("../examples/urban.yaml");
pub const DEFAULT: &str = include_str!("../../../python/openodd/odds/default/default.yaml");

/// What the form shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sel {
    Overview,
    Concept(u64),
    Module(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Yaml,
    Check,
    Evaluate,
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
    /// A message for the toolbar (what was opened, why it was not).
    pub notice: RwSignal<Option<(bool, String)>>,
}

pub fn ctx() -> Ctx {
    expect_context::<Ctx>()
}

impl Ctx {
    pub fn files(&self) -> Option<Files> {
        let text = self.yaml.get().ok()?;
        Some(Files {
            entry: self.name.get(),
            text,
            library: self.library.get(),
        })
    }

    /// Replace the document with one read from `text`.
    pub fn open(&self, name: &str, text: &str) -> bool {
        match Document::from_yaml(text) {
            Ok(doc) => {
                self.doc.set(doc);
                self.name.set(name.to_string());
                self.sel.set(Sel::Overview);
                self.notice.set(Some((true, format!("Opened {name}"))));
                true
            }
            Err(e) => {
                self.notice.set(Some((false, format!("{name}: {e}"))));
                false
            }
        }
    }
}

#[component]
pub fn App() -> impl IntoView {
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
        check::check(&Files {
            entry: name.get(),
            text,
            library: library.get(),
        })
    });
    let ctx = Ctx {
        doc,
        name,
        library,
        sel: RwSignal::new(Sel::Overview),
        yaml,
        report,
        notice: RwSignal::new(None),
    };
    provide_context(ctx);

    // Keep the draft between visits.
    Effect::new(move |_| {
        if let Ok(text) = yaml.get() {
            io::save_draft(&name.get(), &text);
        }
    });

    view! {
        <div class="app">
            <Toolbar/>
            <main class="layout">
                <aside class="outline">
                    <Outline/>
                </aside>
                <section class="form">
                    {move || match ctx.sel.get() {
                        Sel::Overview => view! { <Overview/> }.into_any(),
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
                ctx.doc.set(Document::empty());
                ctx.name.set("odd.yaml".into());
                ctx.sel.set(Sel::Overview);
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
        Ok(r) => view! {
            <span class="badge ok" title="openodd reads this ODD">
                "✓ valid · " {r.attributes.len()} " attributes · " {r.modules.len()} " modules"
            </span>
        }
        .into_any(),
        Err(e) => {
            let title = e.clone();
            view! { <span class="badge bad" title=title>"✗ " {e}</span> }.into_any()
        }
    };
    view! {
        <header class="toolbar">
            <div class="brand">
                <span class="logo">"◇"</span>
                <span>"OpenODD Editor"</span>
            </div>
            <input
                class="filename"
                aria-label="File name"
                prop:value=move || ctx.name.get()
                on:change=move |ev| ctx.name.set(event_target_value(&ev))
            />
            <label class="button">
                "Open…"
                <input type="file" accept=".yaml,.yml" class="hidden" on:change=open/>
            </label>
            <select class="button" on:change=example aria-label="Examples">
                <option value="" selected>"New / example…"</option>
                <option value="empty">"Empty document"</option>
                <option value="urban">"Urban shuttle (modules)"</option>
                <option value="default">"openodd default (taxonomy)"</option>
            </select>
            <button class="button primary" on:click=save>"Download YAML"</button>
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
    tag: KindTag,
    summary: String,
}

fn rows(cs: &[Concept], depth: usize, out: &mut Vec<Row>) {
    for c in cs {
        let summary = match &c.kind {
            Kind::Number {
                primitive,
                unit_type,
            } => format!("{primitive} {unit_type}"),
            Kind::Categorical(l) => format!("[{}]", l.join(", ")),
            Kind::Derived(d) => format!(
                "{{{}}}",
                d.iter()
                    .map(|d| d.literal.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Kind::Reference(r) => format!("→ {r}"),
            Kind::Boolean => "boolean".into(),
            Kind::Group(_) => String::new(),
            Kind::Other(t) => t.clone(),
        };
        out.push(Row {
            id: c.id,
            depth,
            name: c.name.clone(),
            tag: c.kind.tag(),
            summary,
        });
        if let Kind::Group(children) = &c.kind {
            rows(children, depth + 1, out);
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
                kind: Kind::Boolean.convert(tag),
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

#[component]
fn Outline() -> impl IntoView {
    let ctx = ctx();
    let concepts = Memo::new(move |_| {
        let mut out = vec![];
        ctx.doc.with(|d| rows(&d.taxonomy, 0, &mut out));
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
                    view! {
                        <li
                            class="row"
                            class:selected=move || selected(Sel::Module(id))
                            class:inactive=!active
                            on:click=move |_| ctx.sel.set(Sel::Module(id))
                        >
                            <span class="icon">{if root { "◆" } else { "◇" }}</span>
                            <span class="name">{name}</span>
                            {(!active).then(|| view! { <span class="tag">"inactive"</span> })}
                        </li>
                    }
                })
                .collect_view()
        }
    };
    view! {
        <nav>
            <ul class="tree">
                <li
                    class="row"
                    class:selected=move || selected(Sel::Overview)
                    on:click=move |_| ctx.sel.set(Sel::Overview)
                >
                    <span class="icon">"▤"</span>
                    <span class="name">"Document"</span>
                </li>
            </ul>
            <div class="heading">
                <span>"TAXONOMY"</span>
                <button class="mini" title="Add a concept" on:click=move |_| add_concept(None, KindTag::Categorical)>
                    "+ concept"
                </button>
                <button class="mini" title="Add a group" on:click=move |_| add_concept(None, KindTag::Group)>
                    "+ group"
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
                            view! {
                                <li
                                    class="row"
                                    class:selected=move || selected(Sel::Concept(id))
                                    style=format!("padding-left: {}px", 10 + r.depth * 16)
                                    on:click=move |_| ctx.sel.set(Sel::Concept(id))
                                >
                                    <span class=format!("icon kind-{}", r.tag.label().replace(' ', "-"))>
                                        {kind_icon(r.tag)}
                                    </span>
                                    <span class="name">{r.name}</span>
                                    <span class="summary">{r.summary}</span>
                                    {group.then(|| view! {
                                        <button
                                            class="mini add"
                                            title="Add a concept to this group"
                                            on:click=move |ev| {
                                                ev.stop_propagation();
                                                add_concept(Some(id), KindTag::Categorical);
                                            }
                                        >
                                            "+"
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
                <span>"ODD"</span>
                <button class="mini" title="Add a root module" on:click=move |_| add_module(true)>"+ root"</button>
            </div>
            <ul class="tree">{module_list(true)}</ul>
            <div class="heading">
                <span>"MODULES"</span>
                <button class="mini" title="Add a module" on:click=move |_| add_module(false)>"+ module"</button>
            </div>
            <ul class="tree">{module_list(false)}</ul>
        </nav>
    }
}

pub fn kind_icon(tag: KindTag) -> &'static str {
    match tag {
        KindTag::Group => "▸",
        KindTag::Number => "#",
        KindTag::Boolean => "◐",
        KindTag::Categorical => "≡",
        KindTag::Derived => "ƒ",
        KindTag::Reference => "→",
        KindTag::Other => "…",
    }
}

#[component]
fn Panel() -> impl IntoView {
    let tab = RwSignal::new(Tab::Yaml);
    let button = move |t: Tab, label: &'static str| {
        view! {
            <button class="tab" class:active=move || tab.get() == t on:click=move |_| tab.set(t)>
                {label}
            </button>
        }
    };
    view! {
        <div class="tabs">
            {button(Tab::Yaml, "YAML")}
            {button(Tab::Check, "Check")}
            {button(Tab::Evaluate, "Evaluate")}
        </div>
        <div class="tab-body">
            {move || match tab.get() {
                Tab::Yaml => view! { <YamlTab/> }.into_any(),
                Tab::Check => view! { <CheckTab/> }.into_any(),
                Tab::Evaluate => view! { <EvaluateTab/> }.into_any(),
            }}
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
                    ctx.sel.set(Sel::Overview);
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
                    "Edited: apply to read it back into the forms."
                } else {
                    "Written from the forms. Type here to edit the YAML directly (comments are not kept)."
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

/// What openodd's reader makes of the document.
#[component]
fn CheckTab() -> impl IntoView {
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
                    <h3>"Attributes"</h3>
                    <p class="hint">"Each attribute's buckets, as openodd covers them; those outside the ODD in red."</p>
                    <table class="attrs">
                        <thead><tr><th>"Attribute"</th><th>"Kind"</th><th>"Buckets"</th></tr></thead>
                        <tbody>
                            {r.attributes.iter().map(|a| {
                                let outside = a.outside.clone();
                                view! {
                                    <tr>
                                        <td class="mono">{a.name.clone()}{(!a.unit.is_empty()).then(|| view! { <span class="unit">{format!(" [{}]", a.unit)}</span> })}</td>
                                        <td>{a.kind.clone()}</td>
                                        <td class="buckets">
                                            {a.buckets.iter().map(|b| {
                                                let out = outside.contains(b);
                                                view! { <span class="bucket" class:outside=out>{b.clone()}</span> }
                                            }).collect_view()}
                                        </td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>
                    <h3>"Modules"</h3>
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
                                    <span>{if m.root { "◆ " } else { "◇ " }}{m.name.clone()}</span>
                                    {(!m.labels.is_empty()).then(|| view! { <span class="tag">{m.labels.join(", ")}</span> })}
                                    {(!m.active).then(|| view! { <span class="tag">"inactive"</span> })}
                                </div>
                                {sec("INCLUDE", &m.include)}
                                {sec("EXCLUDE", &m.exclude)}
                            </div>
                        }
                    }).collect_view()}
                </div>
            }
            .into_any()
        }
    }
}

/// Judge values against the ODD, as a run would be.
#[component]
fn EvaluateTab() -> impl IntoView {
    let ctx = ctx();
    let inputs = RwSignal::new(Vec::<(String, String)>::new());
    let set = move |name: String, value: String| {
        inputs.update(|v| {
            if let Some(e) = v.iter_mut().find(|(n, _)| *n == name) {
                e.1 = value;
            } else {
                v.push((name, value));
            }
        })
    };
    let get = move |name: &str| {
        inputs.with(|v| {
            v.iter()
                .find(|(n, _)| n == name)
                .map(|(_, x)| x.clone())
                .unwrap_or_default()
        })
    };
    let result = Memo::new(move |_| {
        let files = ctx
            .files()
            .ok_or_else(|| "the document cannot be written".to_string())?;
        check::evaluate(&files, &inputs.get())
    });
    let attributes = Memo::new(move |_| {
        ctx.report
            .get()
            .map(|r| {
                r.attributes
                    .into_iter()
                    .filter(|a| !a.derived)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    view! {
        <div class="evaluate">
            <p class="hint">"Enter what a run measures; leave a value empty for unknown (OpenODD's missing value)."</p>
            <div class="verdict">
                {move || match result.get() {
                    Err(e) => view! { <div class="error">{e}</div> }.into_any(),
                    Ok(e) => view! {
                        <div class=if e.inside { "big-verdict inside" } else { "big-verdict outside" }>
                            {if e.inside { if e.assumed { "Inside (assumed: values missing)" } else { "Inside the ODD" } } else { "Outside the ODD" }}
                        </div>
                        <div class="truths">
                            {e.modules.into_iter().map(|(m, t)| view! {
                                <span class=format!("truth {t}")>{m}": "{t.clone()}</span>
                            }).collect_view()}
                        </div>
                        {(!e.derived.is_empty()).then(|| view! {
                            <div class="truths">
                                {e.derived.into_iter().map(|(n, v)| view! { <span class="truth derived">{n}" = "{v}</span> }).collect_view()}
                            </div>
                        })}
                    }.into_any(),
                }}
            </div>
            <table class="inputs">
                <tbody>
                    <For each=move || attributes.get() key=|a| (a.name.clone(), a.values.clone()) let:a>
                        {
                            let name = a.name.clone();
                            let n2 = name.clone();
                            let control = if a.values.is_empty() {
                                view! {
                                    <input
                                        type="number"
                                        step="any"
                                        placeholder=if a.unit.is_empty() { "unknown".to_string() } else { a.unit.clone() }
                                        prop:value=move || get(&n2)
                                        on:input=move |ev| set(name.clone(), event_target_value(&ev))
                                    />
                                }.into_any()
                            } else {
                                let values: Vec<String> = a.values.iter().map(|v| match v.as_str() {
                                    "True" => "true".to_string(),
                                    "False" => "false".to_string(),
                                    other => other.to_string(),
                                }).collect();
                                view! {
                                    <select
                                        prop:value=move || get(&n2)
                                        on:change=move |ev| set(name.clone(), event_target_value(&ev))
                                    >
                                        <option value="">"(unknown)"</option>
                                        {values.into_iter().map(|v| view! { <option value=v.clone()>{v.clone()}</option> }).collect_view()}
                                    </select>
                                }.into_any()
                            };
                            view! {
                                <tr>
                                    <td class="mono">{a.name.clone()}</td>
                                    <td>{control}</td>
                                </tr>
                            }
                        }
                    </For>
                </tbody>
            </table>
        </div>
    }
}

pub fn event_target<T: JsCast>(ev: &leptos::ev::Event) -> T {
    ev.target().unwrap().unchecked_into::<T>()
}
