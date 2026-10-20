//! Icons, drawn inline: a concept's by what its name says it is (a road, the
//! rain, a pedestrian), a kind's, and the editor's own.

use leptos::prelude::*;

use crate::doc::KindTag;

/// The SVG body of an icon on a 24 x 24 grid, stroked.
fn body(name: &str) -> &'static str {
    match name {
        "road" => r#"<path d="M8 3 5 21M16 3l3 18M12 4v3M12 10v3M12 16v3"/>"#,
        "lanes" => {
            r#"<path d="M4 3v18M20 3v18M9.3 4v4M9.3 11v3M9.3 17v3M14.7 4v4M14.7 11v3M14.7 17v3"/>"#
        }
        "junction" => r#"<path d="M9 3v6H3M15 3v6h6M9 21v-6H3M15 21v-6h6"/>"#,
        "gauge" => {
            r#"<path d="M4.5 17a8.5 8.5 0 1 1 15 0"/><path d="m12 13 4-5"/><circle cx="12" cy="13" r="1.2"/>"#
        }
        "sign" => {
            r#"<circle cx="12" cy="12" r="8.5"/><path d="M9.5 9.5h3.2a1.6 1.6 0 0 1 0 3.2h-1.4a1.6 1.6 0 0 0 0 3.2H14.5"/>"#
        }
        "sun" => {
            r#"<circle cx="12" cy="12" r="3.8"/><path d="M12 2.5v2.2M12 19.3v2.2M2.5 12h2.2M19.3 12h2.2M5.3 5.3l1.6 1.6M17.1 17.1l1.6 1.6M5.3 18.7l1.6-1.6M17.1 6.9l1.6-1.6"/>"#
        }
        "moon" => r#"<path d="M19 14.5A7.5 7.5 0 1 1 9.5 5a6 6 0 0 0 9.5 9.5Z"/>"#,
        "rain" => {
            r#"<path d="M7 15a4.5 4.5 0 1 1 1.2-8.8A5.5 5.5 0 0 1 18.5 9 3.5 3.5 0 0 1 17.5 15"/><path d="m9 17-1 3M13 17l-1 3M17 17l-1 3"/>"#
        }
        "fog" => {
            r#"<path d="M7 12a4.5 4.5 0 1 1 1.2-8.8A5.5 5.5 0 0 1 18.5 6"/><path d="M4 15h16M6 18.5h12M8 22h8"/>"#
        }
        "wind" => r#"<path d="M3 9h11a3 3 0 1 0-3-3M3 15h15a3 3 0 1 1-3 3M3 12h7"/>"#,
        "thermo" => r#"<path d="M10 4a2 2 0 1 1 4 0v10.3a4 4 0 1 1-4 0Z"/><path d="M12 9v7"/>"#,
        "walk" => {
            r#"<circle cx="13" cy="4.5" r="1.8"/><path d="m9 21 2.5-6 2.5 2.5V21M8 11l3-3.5 3 2 2.5 2.5M11.5 15 11 8"/>"#
        }
        "car" => {
            r#"<path d="M4 16v-4l2-5h12l2 5v4Z"/><path d="M4 12h16"/><circle cx="7.5" cy="17.5" r="1.8"/><circle cx="16.5" cy="17.5" r="1.8"/>"#
        }
        "cars" => {
            r#"<path d="M2.5 14v-3l1.5-4h8l1.5 4v3Z"/><path d="M10.5 18v-3l1.5-4h8l1.5 4v3Z"/><circle cx="5.5" cy="15" r="1.2"/><circle cx="18.5" cy="19" r="1.2"/>"#
        }
        "ruler" => r#"<path d="M3 16 16 3l5 5L8 21Z"/><path d="m7 12 2 2M10 9l2 2M13 6l2 2"/>"#,
        "clock" => r#"<circle cx="12" cy="12" r="8.5"/><path d="M12 7v5l3 2"/>"#,
        "pin" => {
            r#"<path d="M12 21s-6.5-6-6.5-11a6.5 6.5 0 0 1 13 0c0 5-6.5 11-6.5 11Z"/><circle cx="12" cy="10" r="2.3"/>"#
        }
        "signal" => r#"<path d="M5 19v-3M9.5 19v-6M14 19v-9M18.5 19V6"/>"#,
        "hash" => r#"<path d="M5 9h14M5 15h14M10 4 8 20M16 4l-2 16"/>"#,
        "toggle" => {
            r#"<rect x="2.5" y="7" width="19" height="10" rx="5"/><circle cx="16.5" cy="12" r="2.8"/>"#
        }
        "list" => {
            r#"<path d="M9 6h11M9 12h11M9 18h11"/><circle cx="4.5" cy="6" r="1"/><circle cx="4.5" cy="12" r="1"/><circle cx="4.5" cy="18" r="1"/>"#
        }
        "func" => {
            r#"<path d="M15 4h-1.5a3 3 0 0 0-3 3v10a3 3 0 0 1-3 3H6M7.5 11h7"/><path d="m15 13 5 5M20 13l-5 5"/>"#
        }
        "folder" => {
            r#"<path d="M3 6.5A1.5 1.5 0 0 1 4.5 5h4l2 2h9A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5Z"/>"#
        }
        "link" => {
            r#"<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>"#
        }
        "dots" => {
            r#"<circle cx="5" cy="12" r="1.3"/><circle cx="12" cy="12" r="1.3"/><circle cx="19" cy="12" r="1.3"/>"#
        }
        "module" => {
            r#"<path d="m12 2.8 8 4.6v9.2l-8 4.6-8-4.6V7.4Z"/><path d="m4 7.4 8 4.6 8-4.6M12 12v9.2"/>"#
        }
        "target" => {
            r#"<circle cx="12" cy="12" r="8.5"/><circle cx="12" cy="12" r="4.5"/><circle cx="12" cy="12" r="1"/>"#
        }
        "tag" => r#"<path d="M3 12V4h8l10 10-8 8Z"/><circle cx="7.5" cy="8.5" r="1.3"/>"#,
        "check" => r#"<circle cx="12" cy="12" r="8.5"/><path d="m8 12 3 3 5-6"/>"#,
        "ban" => r#"<circle cx="12" cy="12" r="8.5"/><path d="m6 6 12 12"/>"#,
        "doc" => r#"<path d="M6 2.5h8l4 4v15H6Z"/><path d="M14 2.5v4h4M9 12h6M9 16h6"/>"#,
        "map" => {
            r#"<path d="m3 6 6-2.5 6 2.5 6-2.5v14.5L15 21l-6-2.5L3 21Z"/><path d="M9 3.5v15M15 6v15"/>"#
        }
        "graph" => {
            r#"<circle cx="5" cy="6" r="2.2"/><circle cx="5" cy="18" r="2.2"/><circle cx="19" cy="12" r="2.2"/><path d="M7.2 6.5 16.8 11.5M7.2 17.5l9.6-5"/>"#
        }
        "grid" => {
            r#"<rect x="3.5" y="3.5" width="17" height="17" rx="1.5"/><path d="M3.5 9.2h17M3.5 14.8h17M9.2 3.5v17M14.8 3.5v17"/>"#
        }
        "flask" => {
            r#"<path d="M9 3h6M10 3v6l-5.5 9.5A1.7 1.7 0 0 0 6 21h12a1.7 1.7 0 0 0 1.5-2.5L14 9V3"/><path d="M7.5 15h9"/>"#
        }
        "code" => r#"<path d="m8 7-5 5 5 5M16 7l5 5-5 5M13.5 4l-3 16"/>"#,
        "plus" => r#"<path d="M12 5v14M5 12h14"/>"#,
        "trash" => r#"<path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/>"#,
        "up" => r#"<path d="m6 14 6-6 6 6"/>"#,
        "down" => r#"<path d="m6 10 6 6 6-6"/>"#,
        "x" => r#"<path d="m6 6 12 12M18 6 6 18"/>"#,
        "upload" => r#"<path d="M12 16V4M7 9l5-5 5 5M4 16v4h16v-4"/>"#,
        "download" => r#"<path d="M12 4v12M7 11l5 5 5-5M4 16v4h16v-4"/>"#,
        "sparkle" => {
            r#"<path d="M12 3c.6 4.2 2.8 6.4 7 7-4.2.6-6.4 2.8-7 7-.6-4.2-2.8-6.4-7-7 4.2-.6 6.4-2.8 7-7Z"/>"#
        }
        "warn" => r#"<path d="M12 3.5 2.5 20h19Z"/><path d="M12 10v4.5M12 17.2v.3"/>"#,
        _ => r#"<circle cx="12" cy="12" r="4"/>"#,
    }
}

#[component]
pub fn Icon(name: &'static str, #[prop(optional)] class: &'static str) -> impl IntoView {
    view! {
        <svg
            class=format!("icon-svg {class}")
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.7"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
            inner_html=body(name)
        ></svg>
    }
}

/// The icon a concept's name suggests, else its kind's.
pub fn concept_icon(name: &str, tag: KindTag) -> &'static str {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if tag == KindTag::Group {
        return match () {
            _ if has(&["scenery", "road", "infra"]) => "road",
            _ if has(&["environment", "weather"]) => "rain",
            _ if has(&["dynamic", "traffic", "actor"]) => "cars",
            _ if has(&["connect"]) => "signal",
            _ => "folder",
        };
    }
    match () {
        _ if has(&["junction", "intersection", "crossing_road"]) => "junction",
        _ if has(&["lane"]) => "lanes",
        _ if has(&["speed_limit", "limit"]) => "sign",
        _ if has(&["speed", "velocity"]) => "gauge",
        _ if has(&["night", "dark"]) => "moon",
        _ if has(&["illumination", "light", "sun", "day"]) => "sun",
        _ if has(&["rain", "precip", "snow"]) => "rain",
        _ if has(&["fog", "visib", "mist"]) => "fog",
        _ if has(&["wind"]) => "wind",
        _ if has(&["temp"]) => "thermo",
        _ if has(&["pedestrian", "person", "walker", "cyclist"]) => "walk",
        _ if has(&["traffic", "density"]) => "cars",
        _ if has(&["vehicle", "car", "ego"]) => "car",
        _ if has(&["gap", "distance", "length", "width"]) => "ruler",
        _ if has(&["time", "latency", "duration"]) => "clock",
        _ if has(&["location", "zone", "area", "region"]) => "pin",
        _ if has(&["road", "street", "highway"]) => "road",
        _ if has(&["throughput", "bandwidth", "signal", "connect"]) => "signal",
        _ => kind_icon(tag),
    }
}

pub fn kind_icon(tag: KindTag) -> &'static str {
    match tag {
        KindTag::Group => "folder",
        KindTag::Number => "hash",
        KindTag::Boolean => "toggle",
        KindTag::Categorical => "list",
        KindTag::Derived => "func",
        KindTag::Reference => "link",
        KindTag::Other => "dots",
    }
}
