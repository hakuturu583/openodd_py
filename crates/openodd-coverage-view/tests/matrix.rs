//! The matrix over synthetic coverage documents, and its HTML.

use openodd_coverage_view::data::{Dataset, MISSING};
use openodd_coverage_view::html::{self, Options};
use openodd_coverage_view::matrix::{CellState, Query};
use serde_json::{json, Value as Json};

const ATTRS: [&str; 3] = ["speed", "rain", "light"];

fn items() -> Json {
    json!([
        {"name": "odd.speed", "attribute": "speed", "unit": "km/h", "buckets": ["[0, 30)", "[30, 60]"], "outside_odd": []},
        {"name": "odd.rain", "attribute": "rain", "buckets": ["none", "heavy", "snow"], "outside_odd": ["snow"]},
        {"name": "odd.light", "attribute": "light", "buckets": ["day", "night"], "outside_odd": []},
        {"name": "other", "buckets": ["a"]},
    ])
}

/// A run that spent 1 s in each of `cells` (speed, rain, light).
fn run(
    cells: &[[&str; 3]],
    passed: Option<bool>,
    failed_cell: Option<[&str; 3]>,
    env: Option<&str>,
) -> Json {
    let rows: Vec<Json> = cells.iter().map(|c| json!([c, 20, 1.0, 10.0])).collect();
    let mut doc = json!({
        "schema": "openodd.coverage/1", "scenario": "s", "items": items(),
        "odd": {"name": "o", "cells": {"attributes": ATTRS, "rows": rows}},
    });
    if let Some(p) = passed {
        let mut o = json!({"passed": p});
        if let Some(c) = failed_cell {
            o["failed_cell"] = json!(c);
        }
        if let Some(e) = env {
            o["environment"] = json!(e);
        }
        doc["outcome"] = o;
    }
    doc
}

const SLOW_DRY_DAY: [&str; 3] = ["[0, 30)", "none", "day"];
const FAST_DRY_DAY: [&str; 3] = ["[30, 60]", "none", "day"];
const FAST_DRY_NIGHT: [&str; 3] = ["[30, 60]", "none", "night"];
const FAST_WET_DAY: [&str; 3] = ["[30, 60]", "heavy", "day"];

fn docs() -> Vec<Json> {
    vec![
        // Dry and slow always passes; fast at night fails half the time (and
        // fails there); heavy rain always fails, where it is fast.
        run(&[SLOW_DRY_DAY, FAST_DRY_NIGHT], Some(true), None, None),
        run(
            &[SLOW_DRY_DAY, FAST_DRY_NIGHT],
            Some(false),
            Some(FAST_DRY_NIGHT),
            None,
        ),
        run(&[SLOW_DRY_DAY, FAST_DRY_NIGHT], Some(true), None, None),
        run(
            &[SLOW_DRY_DAY, FAST_DRY_NIGHT],
            Some(false),
            Some(FAST_DRY_NIGHT),
            None,
        ),
        run(
            &[SLOW_DRY_DAY, FAST_WET_DAY],
            Some(false),
            Some(FAST_WET_DAY),
            None,
        ),
        run(
            &[SLOW_DRY_DAY, FAST_WET_DAY],
            Some(false),
            Some(FAST_WET_DAY),
            None,
        ),
        run(
            &[SLOW_DRY_DAY, FAST_WET_DAY],
            Some(false),
            Some(FAST_WET_DAY),
            None,
        ),
        run(&[FAST_DRY_DAY], None, None, None), // no outcome
        json!({"schema": "openodd.coverage/1", "scenario": "s", "items": []}), // no cells
    ]
}

#[test]
fn reads_axes_in_bucket_order() {
    let data = Dataset::from_documents(&docs());
    assert_eq!(data.attributes(), ATTRS);
    assert_eq!(data.axes["rain"].labels, ["none", "heavy", "snow"]);
    assert_eq!(data.axes["rain"].outside, ["snow"]);
    assert_eq!(data.axes["speed"].unit, "km/h");
    assert_eq!(data.runs.len(), 8);
    assert_eq!(data.unsampled, 1);
}

#[test]
fn the_top_matrix_counts_and_colours() {
    let data = Dataset::from_documents(&docs());
    let m = data.matrix(&Query::default());
    let (x, y) = (m.x.as_ref().unwrap(), m.y.as_ref().unwrap());
    assert_eq!((x.name.as_str(), y.name.as_str()), ("speed", "rain"));
    let at = |yl: &str, xl: &str| {
        let r = y.labels.iter().position(|l| l == yl).unwrap();
        let c = x.labels.iter().position(|l| l == xl).unwrap();
        m.cells[r][c].clone()
    };
    // Every run with an outcome was slow and dry: 5 of 7 failed (elsewhere).
    let slow_dry = at("none", "[0, 30)");
    assert_eq!(
        (slow_dry.runs, slow_dry.failed, slow_dry.fail_at_cell),
        (7, 5, 0)
    );
    assert_eq!(slow_dry.state, CellState::Flaky);
    let fast_dry = at("none", "[30, 60]");
    assert_eq!(
        (
            fast_dry.runs,
            fast_dry.failed,
            fast_dry.fail_at_cell,
            fast_dry.runs_unknown
        ),
        (4, 2, 2, 1)
    );
    assert_eq!(fast_dry.state, CellState::Flaky);
    let fast_wet = at("heavy", "[30, 60]");
    assert_eq!(
        (fast_wet.runs, fast_wet.failed, fast_wet.fail_at_cell),
        (3, 3, 3)
    );
    assert_eq!(fast_wet.state, CellState::Fail);
    assert!(!fast_wet.tentative); // 3 runs is enough by default
    assert_eq!(at("heavy", "[0, 30)").state, CellState::Unsampled);
    let snow = at("snow", "[0, 30)");
    assert_eq!(snow.state, CellState::Outside);
    assert_eq!(m.total.runs, 7);
    assert_eq!(m.total.runs_unknown, 1);
    assert_eq!(m.counts()["outside"], 2);
}

#[test]
fn clicking_in_keeps_only_the_time_spent_there() {
    let data = Dataset::from_documents(&docs());
    let q = Query {
        conditions: vec![
            ("speed".into(), "[30, 60]".into()),
            ("rain".into(), "none".into()),
        ],
        ..Query::default()
    };
    let m = data.matrix(&q);
    assert_eq!(m.x.as_ref().unwrap().name, "light");
    assert!(m.y.is_none()); // only one attribute left
    let x = m.x.as_ref().unwrap();
    let night = &m.cells[0][x.labels.iter().position(|l| l == "night").unwrap()];
    assert_eq!((night.runs, night.failed, night.fail_at_cell), (4, 2, 2));
    let day = &m.cells[0][x.labels.iter().position(|l| l == "day").unwrap()];
    assert_eq!((day.runs, day.runs_unknown), (0, 1));
    assert_eq!(day.state, CellState::Unknown);
    assert_eq!(m.total.runs, 4);
}

#[test]
fn thresholds_and_selection() {
    let mut docs = docs();
    docs.push(run(&[SLOW_DRY_DAY], Some(true), None, Some("e2")));
    let data = Dataset::from_documents(&docs);
    let mut q = Query {
        min_runs: 10,
        ..Query::default()
    };
    let m = data.matrix(&q);
    assert!(m
        .cells
        .iter()
        .flatten()
        .filter(|c| c.runs > 0)
        .all(|c| c.tentative));
    assert_eq!(m.environments, [None, Some("e2".to_string())]);
    q.environment = Some(Some("e2".into()));
    let only = data.matrix(&q);
    assert_eq!(only.total.runs, 1);
    q.min_seconds = 5.0; // nobody stayed 5 s anywhere
    assert_eq!(data.matrix(&q).total.runs, 0);
    // An axis asked for is used; one taken by a condition is not.
    let q = Query {
        x: Some("light".into()),
        y: Some(String::new()),
        ..Query::default()
    };
    let m = data.matrix(&q);
    assert_eq!(m.x.as_ref().unwrap().name, "light");
    assert!(m.y.is_none());
    let q = Query {
        x: Some("speed".into()),
        conditions: vec![
            ("speed".into(), "[0, 30)".into()),
            ("nope".into(), "x".into()),
        ],
        ..Query::default()
    };
    let m = data.matrix(&q);
    assert_eq!(m.query.conditions.len(), 1);
    assert_eq!(m.x.as_ref().unwrap().name, "rain");
}

#[test]
fn missing_labels_go_last() {
    let data = Dataset::from_documents(&[run(
        &[[MISSING, "none", "day"], ["<above>", "none", "day"]],
        Some(true),
        None,
        None,
    )]);
    let labels = &data.axes["speed"].labels;
    assert_eq!(labels, &["[0, 30)", "[30, 60]", "<above>", MISSING]);
}

#[test]
fn the_fragment_is_host_driven() {
    let data = Dataset::from_documents(&docs());
    let m = data.matrix(&Query::default());
    let opts = Options {
        id: "cov1".into(),
        link: Some("/coverage?{conditions}".into()),
        ..Options::default()
    };
    let html = html::fragment(&data, &m, &opts);
    assert!(html.starts_with("<div class=\"oo-cov\" id=\"cov1\" data-oo-mode=\"host\">"));
    assert!(html.contains("class=\"oo-cell oo-fail\""), "{html}");
    assert!(html.contains("class=\"oo-cell oo-flaky"));
    assert!(html.contains("class=\"oo-cell oo-unsampled\""));
    assert!(html.contains("class=\"oo-cell oo-outside\""));
    // A visited cell links to its conditions; one no run visited does not.
    assert!(
        html.contains("href=\"/coverage?at=rain%3Dheavy&amp;at=speed%3D%5B30%2C%2060%5D\""),
        "{html}"
    );
    assert!(html.contains("data-oo-drill=\"[[&quot;rain&quot;,&quot;heavy&quot;],[&quot;speed&quot;,&quot;[30, 60]&quot;]]\""));
    assert!(html.contains(" disabled"));
    assert!(!html.contains("<script"));
}

#[test]
fn the_explorer_renders_the_levels_below() {
    let data = Dataset::from_documents(&docs());
    let html = html::explorer(&data, &Query::default(), 2, &Options::default());
    // The first level for each axis pair the pickers offer: 3 columns x
    // (no rows + 2 others) = 9; below the asked pair's 3 visited cells only
    // `light` is left, so one level each.
    let shown = html
        .matches("<section class=\"oo-panel\" id=\"oo-cov-p0\" tabindex=\"-1\">")
        .count();
    assert_eq!(shown, 1);
    assert_eq!(html.matches("data-oo-map=").count(), 9);
    assert!(html.contains("data-oo-open=\"oo-cov-p1\""));
    assert!(html.contains(" hidden>"));
    // A level below links back to the whole ODD.
    assert!(html.contains("class=\"oo-crumb\" data-oo-open=\"oo-cov-p0\""));
    let depth0 = html::explorer(&data, &Query::default(), 0, &Options::default());
    assert_eq!(depth0.matches("<section class=\"oo-panel\"").count(), 9);
    assert_eq!(depth0.matches(" hidden>").count(), 8);
}

#[test]
fn text_is_escaped() {
    let doc = json!({
        "schema": "openodd.coverage/1", "scenario": "s", "items": [],
        "odd": {"name": "o", "cells": {"attributes": ["<b>a</b>"], "rows": [[["\"x\"<y>"], 1, 1.0, 0.0]]}},
        "outcome": {"passed": true},
    });
    let data = Dataset::from_documents(&[doc]);
    let page = html::page(
        "t<",
        &html::fragment(&data, &data.matrix(&Query::default()), &Options::default()),
    );
    assert!(!page.contains("<b>a</b>") && page.contains("&lt;b&gt;a&lt;/b&gt;"));
    assert!(page.contains("&quot;x&quot;&lt;y&gt;"));
    assert!(page.contains("<title>t&lt;</title>"));
    assert!(page.contains("openodd:drill"));
}

#[test]
fn the_json_has_every_cell() {
    let data = Dataset::from_documents(&docs());
    let j = data.matrix(&Query::default()).to_json();
    assert_eq!(j["x"]["name"], "speed");
    assert_eq!(j["cells"].as_array().unwrap().len(), 3);
    assert_eq!(j["cells"][1][1]["state"], "fail");
    assert_eq!(j["total"]["runs"], 7);
    assert_eq!(j["counts"]["unsampled"], 1);
}

#[test]
fn a_document_without_cells_adds_no_axis() {
    // An older file with ODD items but no cells, before current ones.
    let old = json!({"schema": "openodd.coverage/1", "scenario": "s", "items": [
        {"name": "odd.legacy", "attribute": "legacy", "buckets": ["a", "b"]}]});
    let mut docs = vec![old];
    docs.extend(docs_without_unsampled());
    let data = Dataset::from_documents(&docs);
    assert!(!data.attributes().contains(&"legacy".to_string()));
    assert_eq!(data.matrix(&Query::default()).total.runs, 7);
}

fn docs_without_unsampled() -> Vec<Json> {
    docs()
        .into_iter()
        .filter(|d| d.get("odd").is_some())
        .collect()
}

#[test]
fn inside_an_outside_cell_everything_is_outside() {
    let mut docs = docs_without_unsampled();
    docs.push(run(&[["[0, 30)", "snow", "day"]], Some(true), None, None));
    let data = Dataset::from_documents(&docs);
    let q = Query {
        conditions: vec![("rain".into(), "snow".into())],
        ..Query::default()
    };
    let m = data.matrix(&q);
    assert!(m.total.outside);
    assert!(m
        .cells
        .iter()
        .flatten()
        .all(|c| c.state == CellState::Outside));
    let visited: u64 = m.cells.iter().flatten().map(|c| c.runs).sum();
    assert_eq!(visited, 1); // still counted, only coloured as outside
}

#[test]
fn default_axes_follow_the_selected_runs() {
    // Another scenario that measured other attributes only.
    let other = json!({"schema": "openodd.coverage/1", "scenario": "t", "items": [],
        "odd": {"name": "o2", "cells": {"attributes": ["fog"], "rows": [[["thick"], 5, 1.0, 0.0]]}},
        "outcome": {"passed": false}});
    let mut docs = docs_without_unsampled();
    docs.push(other);
    let data = Dataset::from_documents(&docs);
    let q = Query {
        scenario: Some("t".into()),
        ..Query::default()
    };
    let m = data.matrix(&q);
    assert_eq!(m.x.as_ref().unwrap().name, "fog");
    assert!(m.y.is_none());
    assert_eq!(m.total.runs, 1);
    assert_eq!(m.cells[0][0].state, CellState::Fail);
    // The whole dataset still lays out its first attributes.
    assert_eq!(data.matrix(&Query::default()).x.unwrap().name, "speed");
}

#[test]
fn every_attribute_in_the_conditions_does_not_panic() {
    let data = Dataset::from_documents(&docs());
    let q = Query {
        conditions: vec![
            ("speed".into(), "[30, 60]".into()),
            ("rain".into(), "none".into()),
            ("light".into(), "night".into()),
        ],
        ..Query::default()
    };
    let m = data.matrix(&q);
    assert!(m.x.is_none() && m.y.is_none());
    assert_eq!(m.total.runs, 4);
    assert!(html::fragment(&data, &m, &Options::default()).contains("oo-note"));
    let m = data.matrix(&Query {
        x: Some(String::new()),
        ..Query::default()
    });
    assert!(m.x.is_none());
    assert_eq!(m.total.runs, 7);
}

#[test]
fn the_total_does_not_depend_on_the_axes() {
    // A run that did not measure `light` still counts in the total.
    let other = json!({"schema": "openodd.coverage/1", "scenario": "s", "items": [],
        "odd": {"name": "o", "cells": {"attributes": ["speed", "rain"],
                "rows": [[["[0, 30)", "none"], 3, 1.0, 0.0]]}},
        "outcome": {"passed": true}});
    let mut docs = docs_without_unsampled();
    docs.push(other);
    let data = Dataset::from_documents(&docs);
    let by_light = data.matrix(&Query {
        x: Some("light".into()),
        ..Query::default()
    });
    let by_default = data.matrix(&Query::default());
    assert_eq!(by_light.total.runs, by_default.total.runs);
    assert_eq!(by_default.total.runs, 8);
}

#[test]
fn bucket_order_is_an_ordered_union() {
    let doc = |buckets: Json, cells: Json| {
        json!({"schema": "openodd.coverage/1", "scenario": "s",
               "items": [{"name": "odd.a", "attribute": "a", "buckets": buckets}],
               "odd": {"name": "o", "cells": {"attributes": ["a"], "rows": cells}}})
    };
    let data = Dataset::from_documents(&[
        doc(
            json!([]),
            json!([[["z"], 1, 1.0, 0.0], [["m"], 1, 1.0, 0.0]]),
        ),
        doc(json!(["a", "b"]), json!([[["b"], 1, 1.0, 0.0]])),
        doc(
            json!(["a", "b", "c"]),
            json!([[["<missing>"], 1, 1.0, 0.0]]),
        ),
    ]);
    assert_eq!(
        data.axes["a"].labels,
        ["a", "b", "c", "m", "z", "<missing>"]
    );
}

#[test]
fn the_explorer_says_when_it_stops_early() {
    // Many buckets on three attributes: more panels than the cap.
    let mut docs = Vec::new();
    for i in 0..30 {
        let rows: Vec<Json> = (0..30)
            .map(|j| json!([[format!("a{i}"), format!("b{j}"), "c"], 1, 1.0, 0.0]))
            .collect();
        docs.push(
            json!({"schema": "openodd.coverage/1", "scenario": "s", "items": [],
            "odd": {"name": "o", "cells": {"attributes": ["a", "b", "c"], "rows": rows}},
            "outcome": {"passed": true}}),
        );
    }
    let data = Dataset::from_documents(&docs);
    let html = html::explorer(&data, &Query::default(), 1, &Options::default());
    assert_eq!(
        html.matches("<section class=\"oo-panel\"").count(),
        html::MAX_PANELS
    );
    assert!(html.contains("were not rendered ahead of time"));
}

#[test]
fn the_pickers_offer_the_free_attributes() {
    let data = Dataset::from_documents(&docs());
    let m = data.matrix(&Query::default());
    let opts = Options {
        link: Some("/c?{conditions}".into()),
        ..Options::default()
    };
    let html = html::fragment(&data, &m, &opts);
    assert!(html.contains("<select data-oo-axis=\"y\" data-oo-current=\"rain\">"));
    assert!(html.contains("<select data-oo-axis=\"x\" data-oo-current=\"speed\">"));
    assert!(html.contains("<option value=\"light\">light</option>"));
    assert!(html.contains("data-oo-link=\"/c?{conditions}\""));
    // Inside a cell, only the attributes left.
    let inner = data.matrix(&Query {
        conditions: vec![("speed".into(), "[30, 60]".into())],
        ..Query::default()
    });
    let html = html::fragment(&data, &inner, &opts);
    assert!(!html.contains("<option value=\"speed\""));
    assert!(html.contains("data-oo-at=\"at=speed%3D%5B30%2C%2060%5D\""));
}
