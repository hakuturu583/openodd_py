//! The binding editor: how each attribute of the ODD being edited is
//! measured, and the connections from the ODD through the binding and the
//! backend's manifest to its fuzzy rules.
//!
//! The overview follows every attribute along that chain in one table; an
//! attribute's page draws its chain -- the ODD's buckets, the probe that
//! measures them, the values the backend's knob writes for each, the rule
//! inputs that read those, and whether the rules' terms say the same as the
//! buckets -- above the form of its binding.

use leptos::prelude::*;
use leptos::task::spawn_local;
use web_sys::HtmlInputElement;

use crate::app::{ctx, event_target, Mode, ModeSwitch};
use crate::binding::{self, alignment, BindingDoc, Entry, Link, Manifest, Source};
use crate::doc::KindTag;
use crate::icons::Icon;
use crate::io;
use crate::rules_app::{rctx, shape_of};
use crate::rules_doc::{num, Shape};

pub const WEATHER_ODD: &str = include_str!("../examples/weather.yaml");
pub const WEATHER_BINDING: &str = include_str!("../examples/carla_weather.binding.yaml");
pub const CARLA_MANIFEST: &str = include_str!("../examples/carla.manifest.yaml");

const DRAFT: &str = "openodd-editor/binding-draft";
const DRAFT_NAME: &str = "openodd-editor/binding-name";
const MANIFEST: &str = "openodd-editor/manifest";
const MANIFEST_NAME: &str = "openodd-editor/manifest-name";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BSel {
    Overview,
    File,
    Attribute(String),
}

/// The binding editor's state, shared by every editor of the studio.
#[derive(Clone, Copy)]
pub struct BCtx {
    pub doc: RwSignal<BindingDoc>,
    pub name: RwSignal<String>,
    /// The backend's manifest, and the file it came from.
    pub manifest: RwSignal<Option<(String, Manifest)>>,
    manifest_text: RwSignal<String>,
    pub yaml: Memo<Result<String, String>>,
    /// openodd's reading of the binding: (attribute, what measures it), or
    /// why it does not read.
    pub checked: Memo<Result<Vec<(String, String)>, String>>,
    pub sel: RwSignal<BSel>,
    pub notice: RwSignal<Option<(bool, String)>>,
    /// The ODD's attributes that are measured (not derived): name, bucket
    /// labels, kind.
    pub attributes: Memo<Vec<(String, Vec<String>, KindTag)>>,
    pub links: Memo<Vec<Link>>,
}

pub fn bctx() -> BCtx {
    expect_context::<BCtx>()
}

/// The binding editor's state, kept while another editor is shown.
pub fn binding_context() -> BCtx {
    let odd = ctx();
    let (name, text) = match (io::load_text(DRAFT_NAME), io::load_text(DRAFT)) {
        (Some(n), Some(t)) => (n, t),
        _ => ("binding.yaml".into(), String::new()),
    };
    let doc = BindingDoc::from_yaml(&text).unwrap_or_default();
    let doc = RwSignal::new(doc);
    let name = RwSignal::new(name);
    let manifest_text = RwSignal::new(io::load_text(MANIFEST).unwrap_or_default());
    let manifest = RwSignal::new(
        Manifest::from_text(&manifest_text.get_untracked())
            .ok()
            .map(|m| (io::load_text(MANIFEST_NAME).unwrap_or_default(), m)),
    );
    let yaml = Memo::new(move |_| doc.with(BindingDoc::to_yaml));
    let checked = Memo::new(move |_| {
        let text = yaml.get()?;
        let odd_text = odd.yaml.get()?;
        let files = crate::check::Files {
            entry: odd.name.get(),
            text: odd_text,
            library: odd.library.get(),
        };
        manifest.with(|m| {
            binding::check(&files, &name.get(), &text, m.as_ref().map(|(_, m)| m)).map(|l| {
                l.odd
                    .attributes
                    .iter()
                    .map(|a| (a.name.clone(), format!("{:?}", a.probe)))
                    .collect()
            })
        })
    });
    let attributes = Memo::new(move |_| -> Vec<(String, Vec<String>, KindTag)> {
        odd.report.with(|r| {
            r.as_ref()
                .map(|r| {
                    r.attributes
                        .iter()
                        .filter(|a| !a.derived())
                        .map(|a| (a.name.clone(), a.buckets.clone(), a.kind))
                        .collect()
                })
                .unwrap_or_default()
        })
    });
    let links = Memo::new(move |_| {
        let attrs: Vec<(String, Vec<String>)> = attributes
            .get()
            .into_iter()
            .map(|(n, l, _)| (n, l))
            .collect();
        doc.with(|d| manifest.with(|m| binding::links(&attrs, d, m.as_ref().map(|(_, m)| m))))
    });
    Effect::new(move |_| {
        if let Ok(text) = yaml.get() {
            io::save_text(DRAFT, &text);
            io::save_text(DRAFT_NAME, &name.get());
        }
    });
    Effect::new(move |_| {
        io::save_text(MANIFEST, &manifest_text.get());
        io::save_text(
            MANIFEST_NAME,
            &manifest.with(|m| m.as_ref().map(|(n, _)| n.clone()).unwrap_or_default()),
        );
    });
    BCtx {
        doc,
        name,
        manifest,
        manifest_text,
        yaml,
        checked,
        sel: RwSignal::new(BSel::Overview),
        notice: RwSignal::new(None),
        attributes,
        links,
    }
}

impl BCtx {
    fn open(&self, name: &str, text: &str) -> bool {
        match BindingDoc::from_yaml(text) {
            Ok(doc) => {
                self.doc.set(doc);
                self.name.set(name.to_string());
                self.sel.set(BSel::Overview);
                self.notice.set(Some((true, format!("Opened {name}"))));
                true
            }
            Err(e) => {
                self.notice.set(Some((false, format!("{name}: {e}"))));
                false
            }
        }
    }

    fn open_manifest(&self, name: &str, text: &str) {
        match Manifest::from_text(text) {
            Ok(m) => {
                self.manifest.set(Some((name.to_string(), m)));
                self.manifest_text.set(text.to_string());
                self.notice
                    .set(Some((true, format!("Read the {name} manifest"))));
            }
            Err(e) => self.notice.set(Some((false, format!("{name}: {e}")))),
        }
    }

    fn update_entry(&self, id: u64, f: impl FnOnce(&mut Entry)) {
        self.doc.update(|d| {
            if let Some(e) = d.entry_mut(id) {
                f(e)
            }
        })
    }

    /// Measure `attribute` by the probe its name suggests, or one named
    /// like it in the manifest.
    fn bind(&self, attribute: &str) {
        let tail = attribute
            .rsplit('.')
            .next()
            .unwrap_or(attribute)
            .to_string();
        let probe = self
            .manifest
            .with(|m| {
                m.as_ref().and_then(|(_, m)| {
                    m.probes
                        .iter()
                        .find(|p| p.name == tail || p.name.starts_with(&tail))
                        .map(|p| p.name.clone())
                })
            })
            .unwrap_or(tail);
        self.doc.update(|d| {
            if d.entry(attribute).is_none() {
                d.entries.push(Entry::new(attribute.to_string(), probe));
            }
        });
    }

    /// The ODD attribute that sets the rule input `input`, if the binding
    /// and the manifest say which.
    pub fn attribute_of_input(&self, input: &str) -> Option<String> {
        self.links.with(|ls| {
            ls.iter()
                .find(|l| l.inputs.iter().any(|i| i == input))
                .map(|l| l.attribute.clone())
        })
    }

    /// The link of the attribute that sets `input`.
    pub fn link_of_input(&self, input: &str) -> Option<Link> {
        self.links.with(|ls| {
            ls.iter()
                .find(|l| l.inputs.iter().any(|i| i == input))
                .cloned()
        })
    }
}

/// A rule base reading an input: (quantity, rule base, the input's terms).
type Reader = (String, String, Vec<(String, Shape)>);

/// The rule bases that read `input`.
fn readers_of(input: &str) -> Vec<Reader> {
    let r = rctx();
    r.good.with(|g| {
        let Some(set) = g else { return vec![] };
        let mut out = vec![];
        for q in set.quantities.values() {
            for b in &q.rule_bases {
                for i in b.inputs.iter().filter(|i| i.name == input) {
                    let terms = i
                        .terms
                        .iter()
                        .map(|(n, m)| (n.clone(), shape_of(m)))
                        .collect();
                    out.push((q.name.clone(), b.name.clone(), terms));
                }
            }
        }
        out
    })
}

/// Show rule base `base` of `quantity` in the rules editor.
fn open_in_rules(quantity: &str, base: &str) {
    let r = rctx();
    let ids = r.doc.with(|d| {
        let q = d.quantities.iter().find(|q| q.name == quantity)?;
        let b = q.bases.iter().find(|b| b.name == base)?;
        Some((q.id, b.id))
    });
    if let Some((q, b)) = ids {
        r.sel.set(crate::rules_app::RSel::Base(q, b));
    }
    let mode = expect_context::<RwSignal<Mode>>();
    mode.set(Mode::Rules);
    if let Some(w) = web_sys::window() {
        let _ = w.location().set_hash("rules");
    }
}

/// Load the CARLA weather example everywhere: the ODD, its binding, the
/// backend's manifest and its rules.
fn load_weather_example() {
    let b = bctx();
    ctx().open("weather.yaml", WEATHER_ODD);
    rctx().open_text("weather_rules.yaml", crate::rules_app::SAMPLE);
    b.open("carla_weather.binding.yaml", WEATHER_BINDING);
    b.open_manifest("carla.manifest.yaml", CARLA_MANIFEST);
    b.notice.set(Some((
        true,
        "Loaded the CARLA weather example: ODD, binding, manifest and rules".into(),
    )));
}

#[component]
pub fn BindingStudio() -> impl IntoView {
    let b = bctx();
    view! {
        <div class="app">
            <BindingToolbar/>
            <main class="layout">
                <aside class="outline">
                    <BindingOutline/>
                </aside>
                <section class="center">
                    {move || b.checked.get().err().map(|e| view! {
                        <div class="card error-card">
                            <div class="card-title"><Icon name="warn"/>"openodd does not read this binding"</div>
                            <div class="mono">{e}</div>
                        </div>
                    })}
                    {move || match b.sel.get() {
                        BSel::Overview => view! { <Overview/> }.into_any(),
                        BSel::File => view! { <FileView/> }.into_any(),
                        BSel::Attribute(a) => view! { <AttributeView name=a/> }.into_any(),
                    }}
                </section>
                <aside class="panel">
                    <YamlPanel/>
                </aside>
            </main>
        </div>
    }
}

#[component]
fn BindingToolbar() -> impl IntoView {
    let b = bctx();
    let open = move |ev: leptos::ev::Event| {
        let input: HtmlInputElement = event_target(&ev);
        spawn_local(async move {
            if let Some((name, text)) = io::read_files(&input).await.into_iter().next() {
                b.open(&name, &text);
            }
        });
    };
    let open_manifest = move |ev: leptos::ev::Event| {
        let input: HtmlInputElement = event_target(&ev);
        spawn_local(async move {
            if let Some((name, text)) = io::read_files(&input).await.into_iter().next() {
                b.open_manifest(&name, &text);
            }
        });
    };
    let example = move |ev: leptos::ev::Event| {
        if event_target_value(&ev) == "weather" {
            load_weather_example();
        }
        let select: web_sys::HtmlSelectElement = event_target(&ev);
        select.set_value("");
    };
    let save = move |_| match b.yaml.get() {
        Ok(text) => io::download(&b.name.get(), &text),
        Err(e) => b.notice.set(Some((false, e))),
    };
    let status = move || match b.checked.get() {
        Ok(_) => view! {
            <span class="badge ok" title="openodd reads this binding over the ODD">
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
                <input class="filename" aria-label="File name" prop:value=move || b.name.get()
                    on:change=move |ev| b.name.set(event_target_value(&ev))/>
            </div>
            <label class="button">
                <Icon name="upload"/>"Open"
                <input type="file" accept=".yaml,.yml" class="hidden" on:change=open/>
            </label>
            <label class="button" title="The backend's manifest: what Backend.manifest() writes">
                <Icon name="module"/>"Backend manifest"
                <input type="file" accept=".yaml,.yml,.json" class="hidden" on:change=open_manifest/>
            </label>
            <select class="button" on:change=example aria-label="Examples">
                <option value="" selected>"Examples…"</option>
                <option value="weather">"CARLA weather: ODD, binding, manifest, rules"</option>
            </select>
            <button class="button primary" on:click=save><Icon name="download"/>"Download YAML"</button>
            <div class="status">{status}</div>
            {move || b.notice.get().map(|(ok, text)| view! {
                <div class=if ok { "notice ok" } else { "notice bad" } on:click=move |_| b.notice.set(None)>{text}</div>
            })}
        </header>
    }
}

#[component]
fn BindingOutline() -> impl IntoView {
    let b = bctx();
    let selected = move |s: BSel| b.sel.with(|x| *x == s);
    view! {
        <nav>
            <ul class="tree">
                <li class="row big" class:selected=move || selected(BSel::Overview) on:click=move |_| b.sel.set(BSel::Overview)>
                    <span class="icon"><Icon name="graph"/></span>
                    <span class="name">"Connections"</span>
                </li>
                <li class="row" class:selected=move || selected(BSel::File) on:click=move |_| b.sel.set(BSel::File)>
                    <span class="icon"><Icon name="doc"/></span>
                    <span class="name">"Binding & backend"</span>
                </li>
            </ul>
            <div class="heading"><span>"Attributes"</span></div>
            <ul class="tree">
                {move || b.attributes.get().into_iter().map(|(name, _, _)| {
                    let n2 = name.clone();
                    let n3 = name.clone();
                    let bound = Signal::derive(move || b.doc.with(|d| d.entry(&n2).is_some()));
                    let linked = {
                        let n = name.clone();
                        move || b.links.with(|ls| ls.iter().any(|l| l.attribute == n && !l.inputs.is_empty()))
                    };
                    let sel = BSel::Attribute(name.clone());
                    let sel2 = sel.clone();
                    view! {
                        <li class="row" class:selected=move || selected(sel.clone()) on:click=move |_| b.sel.set(sel2.clone())>
                            <span class="icon"><Icon name="ruler"/></span>
                            <span class="name">{n3}</span>
                            {move || linked().then(|| view! { <span class="icon" title="Its knob feeds the fuzzy rules"><Icon name="func"/></span> })}
                            <span class=move || if bound.get() { "truth-dot true" } else { "truth-dot" } title=move || if bound.get() { "measured" } else { "not measured" }></span>
                        </li>
                    }
                }).collect_view()}
                {move || b.attributes.with(Vec::is_empty).then(|| view! {
                    <li class="empty">"The ODD has no attribute to measure yet."</li>
                })}
            </ul>
        </nav>
    }
}

/// Each attribute followed along the chain, in one table.
#[component]
fn Overview() -> impl IntoView {
    let b = bctx();
    let odd = ctx();
    let rows = move || {
        let links = b.links.get();
        b.attributes.get().into_iter().map(|(name, labels, _)| {
            let link = links.iter().find(|l| l.attribute == name).cloned();
            let n2 = name.clone();
            let n3 = name.clone();
            let probe = link.as_ref().and_then(|l| l.probe.clone());
            let entry_source = b.doc.with(|d| d.entry(&name).map(|e| match &e.source {
                Source::Probe(p) => p.clone(),
                Source::Reader(k, v) => format!("{k}: {v}"),
            }));
            let knob = link.as_ref().and_then(|l| l.knob.clone());
            let inputs = link.as_ref().map(|l| l.inputs.clone()).unwrap_or_default();
            let readers: Vec<(String, String)> = inputs
                .iter()
                .flat_map(|i| readers_of(i).into_iter().map(|(q, base, _)| (q, base)))
                .collect();
            let labels_view = labels.iter().map(|l| view! { <span class="tag">{l.clone()}</span> }).collect_view();
            view! {
                <tr>
                    <td><button class="pill" on:click=move |_| b.sel.set(BSel::Attribute(n2.clone()))><Icon name="ruler"/>{n3}</button></td>
                    <td><div class="chips">{labels_view}</div></td>
                    <td>{match entry_source {
                        Some(s) => view! { <span class="mono">{s}</span> }.into_any(),
                        None => {
                            let n4 = name.clone();
                            view! { <button class="mini" on:click=move |_| b.bind(&n4)><Icon name="plus"/>"Bind"</button> }.into_any()
                        }
                    }}</td>
                    <td>{match (&knob, probe.is_some()) {
                        (Some(k), _) => view! {
                            <div class="mono">{k.key.clone()}</div>
                            <div class="hint">{k.values.iter().map(|(l, v)| format!("{l} {}", v.words())).collect::<Vec<_>>().join(" · ")}</div>
                        }.into_any(),
                        (None, true) => view! { <span class="hint">"no knob in the manifest"</span> }.into_any(),
                        (None, false) => view! { <span class="hint">"—"</span> }.into_any(),
                    }}</td>
                    <td>{if inputs.is_empty() {
                        view! { <span class="hint">"—"</span> }.into_any()
                    } else {
                        view! { <span class="mono">{inputs.join(", ")}</span> }.into_any()
                    }}</td>
                    <td>{readers.into_iter().map(|(q, base)| {
                        let (q2, b2) = (q.clone(), base.clone());
                        view! { <button class="pill" on:click=move |_| open_in_rules(&q2, &b2)><Icon name="func"/>{format!("{q} · {base}")}</button> }
                    }).collect_view()}</td>
                </tr>
            }
        }).collect_view()
    };
    view! {
        <div class="editor-head">
            <div class="big-icon root"><Icon name="graph"/></div>
            <div class="head-text">
                <div class="crumbs"><span class="last">"Connections"</span></div>
                <div class="kind-line">{move || format!("{} · {} · {}", odd.name.get(), b.name.get(),
                    b.manifest.with(|m| m.as_ref().map(|(n, m)| format!("{n} ({})", m.name)).unwrap_or_else(|| "no backend manifest".into())))}</div>
            </div>
        </div>
        <div class="card">
            <div class="chain-legend">
                <div><b>"ODD"</b><span class="hint">"what is talked about: the attribute and its buckets"</span></div>
                <span class="arrow">"→"</span>
                <div><b>"Binding"</b><span class="hint">"how it is measured: the backend's probe"</span></div>
                <span class="arrow">"→"</span>
                <div><b>"Backend"</b><span class="hint">"how a run sets each bucket: the knob and its values"</span></div>
                <span class="arrow">"→"</span>
                <div><b>"Fuzzy rules"</b><span class="hint">"the inputs that read what the knob writes"</span></div>
            </div>
            {move || b.manifest.with(Option::is_none).then(|| view! {
                <p class="hint">"Load the backend's manifest (what " <code>"Backend.manifest()"</code> " writes) to follow the attributes to the knobs and the rules, or try the CARLA weather example."</p>
                <button class="button subtle" on:click=move |_| load_weather_example()><Icon name="sparkle"/>"Load the CARLA weather example"</button>
            })}
        </div>
        <div class="card table-scroll">
            <table class="rules-table connections">
                <thead><tr><th>"ODD attribute"</th><th>"Buckets"</th><th>"Binding"</th><th>"Backend knob"</th><th>"Rule inputs"</th><th>"Read by"</th></tr></thead>
                <tbody>{rows}</tbody>
            </table>
        </div>
    }
}

#[component]
fn FileView() -> impl IntoView {
    let b = bctx();
    let odd = ctx();
    let set_text = move |f: fn(&mut BindingDoc, String)| {
        move |ev: leptos::ev::Event| {
            let v = event_target_value(&ev);
            b.doc.update(|d| f(d, v));
        }
    };
    let openodd = move || b.doc.with(|d| d.openodd.join(", "));
    let points_elsewhere = move || {
        let n = odd.name.get();
        b.doc.with(|d| !d.openodd.contains(&n))
    };
    view! {
        <div class="editor-head">
            <div class="big-icon"><Icon name="doc"/></div>
            <div class="head-text">
                <div class="crumbs"><span class="last">{move || b.name.get()}</span></div>
                <div class="kind-line">"binding: how each attribute of the ODD is measured"</div>
            </div>
        </div>
        <div class="card">
            <div class="grid2">
                <label class="field"><span>"Name"</span><input prop:value=move || b.doc.with(|d| d.name.clone()) on:change=set_text(|d, v| d.name = v)/></label>
                <label class="field"><span>"Measures the ODD in"</span><input prop:value=openodd on:change=set_text(|d, v| d.openodd = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())/></label>
            </div>
            <label class="field"><span>"Text"</span><input prop:value=move || b.doc.with(|d| d.text.clone()) on:change=set_text(|d, v| d.text = v)/></label>
            <Show when=points_elsewhere>
                <p class="hint">{move || format!("The ODD being edited is {}.", odd.name.get())}</p>
                <button class="button subtle" on:click=move |_| {
                    let n = odd.name.get();
                    b.doc.update(|d| d.openodd = vec![n]);
                }><Icon name="link"/>{move || format!("Measure {}", odd.name.get())}</button>
            </Show>
        </div>
        <div class="card">
            <div class="card-title"><Icon name="module"/>"Backend manifest"</div>
            {move || match b.manifest.get() {
                None => view! { <p class="hint">"None loaded: any probe name reads, and nothing follows the attributes past the binding. Python writes one with " <code>"backend.manifest()"</code> "."</p> }.into_any(),
                Some((file, m)) => view! {
                    <p class="hint">{format!("{file}: the {} backend, {} probes, {} knobs, {} rule inputs", m.name, m.probes.len(), m.knobs.len(), m.rule_inputs.len())}</p>
                    <table class="rules-table compact">
                        <thead><tr><th>"Probe"</th><th>"Unit"</th><th>"Knob"</th><th>"Says"</th></tr></thead>
                        <tbody>{m.probes.iter().map(|p| {
                            let knob = m.knob(&p.name).map(|k| k.key.clone()).unwrap_or_default();
                            view! { <tr><td class="mono">{p.name.clone()}</td><td>{p.unit.clone()}</td><td class="mono">{knob}</td><td class="hint">{p.text.clone()}</td></tr> }
                        }).collect_view()}</tbody>
                    </table>
                    <h3>"Rule inputs"</h3>
                    <table class="rules-table compact">
                        <thead><tr><th>"Input"</th><th>"Written by the knob"</th><th>"Says"</th></tr></thead>
                        <tbody>{m.rule_inputs.iter().map(|r| view! {
                            <tr><td class="mono">{r.name.clone()}</td><td class="mono">{r.knob.clone().unwrap_or_default()}</td><td class="hint">{r.text.clone()}</td></tr>
                        }).collect_view()}</tbody>
                    </table>
                    <div class="actions"><button class="mini danger" on:click=move |_| { b.manifest.set(None); b.manifest_text.set(String::new()); }><Icon name="x"/>"Forget the manifest"</button></div>
                }.into_any(),
            }}
        </div>
    }
}

/// An attribute's chain and the form of its binding.
#[component]
fn AttributeView(name: String) -> impl IntoView {
    let b = bctx();
    let n = StoredValue::new(name.clone());
    let attr = Memo::new(move |_| {
        b.attributes
            .with(|a| a.iter().find(|x| x.0 == n.get_value()).cloned())
    });
    let entry = Memo::new(move |_| b.doc.with(|d| d.entry(&n.get_value()).cloned()));
    let link = Memo::new(move |_| {
        b.links
            .with(|ls| ls.iter().find(|l| l.attribute == n.get_value()).cloned())
    });
    view! {
        <div class="editor-head">
            <div class="big-icon"><Icon name="ruler"/></div>
            <div class="head-text">
                <div class="crumbs"><span class="last">{name.clone()}</span></div>
                <div class="kind-line">{move || attr.get().map(|(_, l, k)| format!("{} · {} bucket{}", format!("{k:?}").to_lowercase(), l.len(), if l.len() == 1 { "" } else { "s" })).unwrap_or_default()}</div>
            </div>
        </div>
        <Chain name=name.clone() link=link entry=entry/>
        {move || match entry.get() {
            None => view! {
                <div class="card">
                    <p class="hint">"Not measured: no probe of the binding reads it."</p>
                    <button class="button primary" on:click=move |_| b.bind(&n.get_value())><Icon name="plus"/>"Measure it"</button>
                </div>
            }.into_any(),
            Some(e) => view! { <EntryForm id=e.id/> }.into_any(),
        }}
    }
}

#[component]
fn Chain(name: String, link: Memo<Option<Link>>, entry: Memo<Option<Entry>>) -> impl IntoView {
    let b = bctx();
    let n = StoredValue::new(name);
    let labels = Memo::new(move |_| {
        b.attributes.with(|a| {
            a.iter()
                .find(|x| x.0 == n.get_value())
                .map(|x| x.1.clone())
                .unwrap_or_default()
        })
    });
    move || {
        let link = link.get();
        let knob = link.as_ref().and_then(|l| l.knob.clone());
        let probe = entry.get().map(|e| match e.source {
            Source::Probe(p) => p,
            Source::Reader(k, v) => format!("{k}: {v}"),
        });
        let probe_info = probe.as_ref().and_then(|p| {
            b.manifest
                .with(|m| m.as_ref().and_then(|(_, m)| m.probe(p).cloned()))
        });
        let inputs = link.as_ref().map(|l| l.inputs.clone()).unwrap_or_default();
        let spans = link.as_ref().map(Link::spans).unwrap_or_default();
        view! {
            <div class="chain">
                <div class="chain-box">
                    <div class="chain-kind">"ODD"</div>
                    <div class="mono strong">{n.get_value()}</div>
                    <div class="chips">{labels.get().into_iter().map(|l| view! { <span class="tag">{l}</span> }).collect_view()}</div>
                </div>
                <span class="arrow">"→"</span>
                <div class="chain-box">
                    <div class="chain-kind">"Binding"</div>
                    {match &probe {
                        Some(p) => view! { <div class="mono strong">{p.clone()}</div> }.into_any(),
                        None => view! { <div class="hint">"not measured"</div> }.into_any(),
                    }}
                    {probe_info.map(|p| view! { <div class="hint">{p.text}</div> })}
                </div>
                <span class="arrow">"→"</span>
                <div class="chain-box">
                    <div class="chain-kind">"Backend knob"</div>
                    {match &knob {
                        Some(k) => view! {
                            <div class="mono strong">{k.key.clone()}</div>
                            <table class="mini-table">{k.values.iter().map(|(l, v)| view! { <tr><td>{l.clone()}</td><td class="num mono">{v.words()}</td></tr> }).collect_view()}</table>
                            {k.range.map(|(a, z)| view! { <div class="hint">{format!("range {} – {}", num(a), num(z))}</div> })}
                        }.into_any(),
                        None => view! { <div class="hint">"no knob: a run does not set it"</div> }.into_any(),
                    }}
                </div>
                <span class="arrow">"→"</span>
                <div class="chain-box wide">
                    <div class="chain-kind">"Fuzzy rules"</div>
                    {if inputs.is_empty() {
                        view! { <div class="hint">"no rule input reads what it sets"</div> }.into_any()
                    } else {
                        inputs.iter().map(|i| {
                            let readers = readers_of(i);
                            let input = i.clone();
                            view! {
                                <div class="mono strong">{input.clone()}</div>
                                {if readers.is_empty() {
                                    view! { <div class="hint">"no rule base reads it"</div> }.into_any()
                                } else {
                                    readers.into_iter().map(|(q, base, terms)| {
                                        let rows = alignment(&spans, &terms);
                                        let all_same = rows.iter().all(|r| r.2);
                                        let term_names = terms.iter().map(|t| t.0.clone()).collect::<Vec<_>>().join(", ");
                                        let (q2, b2) = (q.clone(), base.clone());
                                        view! {
                                            <div class="reader">
                                                <button class="pill" on:click=move |_| open_in_rules(&q2, &b2)><Icon name="func"/>{format!("{q} · {base}")}</button>
                                                <span class=if all_same { "tag ok" } else { "tag bad" }>{if all_same { "terms are the ODD's buckets" } else { "terms differ from the ODD's buckets" }}</span>
                                                <div class="hint">{format!("terms: {term_names}")}</div>
                                                <table class="mini-table">
                                                    <tr><th>"ODD bucket"</th><th>"written as"</th><th>"strongest term there"</th></tr>
                                                    {rows.into_iter().map(|(label, best, same)| {
                                                        let span = spans.iter().find(|s| s.0 == label).map(|s| if s.1 == s.2 { num(s.1) } else { format!("{} – {}", num(s.1), num(s.2)) }).unwrap_or_default();
                                                        view! { <tr class:bad=!same><td>{label}</td><td class="mono">{span}</td><td>{best.unwrap_or_else(|| "none fires".into())}</td></tr> }
                                                    }).collect_view()}
                                                </table>
                                            </div>
                                        }
                                    }).collect_view().into_any()
                                }}
                            }
                        }).collect_view().into_any()
                    }}
                </div>
            </div>
        }
    }
}

#[component]
fn EntryForm(id: u64) -> impl IntoView {
    let b = bctx();
    let entry = Memo::new(move |_| {
        b.doc
            .with(|d| d.entries.iter().find(|e| e.id == id).cloned())
    });
    let get = move |f: fn(&Entry) -> String| {
        move || entry.with(|e| e.as_ref().map(f).unwrap_or_default())
    };
    let kind = move || {
        entry.with(|e| match e.as_ref().map(|e| &e.source) {
            Some(Source::Reader(k, _)) => k.clone(),
            _ => "probe".into(),
        })
    };
    let set_kind = move |ev: leptos::ev::Event| {
        let k = event_target_value(&ev);
        b.update_entry(id, |e| {
            let value = match &e.source {
                Source::Probe(p) => p.clone(),
                Source::Reader(_, v) => v.clone(),
            };
            e.source = if k == "probe" {
                Source::Probe(value)
            } else {
                Source::Reader(k, value)
            };
        });
    };
    let set_source = move |ev: leptos::ev::Event| {
        let v = event_target_value(&ev).trim().to_string();
        b.update_entry(id, |e| match &mut e.source {
            Source::Probe(p) => *p = v,
            Source::Reader(_, x) => *x = v,
        });
    };
    let set_buckets = move |ev: leptos::ev::Event| {
        let t = event_target_value(&ev);
        let edges: Vec<f64> = t.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        b.update_entry(id, |e| e.buckets = (!edges.is_empty()).then_some(edges));
    };
    let set_values = move |ev: leptos::ev::Event| {
        let t = event_target_value(&ev);
        let vs: Vec<serde_norway::Value> = t
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                serde_norway::from_str(s).unwrap_or_else(|_| serde_norway::Value::String(s.into()))
            })
            .collect();
        b.update_entry(id, |e| e.values = (!vs.is_empty()).then_some(vs));
    };
    let set_opt = move |f: fn(&mut Entry, Option<String>)| {
        move |ev: leptos::ev::Event| {
            let t = event_target_value(&ev).trim().to_string();
            b.update_entry(id, |e| f(e, (!t.is_empty()).then_some(t)));
        }
    };
    let remove = move |_| b.doc.update(|d| d.entries.retain(|e| e.id != id));
    let probes = move || {
        b.manifest.with(|m| {
            m.as_ref()
                .map(|(_, m)| m.probes.iter().map(|p| p.name.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    };
    let readers = move || {
        b.manifest.with(|m| {
            m.as_ref()
                .map(|(_, m)| m.readers.clone())
                .unwrap_or_else(|| vec!["measure".into()])
        })
    };
    view! {
        <div class="card">
            <div class="card-head">
                <div class="card-title"><Icon name="link"/>"How it is measured"</div>
                <div class="actions"><button class="mini danger" on:click=remove><Icon name="trash"/>"Stop measuring"</button></div>
            </div>
            <div class="grid2">
                <label class="field"><span>"By"</span>
                    <select prop:value=kind on:change=set_kind>
                        <option value="probe">"a probe"</option>
                        {move || readers().into_iter().map(|r| view! { <option value=r.clone()>{format!("a {r} reader")}</option> }).collect_view()}
                    </select>
                </label>
                <label class="field"><span>{move || if kind() == "probe" { "Probe" } else { "Reads" }}</span>
                    <input list="probe-names" prop:value=get(|e| match &e.source { Source::Probe(p) => p.clone(), Source::Reader(_, v) => v.clone() }) on:change=set_source/>
                    <datalist id="probe-names">{move || probes().into_iter().map(|p| view! { <option value=p/> }).collect_view()}</datalist>
                </label>
                <label class="field"><span>"Unit (when not the backend's)"</span><input prop:value=get(|e| e.unit.clone().unwrap_or_default()) on:change=set_opt(|e, v| e.unit = v)/></label>
                <label class="field"><span>"Text"</span><input prop:value=get(|e| e.text.clone()) on:change=set_opt(|e, v| e.text = v.unwrap_or_default())/></label>
                <label class="field"><span>"Bucket edges (a number)"</span><input placeholder="0, 30, 50" prop:value=get(|e| e.buckets.as_ref().map(|b| b.iter().map(|x| num(*x)).collect::<Vec<_>>().join(", ")).unwrap_or_default()) on:change=set_buckets/></label>
                <label class="field"><span>"Values (what the probe returns)"</span><input placeholder="urban, nonurban" prop:value=get(|e| e.values.as_ref().map(|v| v.iter().map(|x| serde_norway::to_string(x).unwrap_or_default().trim().to_string()).collect::<Vec<_>>().join(", ")).unwrap_or_default()) on:change=set_values/></label>
                <label class="field"><span>"Cover by"</span>
                    <select prop:value=get(|e| e.cover_by.clone().unwrap_or_default()) on:change=set_opt(|e, v| e.cover_by = v)>
                        <option value="">"default"</option>
                        {openodd::items::COVER_MEASURES.iter().map(|m| view! { <option value=*m>{*m}</option> }).collect_view()}
                    </select>
                </label>
                <label class="field"><span>"Least stay, s"</span><input inputmode="decimal" prop:value=get(|e| e.min_stay.map(num).unwrap_or_default()) on:change=set_opt(|e, v| e.min_stay = v.and_then(|v| v.parse().ok()))/></label>
            </div>
        </div>
    }
}

#[component]
fn YamlPanel() -> impl IntoView {
    let b = bctx();
    let draft = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let text = move || {
        draft
            .get()
            .unwrap_or_else(|| b.yaml.get().unwrap_or_else(|e| format!("# {e}")))
    };
    let apply = move |_| {
        if let Some(t) = draft.get() {
            match BindingDoc::from_yaml(&t) {
                Ok(doc) => {
                    b.doc.set(doc);
                    draft.set(None);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
        }
    };
    view! {
        <div class="tabs"><button class="tab active"><Icon name="code"/>"YAML"</button></div>
        <div class="tab-body yaml">
            <div class="hint">"The binding file. Type here to edit it directly, then apply."</div>
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
