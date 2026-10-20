//! The forms: the document, a concept, a module, and the lines of a section,
//! each drawn with what it does to the ODD.
//!
//! Every form reads and writes the one document signal. Lists are keyed by
//! their rows' ids and re-rendered only when rows come or go (a `Memo` of the
//! ids), so typing into a row keeps its focus.

use leptos::prelude::*;
use leptos::task::spawn_local;
use web_sys::HtmlInputElement;

use crate::app::{add_concept, ctx, event_target, AttributeCard, Sel};
use crate::check::{Attribute, State};
use crate::doc::{
    fresh_id, move_in, tail, Cond, CondValue, CondsAt, Definition, Document, KeyKind, Kind,
    KindTag, Section, ValueTag, OPERATORS, PRIMITIVES, UNIT_TYPES,
};
use crate::icons::{concept_icon, kind_icon, Icon};
use crate::viz::{fmt_num, Domain, Legend, NumberLine};

/// A change to part of the document.
type Edit<T> = Box<dyn FnOnce(&mut T)>;

/// Text in a list, comma separated.
fn split_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The keys a section may name: every leaf concept, the modules and labels.
fn key_options(d: &Document) -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = d
        .leaves()
        .into_iter()
        .map(|(n, k)| (n, k.tag().label()))
        .collect();
    out.extend(d.modules.iter().map(|m| (m.name.clone(), "module")));
    out.extend(d.labels().into_iter().map(|l| (l, "label")));
    out
}

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

#[component]
pub fn Settings() -> impl IntoView {
    let ctx = ctx();
    let add_library = move |ev: leptos::ev::Event| {
        let input: HtmlInputElement = event_target(&ev);
        spawn_local(async move {
            let files = crate::io::read_files(&input).await;
            ctx.library.update(|lib| {
                for (name, text) in files {
                    lib.retain(|(n, _)| *n != name);
                    lib.push((name, text));
                }
            });
        });
    };
    let imports = Memo::new(move |_| ctx.doc.with(|d| d.imports.len()));
    view! {
        <div class="card">
            <div class="card-title"><Icon name="doc"/>"Document"</div>
            <p class="hint">
                "An ASAM OpenODD 1.0 document: the concepts of the "
                <b>"taxonomy"</b>", the "<b>"modules"</b>" that bound them, and the "<b>"ODD"</b>
                " modules that are its roots."
            </p>
            <label class="field">
                <span>"File name"</span>
                <input
                    prop:value=move || ctx.name.get()
                    on:change=move |ev| ctx.name.set(event_target_value(&ev))
                />
            </label>
        </div>
        <div class="card">
            <div class="card-title"><Icon name="link"/>"IMPORT"</div>
            <p class="hint">"Other OpenODD files this one reads. Load them below so the check reads them too."</p>
            <ul class="list">
                {move || (0..imports.get()).map(|i| view! {
                    <li class="line">
                        <input
                            prop:value=move || ctx.doc.with(|d| d.imports.get(i).cloned().unwrap_or_default())
                            on:change=move |ev| {
                                let v = event_target_value(&ev);
                                ctx.doc.update(|d| if let Some(x) = d.imports.get_mut(i) { *x = v });
                            }
                        />
                        {move || {
                            let name = ctx.doc.with(|d| d.imports.get(i).cloned().unwrap_or_default());
                            let have = ctx.library.with(|l| l.iter().any(|(n, _)| *n == name));
                            view! { <span class=if have { "tag ok" } else { "tag bad" }>{if have { "loaded" } else { "not loaded" }}</span> }
                        }}
                        <button class="mini danger" title="Remove" on:click=move |_| ctx.doc.update(|d| { if i < d.imports.len() { d.imports.remove(i); } })><Icon name="x"/></button>
                    </li>
                }).collect_view()}
            </ul>
            <div class="actions">
                <button class="button" on:click=move |_| ctx.doc.update(|d| d.imports.push("taxonomy.yaml".into()))><Icon name="plus"/>"import"</button>
                <label class="button">
                    <Icon name="upload"/>"Load imported files…"
                    <input type="file" multiple accept=".yaml,.yml" class="hidden" on:change=add_library/>
                </label>
            </div>
            {move || {
                let lib = ctx.library.get();
                (!lib.is_empty()).then(|| view! {
                    <ul class="list">
                        {lib.into_iter().map(|(name, text)| {
                            let n = name.clone();
                            view! {
                                <li class="line">
                                    <Icon name="doc"/>
                                    <span class="mono">{name.clone()}</span>
                                    <span class="hint">{format!("{} lines", text.lines().count())}</span>
                                    <button class="mini" title="Edit this file instead" on:click=move |_| { ctx.open(&name, &text); }>"open"</button>
                                    <button class="mini danger" title="Remove" on:click=move |_| ctx.library.update(|l| l.retain(|(x, _)| *x != n))><Icon name="x"/></button>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                })
            }}
        </div>
        <div class="card">
            <div class="card-title"><Icon name="code"/>"Other sections"</div>
            <p class="hint">"conversion (units the document adds), COD, OD: kept as YAML."</p>
            <textarea
                class="code"
                spellcheck="false"
                placeholder="conversion:\n  length:\n    ft:\n      m: {scale: 0.3048}"
                prop:value=move || ctx.doc.with(|d| d.extra.clone())
                on:input=move |ev| {
                    let v = event_target_value(&ev);
                    ctx.doc.update(|d| d.extra = v);
                }
            ></textarea>
        </div>
    }
}

// ---------------------------------------------------------------------------
// A concept
// ---------------------------------------------------------------------------

#[component]
pub fn ConceptEditor(id: u64) -> impl IntoView {
    let ctx = ctx();
    let exists = Memo::new(move |_| ctx.doc.with(|d| d.concept(id).is_some()));
    let tag = Memo::new(move |_| ctx.doc.with(|d| d.concept(id).map(|c| c.kind.tag())));
    let path = Memo::new(move |_| ctx.doc.with(|d| d.concept_path(id).unwrap_or_default()));
    let name = move || {
        ctx.doc
            .with(|d| d.concept(id).map(|c| c.name.clone()).unwrap_or_default())
    };
    let rename = move |ev: leptos::ev::Event| {
        let new = event_target_value(&ev).trim().to_string();
        if new.is_empty() {
            return;
        }
        ctx.doc.update(|d| {
            let old_path = d.concept_path(id).unwrap_or_default();
            let Some(c) = d.concept_mut(id) else { return };
            let old = std::mem::replace(&mut c.name, new.clone());
            let new_path = format!(
                "{}{new}",
                old_path.strip_suffix(old.as_str()).unwrap_or_default()
            );
            d.rename_keys(&[(old_path, new_path), (old, new)]);
        });
    };
    let remove = move |_| {
        let parent = ctx.doc.with(|d| d.parent_of(id)).flatten();
        ctx.doc.update(|d| d.remove_concept(id));
        ctx.sel.set(parent.map(Sel::Concept).unwrap_or(Sel::Map));
    };
    let is_attribute = Memo::new(move |_| {
        let p = path.get();
        ctx.report
            .with(|r| r.as_ref().is_ok_and(|r| r.attribute(&p).is_some()))
    });
    move || {
        if !exists.get() {
            return view! { <div class="card"><p class="hint">"This concept is gone."</p></div> }
                .into_any();
        }
        let icon = move || {
            let t = tag.get().unwrap_or(KindTag::Other);
            view! { <Icon name=concept_icon(&name(), t)/> }
        };
        view! {
            <div class="editor-head">
                <span class="big-icon">{icon}</span>
                <div class="head-text">
                    <div class="crumbs">{move || {
                        let p = path.get();
                        let parts: Vec<String> = p.split('.').map(str::to_string).collect();
                        let n = parts.len();
                        parts.into_iter().enumerate().map(|(i, part)| view! {
                            <span class:last=i + 1 == n>{part}</span>
                            {(i + 1 < n).then(|| view! { <span class="sep">"›"</span> })}
                        }).collect_view()
                    }}</div>
                    <div class="kind-line">{move || tag.get().map(|t| t.label())}</div>
                </div>
                <div class="actions">
                    <button class="mini" title="Move up" on:click=move |_| ctx.doc.update(|d| d.move_concept(id, -1))><Icon name="up"/></button>
                    <button class="mini" title="Move down" on:click=move |_| ctx.doc.update(|d| d.move_concept(id, 1))><Icon name="down"/></button>
                    <button class="mini danger" title="Delete" on:click=remove><Icon name="trash"/></button>
                </div>
            </div>
            {move || is_attribute.get().then(|| view! {
                <div class="card preview">
                    <div class="card-title"><Icon name="map"/>"In the ODD"<span class="hint">"what the modules leave of it · drag or click to test"</span></div>
                    <Legend/>
                    <AttributeCard name=path.get()/>
                </div>
            })}
            <div class="card">
                <div class="grid2">
                    <label class="field">
                        <span>"Name"</span>
                        <input prop:value=name on:change=rename/>
                    </label>
                    <div class="field">
                        <span>"Kind"</span>
                        <KindPicker id=id tag=tag/>
                    </div>
                </div>
                {move || match tag.get() {
                    Some(KindTag::Number) => view! { <NumberForm id=id/> }.into_any(),
                    Some(KindTag::Categorical) => view! { <LiteralsForm id=id path=path/> }.into_any(),
                    Some(KindTag::Derived) => view! { <DerivedForm id=id/> }.into_any(),
                    Some(KindTag::Group) => view! { <GroupForm id=id/> }.into_any(),
                    Some(KindTag::Reference) => view! { <ReferenceForm id=id/> }.into_any(),
                    Some(KindTag::Other) => view! { <OtherForm id=id/> }.into_any(),
                    Some(KindTag::Boolean) => view! { <p class="hint">"true or false."</p> }.into_any(),
                    None => ().into_any(),
                }}
            </div>
        }
        .into_any()
    }
}

/// The kinds, as a row of icon buttons.
#[component]
fn KindPicker(id: u64, tag: Memo<Option<KindTag>>) -> impl IntoView {
    let ctx = ctx();
    view! {
        <div class="kind-picker">
            {KindTag::ALL.into_iter().map(|t| view! {
                <button
                    class:active=move || tag.get() == Some(t)
                    title=t.label()
                    on:click=move |_| ctx.doc.update(|d| if let Some(c) = d.concept_mut(id) { c.kind = c.kind.convert(t) })
                >
                    <Icon name=kind_icon(t)/>
                    <span>{t.label().split(' ').next().unwrap_or_default()}</span>
                </button>
            }).collect_view()}
        </div>
    }
}

/// Edit the concept's kind in place.
fn edit_kind(id: u64, f: impl FnOnce(&mut Kind)) {
    ctx().doc.update(|d| {
        if let Some(c) = d.concept_mut(id) {
            f(&mut c.kind);
        }
    });
}

#[component]
fn NumberForm(id: u64) -> impl IntoView {
    let ctx = ctx();
    let read = move |want_primitive: bool| {
        ctx.doc.with(|d| match d.concept(id).map(|c| &c.kind) {
            Some(Kind::Number {
                primitive,
                unit_type,
            }) => {
                if want_primitive {
                    primitive.clone()
                } else {
                    unit_type.clone()
                }
            }
            _ => String::new(),
        })
    };
    view! {
        <div class="grid2">
            <div class="field">
                <span>"Primitive"</span>
                <div class="segmented">
                    {PRIMITIVES.into_iter().map(|p| view! {
                        <button
                            class:active=move || read(true) == p
                            on:click=move |_| edit_kind(id, |k| if let Kind::Number { primitive, .. } = k { *primitive = p.into() })
                        >{p}</button>
                    }).collect_view()}
                </div>
            </div>
            <label class="field">
                <span>"Unit type"</span>
                <input
                    list="dl-unit-types"
                    placeholder="velocity, length, time…"
                    prop:value=move || read(false)
                    on:input=move |ev| {
                        let v = event_target_value(&ev);
                        edit_kind(id, |k| if let Kind::Number { unit_type, .. } = k { *unit_type = v });
                    }
                />
                <datalist id="dl-unit-types">
                    {UNIT_TYPES.into_iter().map(|u| view! { <option value=u/> }).collect_view()}
                </datalist>
            </label>
        </div>
        <p class="hint">"Conditions on it name a unit of this type (e.g. "<code>"<= 60 km/h"</code>"); openodd converts between units. Its buckets are cut where the modules draw lines."</p>
    }
}

#[component]
fn LiteralsForm(id: u64, path: Memo<String>) -> impl IntoView {
    let ctx = ctx();
    let count = Memo::new(move |_| {
        ctx.doc.with(|d| match d.concept(id).map(|c| &c.kind) {
            Some(Kind::Categorical(l)) => l.len(),
            _ => 0,
        })
    });
    let edit = move |f: Edit<Vec<String>>| {
        edit_kind(id, |k| {
            if let Kind::Categorical(l) = k {
                f(l)
            }
        })
    };
    let state_of = move |i: usize| {
        let p = path.get();
        ctx.report.with(|r| {
            r.as_ref()
                .ok()
                .and_then(|r| r.attribute(&p).and_then(|a| a.states.get(i).copied()))
        })
    };
    view! {
        <h3>"Literals"</h3>
        <p class="hint">"In order: a comparison like "<code>">= heavy"</code>" reads this order. The dot shows where the ODD stands on each."</p>
        <ul class="list literals">
            {move || (0..count.get()).map(|i| view! {
                <li class="line">
                    <span class=move || format!("state-dot {}", state_of(i).map(State::class).unwrap_or(""))></span>
                    <input
                        prop:value=move || ctx.doc.with(|d| match d.concept(id).map(|c| &c.kind) {
                            Some(Kind::Categorical(l)) => l.get(i).cloned().unwrap_or_default(),
                            _ => String::new(),
                        })
                        on:input=move |ev| {
                            let v = event_target_value(&ev);
                            edit(Box::new(move |l| if let Some(x) = l.get_mut(i) { *x = v }));
                        }
                    />
                    <button class="mini" title="Move up" on:click=move |_| edit(Box::new(move |l| if i > 0 { l.swap(i, i - 1) }))><Icon name="up"/></button>
                    <button class="mini" title="Move down" on:click=move |_| edit(Box::new(move |l| if i + 1 < l.len() { l.swap(i, i + 1) }))><Icon name="down"/></button>
                    <button class="mini danger" title="Remove" on:click=move |_| edit(Box::new(move |l| { if i < l.len() { l.remove(i); } }))><Icon name="x"/></button>
                </li>
            }).collect_view()}
        </ul>
        <div class="actions">
            <button class="button" on:click=move |_| edit(Box::new(|l| {
                let taken = l.clone();
                l.push(crate::app::unique("value", &taken));
            }))><Icon name="plus"/>"literal"</button>
        </div>
    }
}

#[component]
fn DerivedForm(id: u64) -> impl IntoView {
    let ctx = ctx();
    let ids = Memo::new(move |_| {
        ctx.doc.with(|d| match d.concept(id).map(|c| &c.kind) {
            Some(Kind::Derived(defs)) => defs.iter().map(|d| d.id).collect::<Vec<_>>(),
            _ => vec![],
        })
    });
    let edit = move |f: Edit<Vec<Definition>>| {
        edit_kind(id, |k| {
            if let Kind::Derived(defs) = k {
                f(defs)
            }
        })
    };
    view! {
        <h3>"Literals and their definitions"</h3>
        <p class="hint">"The first literal whose conditions all hold is the value."</p>
        <For each=move || ids.get() key=|x| *x let:lit>
            <div class="definition">
                <div class="line">
                    <Icon name="func"/>
                    <input
                        class="literal"
                        prop:value=move || ctx.doc.with(|d| match d.concept(id).map(|c| &c.kind) {
                            Some(Kind::Derived(defs)) => defs.iter().find(|x| x.id == lit).map(|x| x.literal.clone()).unwrap_or_default(),
                            _ => String::new(),
                        })
                        on:input=move |ev| {
                            let v = event_target_value(&ev);
                            edit(Box::new(move |defs| if let Some(x) = defs.iter_mut().find(|x| x.id == lit) { x.literal = v }));
                        }
                    />
                    <span class="hint">"when"</span>
                    <button class="mini" title="Move up" on:click=move |_| edit(Box::new(move |defs| move_in(defs, |x| x.id == lit, -1)))><Icon name="up"/></button>
                    <button class="mini" title="Move down" on:click=move |_| edit(Box::new(move |defs| move_in(defs, |x| x.id == lit, 1)))><Icon name="down"/></button>
                    <button class="mini danger" title="Remove" on:click=move |_| edit(Box::new(move |defs| defs.retain(|x| x.id != lit)))><Icon name="x"/></button>
                </div>
                <CondList at=CondsAt::Definition { concept: id, literal: lit }/>
            </div>
        </For>
        <div class="actions">
            <button class="button" on:click=move |_| edit(Box::new(|defs| {
                let taken: Vec<String> = defs.iter().map(|d| d.literal.clone()).collect();
                defs.push(Definition { id: fresh_id(), literal: crate::app::unique("value", &taken), conditions: vec![] });
            }))><Icon name="plus"/>"literal"</button>
        </div>
    }
}

#[component]
fn GroupForm(id: u64) -> impl IntoView {
    let ctx = ctx();
    let children = Memo::new(move |_| {
        ctx.doc.with(|d| match d.concept(id).map(|c| &c.kind) {
            Some(Kind::Group(cs)) => cs
                .iter()
                .map(|c| (c.id, c.name.clone(), c.kind.tag()))
                .collect::<Vec<_>>(),
            _ => vec![],
        })
    });
    view! {
        <h3>"Concepts in this group"</h3>
        <div class="child-grid">
            {move || children.get().into_iter().map(|(cid, name, tag)| {
                let icon = concept_icon(&name, tag);
                view! {
                    <button class="child" on:click=move |_| ctx.sel.set(Sel::Concept(cid))>
                        <Icon name=icon/>
                        <span>{name}</span>
                        <span class="hint">{tag.label()}</span>
                    </button>
                }
            }).collect_view()}
        </div>
        <div class="actions">
            <button class="button" on:click=move |_| add_concept(Some(id), KindTag::Categorical)><Icon name="plus"/>"concept"</button>
            <button class="button" on:click=move |_| add_concept(Some(id), KindTag::Group)><Icon name="folder"/>"group"</button>
        </div>
    }
}

#[component]
fn ReferenceForm(id: u64) -> impl IntoView {
    let ctx = ctx();
    let options = Memo::new(move |_| {
        ctx.doc.with(|d| {
            d.leaves()
                .into_iter()
                .filter(|(_, k)| matches!(k.tag(), KindTag::Categorical | KindTag::Derived))
                .map(|(n, _)| tail(&n).to_string())
                .collect::<Vec<_>>()
        })
    });
    view! {
        <label class="field">
            <span>"Type id"</span>
            <input
                list="dl-types"
                prop:value=move || ctx.doc.with(|d| match d.concept(id).map(|c| &c.kind) {
                    Some(Kind::Reference(r)) => r.clone(),
                    _ => String::new(),
                })
                on:input=move |ev| {
                    let v = event_target_value(&ev);
                    edit_kind(id, |k| if let Kind::Reference(r) = k { *r = v });
                }
            />
            <datalist id="dl-types">
                {move || options.get().into_iter().map(|o| view! { <option value=o/> }).collect_view()}
            </datalist>
        </label>
        <p class="hint">"This concept takes the literals of the categorical it names."</p>
    }
}

#[component]
fn OtherForm(id: u64) -> impl IntoView {
    let ctx = ctx();
    view! {
        <label class="field">
            <span>"YAML"</span>
            <textarea
                class="code"
                spellcheck="false"
                prop:value=move || ctx.doc.with(|d| match d.concept(id).map(|c| &c.kind) {
                    Some(Kind::Other(t)) => t.clone(),
                    _ => String::new(),
                })
                on:input=move |ev| {
                    let v = event_target_value(&ev);
                    edit_kind(id, |k| if let Kind::Other(t) = k { *t = v });
                }
            ></textarea>
        </label>
    }
}

// ---------------------------------------------------------------------------
// A module
// ---------------------------------------------------------------------------

#[component]
pub fn ModuleEditor(id: u64) -> impl IntoView {
    let ctx = ctx();
    let exists = Memo::new(move |_| ctx.doc.with(|d| d.module(id).is_some()));
    let read = move |f: fn(&crate::doc::Module) -> String| {
        ctx.doc.with(|d| d.module(id).map(f).unwrap_or_default())
    };
    let edit = move |f: Edit<crate::doc::Module>| {
        ctx.doc.update(|d| {
            if let Some(m) = d.module_mut(id) {
                f(m)
            }
        })
    };
    let rename = move |ev: leptos::ev::Event| {
        let new = event_target_value(&ev).trim().to_string();
        if new.is_empty() {
            return;
        }
        ctx.doc.update(|d| {
            let Some(m) = d.module_mut(id) else { return };
            let old = std::mem::replace(&mut m.name, new.clone());
            d.rename_keys(&[(old, new)]);
        });
    };
    let root = Memo::new(move |_| ctx.doc.with(|d| d.module(id).is_some_and(|m| m.root)));
    let active = Memo::new(move |_| ctx.doc.with(|d| d.module(id).is_some_and(|m| m.active)));
    let meta_count = Memo::new(move |_| {
        ctx.doc
            .with(|d| d.module(id).map_or(0, |m| m.metadata.len()))
    });
    let remove = move |_| {
        ctx.doc.update(|d| d.modules.retain(|m| m.id != id));
        ctx.sel.set(Sel::Map);
    };
    let truth = move || {
        let name = read(|m| m.name.clone());
        ctx.evaluation
            .with(|e| e.as_ref().and_then(|e| e.truth(&name).map(str::to_string)))
    };
    move || {
        if !exists.get() {
            return view! { <div class="card"><p class="hint">"This module is gone."</p></div> }
                .into_any();
        }
        view! {
            <div class="editor-head">
                <span class="big-icon" class:root=root><Icon name=if root.get() { "target" } else { "module" }/></span>
                <div class="head-text">
                    <div class="crumbs"><span class="last">{move || read(|m| m.name.clone())}</span></div>
                    <div class="kind-line">
                        {move || if root.get() { "ODD root" } else { "module" }}
                        {move || (!read(|m| m.title.clone()).is_empty()).then(|| format!(" · {}", read(|m| m.title.clone())))}
                    </div>
                </div>
                {move || truth().map(|t| view! { <span class=format!("truth big {t}")>{format!("now: {t}")}</span> })}
                <div class="actions">
                    <button class="mini" title="Move up" on:click=move |_| ctx.doc.update(|d| move_in(&mut d.modules, |m| m.id == id, -1))><Icon name="up"/></button>
                    <button class="mini" title="Move down" on:click=move |_| ctx.doc.update(|d| move_in(&mut d.modules, |m| m.id == id, 1))><Icon name="down"/></button>
                    <button class="mini danger" title="Delete" on:click=remove><Icon name="trash"/></button>
                </div>
            </div>
            <Accepts id=id/>
            <SectionForm id=id include=true/>
            <SectionForm id=id include=false/>
            <div class="card">
                <div class="card-title"><Icon name="tag"/>"About this module"</div>
                <div class="grid2">
                    <label class="field">
                        <span>"Id"</span>
                        <input prop:value=move || read(|m| m.name.clone()) on:change=rename/>
                    </label>
                    <div class="field">
                        <span>"Role"</span>
                        <div class="segmented">
                            <button class:active=move || root.get() on:click=move |_| edit(Box::new(|m| m.root = true))><Icon name="target"/>"ODD root"</button>
                            <button class:active=move || !root.get() on:click=move |_| edit(Box::new(|m| m.root = false))><Icon name="module"/>"module"</button>
                        </div>
                    </div>
                    <label class="field">
                        <span>"Title"</span>
                        <input
                            prop:value=move || read(|m| m.title.clone())
                            on:input=move |ev| { let v = event_target_value(&ev); edit(Box::new(move |m| m.title = v)); }
                        />
                    </label>
                    <label class="field">
                        <span>"Labels (comma separated)"</span>
                        <input
                            prop:value=move || read(|m| m.labels.join(", "))
                            on:change=move |ev| { let v = split_list(&event_target_value(&ev)); edit(Box::new(move |m| m.labels = v)); }
                        />
                    </label>
                </div>
                <label class="field">
                    <span>"Description"</span>
                    <textarea
                        rows="2"
                        prop:value=move || read(|m| m.description.clone())
                        on:input=move |ev| { let v = event_target_value(&ev); edit(Box::new(move |m| m.description = v)); }
                    ></textarea>
                </label>
                <div class="switch-line">
                    <Switch on=Signal::derive(move || active.get()) set=Callback::new(move |b: bool| edit(Box::new(move |m| m.active = b)))/>
                    <span>"Active"</span>
                    <span class="hint">"an inactive module is left out of the ODD"</span>
                </div>
                <h3>"Metadata"</h3>
                <ul class="list">
                    {move || (0..meta_count.get()).map(|i| {
                        let get = move |key: bool| ctx.doc.with(|d| d.module(id).and_then(|m| m.metadata.get(i)).map(|(k, v)| if key { k.clone() } else { v.clone() }).unwrap_or_default());
                        view! {
                            <li class="line">
                                <input class="key" placeholder="key" prop:value=move || get(true)
                                    on:input=move |ev| { let v = event_target_value(&ev); edit(Box::new(move |m| if let Some(e) = m.metadata.get_mut(i) { e.0 = v })); }/>
                                <input placeholder="value" prop:value=move || get(false)
                                    on:input=move |ev| { let v = event_target_value(&ev); edit(Box::new(move |m| if let Some(e) = m.metadata.get_mut(i) { e.1 = v })); }/>
                                <button class="mini danger" title="Remove" on:click=move |_| edit(Box::new(move |m| { if i < m.metadata.len() { m.metadata.remove(i); } }))><Icon name="x"/></button>
                            </li>
                        }
                    }).collect_view()}
                </ul>
                <div class="actions">
                    <button class="button" on:click=move |_| edit(Box::new(|m| m.metadata.push((if m.metadata.is_empty() { "version".into() } else { String::new() }, String::new()))))><Icon name="plus"/>"metadata"</button>
                </div>
            </div>
        }
        .into_any()
    }
}

/// What the module accepts of each attribute it tests, and the modules it reads.
#[component]
fn Accepts(id: u64) -> impl IntoView {
    let ctx = ctx();
    let name = Memo::new(move |_| {
        ctx.doc
            .with(|d| d.module(id).map(|m| m.name.clone()).unwrap_or_default())
    });
    let info = Memo::new(move |_| {
        let n = name.get();
        ctx.report.with(|r| {
            let r = r.as_ref().ok()?;
            let m = r.modules.iter().find(|m| m.name == n)?;
            let attrs: Vec<String> = {
                let mut v: Vec<String> = vec![];
                for l in m.links.iter().filter(|l| !l.module) {
                    if !v.contains(&l.target) {
                        v.push(l.target.clone());
                    }
                }
                v
            };
            let reads: Vec<(String, bool)> = m
                .links
                .iter()
                .filter(|l| l.module)
                .map(|l| (l.target.clone(), l.include))
                .collect();
            let used_by: Vec<String> = r
                .modules
                .iter()
                .filter(|o| {
                    o.links
                        .iter()
                        .any(|l| l.module && (l.target == n || m.labels.contains(&l.target)))
                })
                .map(|o| o.name.clone())
                .collect();
            Some((attrs, reads, used_by))
        })
    });
    move || {
        let Some((attrs, reads, used_by)) = info.get() else {
            return view! {
                <div class="card preview"><p class="hint">"What this module accepts appears once openodd reads the document."</p></div>
            }
            .into_any();
        };
        view! {
            <div class="card preview">
                <div class="card-title"><Icon name="map"/>"What this module accepts"<span class="hint">"of each attribute it tests, the others aside"</span></div>
                <Legend/>
                {(!used_by.is_empty() || !reads.is_empty()).then(|| view! {
                    <div class="flow">
                        {used_by.into_iter().map(|u| {
                            let u2 = u.clone();
                            view! { <button class="pill" on:click=move |_| ctx.select_module(&u2)><Icon name="module"/>{u}</button> }
                        }).collect_view()}
                        <span class="flow-arrow">"→"</span>
                        <span class="pill this"><Icon name="module"/>{name.get()}</span>
                        {(!reads.is_empty()).then(|| view! { <span class="flow-arrow">"→"</span> })}
                        {reads.into_iter().map(|(t, inc)| {
                            let t2 = t.clone();
                            view! { <button class=if inc { "pill include" } else { "pill exclude" } on:click=move |_| ctx.select_module(&t2)><Icon name=if inc { "check" } else { "ban" }/>{t}</button> }
                        }).collect_view()}
                    </div>
                })}
                {attrs.into_iter().map(|a| {
                    let attr = ctx.attribute(a.clone());
                    let module = Signal::derive(move || name.get());
                    let short = tail(&a).to_string();
                    let a4 = a.clone();
                    let icon = concept_icon(&short, KindTag::Number);
                    view! {
                        <div class="accept-row">
                            <button class="accept-name" on:click=move |_| ctx.select_attribute(&a4)>
                                <Icon name=icon/>{short}
                            </button>
                            <div class="accept-viz"><Domain attr=attr module=module/></div>
                        </div>
                    }
                }).collect_view()}
            </div>
        }
        .into_any()
    }
}

/// A toggle switch.
#[component]
fn Switch(#[prop(into)] on: Signal<bool>, set: Callback<bool>) -> impl IntoView {
    view! {
        <button
            class="switch"
            class:on=move || on.get()
            role="switch"
            aria-checked=move || on.get().to_string()
            on:click=move |_| set.run(!on.get_untracked())
        >
            <span class="knob"></span>
        </button>
    }
}

#[component]
fn SectionForm(id: u64, include: bool) -> impl IntoView {
    let ctx = ctx();
    let word = if include { "Include" } else { "Exclude" };
    let state = Memo::new(move |_| {
        ctx.doc.with(|d| {
            d.module(id)
                .and_then(|m| m.section(include).as_ref().map(|s| s.all))
        })
    });
    let edit = move |f: Edit<Option<Section>>| {
        ctx.doc.update(|d| {
            if let Some(m) = d.module_mut(id) {
                f(m.section_mut(include))
            }
        })
    };
    let has_nested = Memo::new(move |_| {
        ctx.doc.with(|d| {
            d.module(id)
                .and_then(|m| m.section(include).as_ref())
                .is_some_and(|s| !s.nested.is_empty())
        })
    });
    view! {
        <div class=if include { "card section include" } else { "card section exclude" }>
            <div class="card-head">
                <div class="card-title">
                    <Icon name=if include { "check" } else { "ban" }/>
                    {word}
                    <span class="hint">{move || match (include, state.get()) {
                        (_, None) => "no section".to_string(),
                        (true, Some(true)) => "inside when ALL of these hold".into(),
                        (true, Some(false)) => "inside when ANY of these holds".into(),
                        (false, Some(true)) => "outside when ALL of these hold".into(),
                        (false, Some(false)) => "outside when ANY of these holds".into(),
                    }}</span>
                </div>
                <div class="segmented">
                    {[(None, "off"), (Some(true), "all (AND)"), (Some(false), "any (OR)")].into_iter().map(|(v, label)| view! {
                        <button
                            class:active=move || state.get() == v
                            on:click=move |_| edit(Box::new(move |s| match v {
                                None => *s = None,
                                Some(all) => match s {
                                    Some(sec) => sec.all = all,
                                    None => *s = Some(Section::new(all)),
                                },
                            }))
                        >{label}</button>
                    }).collect_view()}
                </div>
            </div>
            {move || state.get().map(|all| {
                let (op, other) = if all { ("AND", "OR") } else { ("OR", "AND") };
                view! {
                    <CondList at=CondsAt::Section { module: id, include, nested: false }/>
                    {move || if has_nested.get() {
                        view! {
                            <div class="nested">
                                <div class="nested-head">
                                    <span class="op">{other}</span>
                                    <span class="hint">{format!("a nested {} group inside the {op} section: {}", other, if other == "OR" { "any of these" } else { "all of these" })}</span>
                                </div>
                                <CondList at=CondsAt::Section { module: id, include, nested: true }/>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <button class="button subtle" on:click=move |_| {
                                ctx.doc.update(|d| if let Some(s) = d.module_mut(id).and_then(|m| m.section_mut(include).as_mut()) {
                                    s.nested.push(Cond { id: fresh_id(), key: String::new(), value: CondValue::Literals(vec![]) });
                                });
                            }><Icon name="plus"/>{format!("nested {other} group")}</button>
                        }.into_any()
                    }}
                }
            })}
        </div>
    }
}

// ---------------------------------------------------------------------------
// The lines of a section
// ---------------------------------------------------------------------------

#[component]
pub fn CondList(at: CondsAt) -> impl IntoView {
    let ctx = ctx();
    let ids = Memo::new(move |_| {
        ctx.doc.with(|d| {
            d.conds(at)
                .map(|cs| cs.iter().map(|c| c.id).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    let options = Memo::new(move |_| ctx.doc.with(key_options));
    let list_id = format!("dl-keys-{}", fresh_id());
    let add = move |_| {
        ctx.doc.update(|d| {
            if let Some(cs) = d.conds_mut(at) {
                cs.push(Cond {
                    id: fresh_id(),
                    key: String::new(),
                    value: CondValue::Literals(vec![]),
                });
            }
        })
    };
    let l2 = list_id.clone();
    view! {
        <div class="conds">
            <datalist id=list_id.clone()>
                {move || options.get().into_iter().map(|(k, what)| view! { <option value=k.clone()>{what}</option> }).collect_view()}
            </datalist>
            <For each=move || ids.get() key=|x| *x let:cid>
                <CondRow at=at id=cid list=l2.clone()/>
            </For>
            {move || ids.with(Vec::is_empty).then(|| view! { <p class="hint empty">"No conditions."</p> })}
            <button class="button subtle" on:click=add><Icon name="plus"/>"condition"</button>
        </div>
    }
}

fn edit_cond(at: CondsAt, id: u64, f: impl FnOnce(&mut Cond)) {
    ctx().doc.update(|d| {
        if let Some(c) = d
            .conds_mut(at)
            .and_then(|cs| cs.iter_mut().find(|c| c.id == id))
        {
            f(c)
        }
    });
}

/// Whether a value says nothing yet, so a new key may choose its form.
fn blank(v: &CondValue) -> bool {
    match v {
        CondValue::Literals(l) => l.is_empty(),
        CondValue::Compare { operand, .. } => operand.trim().is_empty(),
        CondValue::Range { low, high, .. } => low.trim().is_empty() && high.trim().is_empty(),
        CondValue::Raw(t) => t.trim().is_empty(),
        CondValue::Bool(_) => false,
    }
}

/// The form a value takes for a key of this kind, by default.
fn default_tag(kind: &KeyKind) -> ValueTag {
    match kind {
        KeyKind::Module | KeyKind::Label => ValueTag::Bool,
        KeyKind::Concept(Kind::Boolean) => ValueTag::Bool,
        KeyKind::Concept(Kind::Number { .. }) => ValueTag::Compare,
        _ => ValueTag::Literals,
    }
}

/// The literals a key's concept takes (empty if it is not a categorical).
fn literals_of(d: &Document, key: &KeyKind) -> Vec<String> {
    match key {
        KeyKind::Concept(Kind::Categorical(l)) => l.clone(),
        KeyKind::Concept(Kind::Derived(defs)) => defs.iter().map(|d| d.literal.clone()).collect(),
        KeyKind::Concept(Kind::Reference(r)) => match d.key_kind(r) {
            KeyKind::Concept(Kind::Categorical(l)) => l.clone(),
            KeyKind::Concept(Kind::Derived(defs)) => {
                defs.iter().map(|d| d.literal.clone()).collect()
            }
            _ => vec![],
        },
        _ => vec![],
    }
}

fn key_icon(key: &str, kind: &KeyKind) -> &'static str {
    match kind {
        KeyKind::Module => "module",
        KeyKind::Label => "tag",
        KeyKind::Concept(k) => concept_icon(tail(key), k.tag()),
        KeyKind::Unknown => "warn",
    }
}

/// A number and its unit, as a condition writes them (`60 km/h`, `60km/h`).
fn split_quantity(text: &str) -> Option<(f64, String)> {
    let c = openodd::loader::QUANTITY_RE.captures(text.trim())?;
    let n = openodd::value::parse_float(&c["value"])?;
    Some((n, c.name("unit").map_or("", |u| u.as_str()).to_string()))
}

/// The interval a numeric condition allows.
fn interval_of(v: &CondValue, unit_ok: impl Fn(&str) -> bool) -> Option<(f64, f64)> {
    match v {
        CondValue::Compare { op, operand } => {
            let (n, u) = split_quantity(operand)?;
            if !unit_ok(&u) {
                return None;
            }
            Some(if op.starts_with('<') {
                (f64::NEG_INFINITY, n)
            } else {
                (n, f64::INFINITY)
            })
        }
        CondValue::Range { low, high, unit } => {
            if !unit_ok(unit) {
                return None;
            }
            Some((low.trim().parse().ok()?, high.trim().parse().ok()?))
        }
        _ => None,
    }
}

#[component]
fn CondRow(at: CondsAt, id: u64, list: String) -> impl IntoView {
    let ctx = ctx();
    let cond = Memo::new(move |_| {
        ctx.doc.with(|d| {
            d.conds(at)
                .and_then(|cs| cs.iter().find(|c| c.id == id).cloned())
        })
    });
    let key =
        Memo::new(move |_| cond.with(|c| c.as_ref().map(|c| c.key.clone()).unwrap_or_default()));
    let tag = Memo::new(move |_| cond.with(|c| c.as_ref().map(|c| c.value.tag())));
    let key_kind = Memo::new(move |_| ctx.doc.with(|d| d.key_kind(&key.get())));
    let literals = Memo::new(move |_| ctx.doc.with(|d| literals_of(d, &key_kind.get())));
    let attr = Memo::new(move |_| {
        let k = key.get();
        ctx.report
            .with(|r| r.as_ref().ok().and_then(|r| r.attribute(&k).cloned()))
    });
    let set_key = move |ev: leptos::ev::Event| {
        let k = event_target_value(&ev);
        let kind = ctx.doc.with(|d| d.key_kind(&k));
        edit_cond(at, id, move |c| {
            c.key = k;
            if blank(&c.value) || matches!(kind, KeyKind::Module | KeyKind::Label) {
                let t = default_tag(&kind);
                if c.value.tag() != t {
                    c.value = CondValue::empty(t);
                }
            }
        });
    };
    let set_tag = move |ev: leptos::ev::Event| {
        if let Some(t) = ValueTag::from_label(&event_target_value(&ev)) {
            edit_cond(at, id, move |c| {
                // Keep what the expression says, as YAML, when it changes form.
                c.value = match (t, &c.value) {
                    (ValueTag::Raw, v) => CondValue::Raw(v.summary()),
                    (t, CondValue::Raw(text)) => {
                        let v = crate::doc::read_value(&crate::doc::yaml_of(text));
                        if v.tag() == t {
                            v
                        } else {
                            CondValue::empty(t)
                        }
                    }
                    (t, _) => CondValue::empty(t),
                };
            });
        }
    };
    let remove = move |_| {
        ctx.doc.update(|d| {
            if let Some(cs) = d.conds_mut(at) {
                cs.retain(|c| c.id != id)
            }
        })
    };
    let icon = move || {
        let k = key.get();
        let kind = key_kind.get();
        let name = key_icon(&k, &kind);
        let bad = matches!(kind, KeyKind::Unknown) && !k.is_empty();
        view! {
            <span class="key-icon" class:bad=bad title=if bad { "No concept, module or label has this name" } else { "" }>
                <Icon name=if k.is_empty() { "dots" } else { name }/>
            </span>
        }
    };
    // A number line under a numeric condition, its ends draggable.
    // (Only in the attribute's own unit: the line is drawn in it.)
    let interval = Memo::new(move |_| {
        let au = attr.with(|a| a.as_ref().map(|a| a.unit.clone()).unwrap_or_default());
        let unit_ok =
            |u: &str| u.is_empty() || au.is_empty() || openodd::units::normalize_unit(u) == au;
        cond.with(|c| c.as_ref().and_then(|c| interval_of(&c.value, unit_ok)))
    });
    let handles =
        Signal::derive(move || interval.get().map(|(a, b)| vec![a, b]).unwrap_or_default());
    let drag = Callback::new(move |(i, v): (usize, f64)| {
        let text = fmt_num(v);
        edit_cond(at, id, move |c| match &mut c.value {
            CondValue::Compare { operand, .. } => {
                let unit = split_quantity(operand).map(|(_, u)| u).unwrap_or_default();
                *operand = if unit.is_empty() {
                    text
                } else {
                    format!("{text} {unit}")
                };
            }
            CondValue::Range { low, high, .. } => {
                if i == 0 {
                    *low = text;
                } else {
                    *high = text;
                }
            }
            _ => {}
        })
    });
    let line_attr = Signal::derive(move || attr.get());
    // Its own memo, so the line is not rebuilt (and a drag not dropped) as it moves.
    let show_line = Memo::new(move |_| {
        interval.get().is_some() && attr.with(|a| a.as_ref().is_some_and(|a| a.is_number()))
    });
    view! {
        <div class="cond-block">
            <div class="cond">
                <div class="key-cell">
                    {icon}
                    <input
                        class="key"
                        placeholder="concept / module / label"
                        list=list
                        prop:value=move || key.get()
                        on:change=set_key
                    />
                </div>
                <select class="value-form" prop:value=move || tag.get().map(|t| t.label()).unwrap_or_default() on:change=set_tag>
                    {ValueTag::ALL.into_iter().map(|t| view! { <option value=t.label()>{t.label()}</option> }).collect_view()}
                </select>
                <div class="value">
                    {move || match tag.get() {
                        Some(ValueTag::Bool) => view! { <BoolValue at=at id=id/> }.into_any(),
                        Some(ValueTag::Literals) => view! { <LiteralsValue at=at id=id literals=literals attr=attr/> }.into_any(),
                        Some(ValueTag::Compare) => view! { <CompareValue at=at id=id literals=literals/> }.into_any(),
                        Some(ValueTag::Range) => view! { <RangeValue at=at id=id/> }.into_any(),
                        Some(ValueTag::Raw) => view! { <RawValue at=at id=id/> }.into_any(),
                        None => ().into_any(),
                    }}
                </div>
                <button class="mini danger" title="Remove" on:click=remove><Icon name="x"/></button>
            </div>
            {move || show_line.get().then(|| view! {
                <div class="cond-line">
                    <NumberLine attr=line_attr interval=Signal::from(interval) handles=Some(handles) on_drag=Some(drag)/>
                </div>
            })}
        </div>
    }
}

fn read_value(at: CondsAt, id: u64) -> Option<CondValue> {
    ctx().doc.with(|d| {
        d.conds(at)
            .and_then(|cs| cs.iter().find(|c| c.id == id))
            .map(|c| c.value.clone())
    })
}

#[component]
fn BoolValue(at: CondsAt, id: u64) -> impl IntoView {
    let on = Signal::derive(move || !matches!(read_value(at, id), Some(CondValue::Bool(false))));
    view! {
        <div class="switch-line">
            <Switch on=on set=Callback::new(move |b: bool| edit_cond(at, id, move |c| c.value = CondValue::Bool(b)))/>
            <span class="mono">{move || if on.get() { "true" } else { "false" }}</span>
        </div>
    }
}

#[component]
fn LiteralsValue(
    at: CondsAt,
    id: u64,
    literals: Memo<Vec<String>>,
    attr: Memo<Option<Attribute>>,
) -> impl IntoView {
    let chosen = move || match read_value(at, id) {
        Some(CondValue::Literals(l)) => l,
        _ => vec![],
    };
    let toggle = move |lit: String| {
        edit_cond(at, id, move |c| {
            if let CondValue::Literals(l) = &mut c.value {
                if let Some(i) = l.iter().position(|x| *x == lit) {
                    l.remove(i);
                } else {
                    l.push(lit);
                }
            }
        })
    };
    let state_of = move |lit: &str| {
        attr.with(|a| {
            a.as_ref().and_then(|a| {
                a.values
                    .iter()
                    .position(|v| v == lit)
                    .and_then(|i| a.states.get(i).copied())
            })
        })
    };
    move || {
        let lits = literals.get();
        if lits.is_empty() {
            view! {
                <input
                    placeholder="literal, literal…"
                    prop:value=move || chosen().join(", ")
                    on:change=move |ev| {
                        let l = split_list(&event_target_value(&ev));
                        edit_cond(at, id, move |c| c.value = CondValue::Literals(l));
                    }
                />
            }
            .into_any()
        } else {
            view! {
                <div class="chips">
                    {lits.into_iter().map(|lit| {
                        let l2 = lit.clone();
                        let l3 = lit.clone();
                        let l4 = lit.clone();
                        view! {
                            <button
                                class="chip"
                                class:on=move || chosen().contains(&l2)
                                on:click=move |_| toggle(l3.clone())
                            >
                                <span class=move || format!("state-dot {}", state_of(&l4).map(State::class).unwrap_or(""))></span>
                                {lit}
                            </button>
                        }
                    }).collect_view()}
                    <button
                        class="chip"
                        class:on=move || chosen().iter().any(|x| x == "unknown")
                        title="OpenODD's missing value"
                        on:click=move |_| toggle("unknown".into())
                    >"unknown"</button>
                </div>
            }
            .into_any()
        }
    }
}

#[component]
fn CompareValue(at: CondsAt, id: u64, literals: Memo<Vec<String>>) -> impl IntoView {
    let parts = move || match read_value(at, id) {
        Some(CondValue::Compare { op, operand }) => (op, operand),
        _ => ("<=".into(), String::new()),
    };
    let list_id = format!("dl-lit-{id}");
    view! {
        <div class="inline">
            <div class="ops">
                {OPERATORS.into_iter().map(|o| view! {
                    <button
                        class:active=move || parts().0 == o
                        on:click=move |_| edit_cond(at, id, move |c| if let CondValue::Compare { op, .. } = &mut c.value { *op = o.into() })
                    >{o}</button>
                }).collect_view()}
            </div>
            <input
                placeholder="60 km/h, or a literal"
                list=list_id.clone()
                prop:value=move || parts().1
                on:input=move |ev| {
                    let v = event_target_value(&ev);
                    edit_cond(at, id, move |c| if let CondValue::Compare { operand, .. } = &mut c.value { *operand = v });
                }
            />
            <datalist id=list_id>
                {move || literals.get().into_iter().map(|l| view! { <option value=l/> }).collect_view()}
            </datalist>
        </div>
    }
}

#[component]
fn RangeValue(at: CondsAt, id: u64) -> impl IntoView {
    let part = move |i: usize| match read_value(at, id) {
        Some(CondValue::Range { low, high, unit }) => [low, high, unit][i].clone(),
        _ => String::new(),
    };
    let set = move |i: usize, v: String| {
        edit_cond(at, id, move |c| {
            if let CondValue::Range { low, high, unit } = &mut c.value {
                *[low, high, unit][i] = v;
            }
        })
    };
    view! {
        <div class="inline">
            "["
            <input class="num" placeholder="low" prop:value=move || part(0) on:input=move |ev| set(0, event_target_value(&ev))/>
            ".."
            <input class="num" placeholder="high" prop:value=move || part(1) on:input=move |ev| set(1, event_target_value(&ev))/>
            "]"
            <input class="unit" placeholder="unit" prop:value=move || part(2) on:input=move |ev| set(2, event_target_value(&ev))/>
        </div>
    }
}

#[component]
fn RawValue(at: CondsAt, id: u64) -> impl IntoView {
    view! {
        <input
            class="mono"
            placeholder="YAML value"
            prop:value=move || match read_value(at, id) { Some(CondValue::Raw(t)) => t, _ => String::new() }
            on:input=move |ev| {
                let v = event_target_value(&ev);
                edit_cond(at, id, move |c| c.value = CondValue::Raw(v));
            }
        />
    }
}
