//! The forms: the document, a concept, a module, and the lines of a section.
//!
//! Every form reads and writes the one document signal. Lists are keyed by
//! their rows' ids and re-rendered only when rows come or go (a `Memo` of the
//! ids), so typing into a row keeps its focus.

use leptos::prelude::*;
use leptos::task::spawn_local;
use web_sys::HtmlInputElement;

use crate::app::{add_concept, ctx, event_target, kind_icon, Sel};
use crate::doc::{
    fresh_id, move_in, Cond, CondValue, CondsAt, Definition, Document, KeyKind, Kind, KindTag,
    Section, ValueTag, OPERATORS, PRIMITIVES, UNIT_TYPES,
};

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

/// Rename references in every section when a concept or module is renamed.
fn rename_keys(d: &mut Document, renames: &[(String, String)]) {
    fn walk(conds: &mut [Cond], renames: &[(String, String)]) {
        for c in conds {
            if let Some((_, new)) = renames.iter().find(|(old, _)| *old == c.key) {
                c.key = new.clone();
            }
        }
    }
    for m in &mut d.modules {
        for s in [&mut m.include, &mut m.exclude].into_iter().flatten() {
            walk(&mut s.conditions, renames);
            walk(&mut s.nested, renames);
        }
    }
    fn defs(cs: &mut [crate::doc::Concept], renames: &[(String, String)]) {
        for c in cs {
            match &mut c.kind {
                Kind::Group(children) => defs(children, renames),
                Kind::Derived(ds) => {
                    for d in ds {
                        walk(&mut d.conditions, renames);
                    }
                }
                _ => {}
            }
        }
    }
    defs(&mut d.taxonomy, renames);
}

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

#[component]
pub fn Overview() -> impl IntoView {
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
            <h2>"Document"</h2>
            <p class="hint">
                "An ASAM OpenODD 1.0 document: the concepts of the "
                <b>"TAXONOMY"</b>", the "<b>"MODULES"</b>" that bound them, and the "<b>"ODD"</b>
                " modules that are its roots. Pick an item on the left to edit it; the YAML and openodd's check of it are on the right."
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
            <h3>"IMPORT"</h3>
            <p class="hint">"Other OpenODD files this one reads. Add them below so the check can read them too."</p>
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
                        <button class="mini danger" title="Remove" on:click=move |_| ctx.doc.update(|d| { if i < d.imports.len() { d.imports.remove(i); } })>"✕"</button>
                    </li>
                }).collect_view()}
            </ul>
            <div class="actions">
                <button class="button" on:click=move |_| ctx.doc.update(|d| d.imports.push("taxonomy.yaml".into()))>"+ import"</button>
                <label class="button">
                    "Load imported files…"
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
                                    <span class="mono">{name.clone()}</span>
                                    <span class="hint">{format!("{} lines", text.lines().count())}</span>
                                    <button class="mini" title="Edit this file instead" on:click=move |_| { ctx.open(&name, &text); }>"open"</button>
                                    <button class="mini danger" title="Remove" on:click=move |_| ctx.library.update(|l| l.retain(|(x, _)| *x != n))>"✕"</button>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                })
            }}
        </div>
        <div class="card">
            <h3>"Other sections"</h3>
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
            rename_keys(d, &[(old_path, new_path), (old, new)]);
        });
    };
    let set_kind = move |ev: leptos::ev::Event| {
        if let Some(t) = KindTag::from_label(&event_target_value(&ev)) {
            ctx.doc.update(|d| {
                if let Some(c) = d.concept_mut(id) {
                    c.kind = c.kind.convert(t);
                }
            });
        }
    };
    let remove = move |_| {
        let parent = ctx.doc.with(|d| d.parent_of(id)).flatten();
        ctx.doc.update(|d| d.remove_concept(id));
        ctx.sel
            .set(parent.map(Sel::Concept).unwrap_or(Sel::Overview));
    };
    move || {
        if !exists.get() {
            return view! { <div class="card"><p class="hint">"This concept is gone."</p></div> }
                .into_any();
        }
        view! {
            <div class="card">
                <div class="card-head">
                    <h2>
                        <span class="icon">{move || tag.get().map(kind_icon)}</span>
                        " "{move || path.get()}
                    </h2>
                    <div class="actions">
                        <button class="mini" title="Move up" on:click=move |_| ctx.doc.update(|d| d.move_concept(id, -1))>"↑"</button>
                        <button class="mini" title="Move down" on:click=move |_| ctx.doc.update(|d| d.move_concept(id, 1))>"↓"</button>
                        <button class="mini danger" on:click=remove>"Delete"</button>
                    </div>
                </div>
                <div class="grid2">
                    <label class="field">
                        <span>"Name"</span>
                        <input prop:value=name on:change=rename/>
                    </label>
                    <label class="field">
                        <span>"Kind"</span>
                        <select on:change=set_kind prop:value=move || tag.get().map(|t| t.label()).unwrap_or_default()>
                            {KindTag::ALL.into_iter().map(|t| view! { <option value=t.label()>{t.label()}</option> }).collect_view()}
                        </select>
                    </label>
                </div>
                {move || match tag.get() {
                    Some(KindTag::Number) => view! { <NumberForm id=id/> }.into_any(),
                    Some(KindTag::Categorical) => view! { <LiteralsForm id=id/> }.into_any(),
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
            <label class="field">
                <span>"Primitive"</span>
                <select
                    prop:value=move || read(true)
                    on:change=move |ev| {
                        let v = event_target_value(&ev);
                        edit_kind(id, |k| if let Kind::Number { primitive, .. } = k { *primitive = v });
                    }
                >
                    {PRIMITIVES.into_iter().map(|p| view! { <option value=p>{p}</option> }).collect_view()}
                </select>
            </label>
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
        <p class="hint">"Conditions on it name a unit of this type (e.g. "<code>"<= 60 km/h"</code>"); openodd converts between units."</p>
    }
}

#[component]
fn LiteralsForm(id: u64) -> impl IntoView {
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
    view! {
        <h3>"Literals"</h3>
        <p class="hint">"In order: a comparison like "<code>">= heavy"</code>" reads this order."</p>
        <ul class="list">
            {move || (0..count.get()).map(|i| view! {
                <li class="line">
                    <span class="index">{i + 1}</span>
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
                    <button class="mini" title="Move up" on:click=move |_| edit(Box::new(move |l| if i > 0 { l.swap(i, i - 1) }))>"↑"</button>
                    <button class="mini" title="Move down" on:click=move |_| edit(Box::new(move |l| if i + 1 < l.len() { l.swap(i, i + 1) }))>"↓"</button>
                    <button class="mini danger" title="Remove" on:click=move |_| edit(Box::new(move |l| { if i < l.len() { l.remove(i); } }))>"✕"</button>
                </li>
            }).collect_view()}
        </ul>
        <div class="actions">
            <button class="button" on:click=move |_| edit(Box::new(|l| {
                let taken = l.clone();
                l.push(crate::app::unique("value", &taken));
            }))>"+ literal"</button>
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
        <p class="hint">"The first literal whose conditions all hold is the value, e.g. "<code>"light: {rainfall_rate: \"[0.1 .. 2.5] mm/h\"}"</code>"."</p>
        <For each=move || ids.get() key=|x| *x let:lit>
            <div class="definition">
                <div class="line">
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
                    <button class="mini" title="Move up" on:click=move |_| edit(Box::new(move |defs| move_in(defs, |x| x.id == lit, -1)))>"↑"</button>
                    <button class="mini" title="Move down" on:click=move |_| edit(Box::new(move |defs| move_in(defs, |x| x.id == lit, 1)))>"↓"</button>
                    <button class="mini danger" title="Remove" on:click=move |_| edit(Box::new(move |defs| defs.retain(|x| x.id != lit)))>"✕"</button>
                </div>
                <CondList at=CondsAt::Definition { concept: id, literal: lit }/>
            </div>
        </For>
        <div class="actions">
            <button class="button" on:click=move |_| edit(Box::new(|defs| {
                let taken: Vec<String> = defs.iter().map(|d| d.literal.clone()).collect();
                defs.push(Definition { id: fresh_id(), literal: crate::app::unique("value", &taken), conditions: vec![] });
            }))>"+ literal"</button>
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
        <ul class="list">
            {move || children.get().into_iter().map(|(cid, name, tag)| view! {
                <li class="line link" on:click=move |_| ctx.sel.set(Sel::Concept(cid))>
                    <span class="icon">{kind_icon(tag)}</span>
                    <span>{name}</span>
                    <span class="hint">{tag.label()}</span>
                </li>
            }).collect_view()}
        </ul>
        <div class="actions">
            <button class="button" on:click=move |_| add_concept(Some(id), KindTag::Categorical)>"+ concept"</button>
            <button class="button" on:click=move |_| add_concept(Some(id), KindTag::Group)>"+ group"</button>
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
                .map(|(n, _)| n.rsplit('.').next().unwrap_or(&n).to_string())
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
            rename_keys(d, &[(old, new)]);
        });
    };
    let root = Memo::new(move |_| ctx.doc.with(|d| d.module(id).is_some_and(|m| m.root)));
    let meta_count = Memo::new(move |_| {
        ctx.doc
            .with(|d| d.module(id).map_or(0, |m| m.metadata.len()))
    });
    let remove = move |_| {
        ctx.doc.update(|d| d.modules.retain(|m| m.id != id));
        ctx.sel.set(Sel::Overview);
    };
    move || {
        if !exists.get() {
            return view! { <div class="card"><p class="hint">"This module is gone."</p></div> }
                .into_any();
        }
        view! {
            <div class="card">
                <div class="card-head">
                    <h2>{move || if root.get() { "◆ " } else { "◇ " }}{move || read(|m| m.name.clone())}</h2>
                    <div class="actions">
                        <button class="mini" title="Move up" on:click=move |_| ctx.doc.update(|d| move_in(&mut d.modules, |m| m.id == id, -1))>"↑"</button>
                        <button class="mini" title="Move down" on:click=move |_| ctx.doc.update(|d| move_in(&mut d.modules, |m| m.id == id, 1))>"↓"</button>
                        <button class="mini danger" on:click=remove>"Delete"</button>
                    </div>
                </div>
                <div class="grid2">
                    <label class="field">
                        <span>"Id"</span>
                        <input prop:value=move || read(|m| m.name.clone()) on:change=rename/>
                    </label>
                    <label class="field">
                        <span>"Section"</span>
                        <select
                            prop:value=move || if root.get() { "ODD" } else { "MODULES" }
                            on:change=move |ev| {
                                let r = event_target_value(&ev) == "ODD";
                                edit(Box::new(move |m| m.root = r));
                            }
                        >
                            <option value="ODD">"ODD (a root)"</option>
                            <option value="MODULES">"MODULES"</option>
                        </select>
                    </label>
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
                <label class="check">
                    <input
                        type="checkbox"
                        prop:checked=move || ctx.doc.with(|d| d.module(id).is_some_and(|m| m.active))
                        on:change=move |ev| {
                            let on = event_target::<HtmlInputElement>(&ev).checked();
                            edit(Box::new(move |m| m.active = on));
                        }
                    />
                    " Active (an inactive module is left out of the ODD)"
                </label>
                <h3>"METADATA"</h3>
                <ul class="list">
                    {move || (0..meta_count.get()).map(|i| {
                        let get = move |key: bool| ctx.doc.with(|d| d.module(id).and_then(|m| m.metadata.get(i)).map(|(k, v)| if key { k.clone() } else { v.clone() }).unwrap_or_default());
                        view! {
                            <li class="line">
                                <input class="key" placeholder="key" prop:value=move || get(true)
                                    on:input=move |ev| { let v = event_target_value(&ev); edit(Box::new(move |m| if let Some(e) = m.metadata.get_mut(i) { e.0 = v })); }/>
                                <input placeholder="value" prop:value=move || get(false)
                                    on:input=move |ev| { let v = event_target_value(&ev); edit(Box::new(move |m| if let Some(e) = m.metadata.get_mut(i) { e.1 = v })); }/>
                                <button class="mini danger" title="Remove" on:click=move |_| edit(Box::new(move |m| { if i < m.metadata.len() { m.metadata.remove(i); } }))>"✕"</button>
                            </li>
                        }
                    }).collect_view()}
                </ul>
                <div class="actions">
                    <button class="button" on:click=move |_| edit(Box::new(|m| m.metadata.push((if m.metadata.is_empty() { "version".into() } else { String::new() }, String::new()))))>"+ metadata"</button>
                </div>
            </div>
            <SectionForm id=id include=true/>
            <SectionForm id=id include=false/>
        }
        .into_any()
    }
}

#[component]
fn SectionForm(id: u64, include: bool) -> impl IntoView {
    let ctx = ctx();
    let word = if include { "INCLUDE" } else { "EXCLUDE" };
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
                <h3>{word}</h3>
                <div class="segmented">
                    {[(None, "none"), (Some(true), "AND · all"), (Some(false), "OR · any")].into_iter().map(|(v, label)| view! {
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
                    <p class="hint">{if include {
                        if all { "Inside when all of these hold." } else { "Inside when any of these holds." }
                    } else if all { "Outside when all of these hold." } else { "Outside when any of these holds." }}</p>
                    <CondList at=CondsAt::Section { module: id, include, nested: false }/>
                    {move || if has_nested.get() {
                        view! {
                            <div class="nested">
                                <div class="nested-head">
                                    <span class="op">{other}</span>
                                    <span class="hint">{format!("one nested {other} line inside the {op} section")}</span>
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
                            }>{format!("+ nested {other} group")}</button>
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
            <button class="button subtle" on:click=add>"+ condition"</button>
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

#[component]
fn CondRow(at: CondsAt, id: u64, list: String) -> impl IntoView {
    let ctx = ctx();
    let cond = move || {
        ctx.doc.with(|d| {
            d.conds(at)
                .and_then(|cs| cs.iter().find(|c| c.id == id).cloned())
        })
    };
    let key = Memo::new(move |_| cond().map(|c| c.key).unwrap_or_default());
    let tag = Memo::new(move |_| cond().map(|c| c.value.tag()));
    let key_kind = Memo::new(move |_| ctx.doc.with(|d| d.key_kind(&key.get())));
    let literals = Memo::new(move |_| ctx.doc.with(|d| literals_of(d, &key_kind.get())));
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
    let warn = move || match key_kind.get() {
        KeyKind::Unknown if !key.get().is_empty() => Some(
            view! { <span class="tag bad" title="No concept, module or label has this name">"?"</span> },
        ),
        _ => None,
    };
    view! {
        <div class="cond">
            <div class="key-cell">
                <input
                    class="key"
                    placeholder="concept / module / label"
                    list=list
                    prop:value=move || key.get()
                    on:change=set_key
                />
                {warn}
            </div>
            <select class="value-form" prop:value=move || tag.get().map(|t| t.label()).unwrap_or_default() on:change=set_tag>
                {ValueTag::ALL.into_iter().map(|t| view! { <option value=t.label()>{t.label()}</option> }).collect_view()}
            </select>
            <div class="value">
                {move || match tag.get() {
                    Some(ValueTag::Bool) => view! { <BoolValue at=at id=id/> }.into_any(),
                    Some(ValueTag::Literals) => view! { <LiteralsValue at=at id=id literals=literals/> }.into_any(),
                    Some(ValueTag::Compare) => view! { <CompareValue at=at id=id literals=literals/> }.into_any(),
                    Some(ValueTag::Range) => view! { <RangeValue at=at id=id/> }.into_any(),
                    Some(ValueTag::Raw) => view! { <RawValue at=at id=id/> }.into_any(),
                    None => ().into_any(),
                }}
            </div>
            <button class="mini danger" title="Remove" on:click=remove>"✕"</button>
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
    view! {
        <select
            prop:value=move || match read_value(at, id) { Some(CondValue::Bool(false)) => "false", _ => "true" }
            on:change=move |ev| {
                let b = event_target_value(&ev) == "true";
                edit_cond(at, id, move |c| c.value = CondValue::Bool(b));
            }
        >
            <option value="true">"true"</option>
            <option value="false">"false"</option>
        </select>
    }
}

#[component]
fn LiteralsValue(at: CondsAt, id: u64, literals: Memo<Vec<String>>) -> impl IntoView {
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
                        view! {
                            <button
                                class="chip"
                                class:on=move || chosen().contains(&l2)
                                on:click=move |_| toggle(l3.clone())
                            >{lit}</button>
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
            <select
                class="op"
                prop:value=move || parts().0
                on:change=move |ev| {
                    let v = event_target_value(&ev);
                    edit_cond(at, id, move |c| if let CondValue::Compare { op, .. } = &mut c.value { *op = v });
                }
            >
                {OPERATORS.into_iter().map(|o| view! { <option value=o>{o}</option> }).collect_view()}
            </select>
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
