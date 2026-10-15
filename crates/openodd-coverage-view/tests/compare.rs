//! Two sets of runs on one ODD, compared: the matrix on the same axes, and
//! the cases matched by their overrides.

use openodd_coverage_view::compare::{Change, Comparison, Side};
use openodd_coverage_view::{CaseQuery, Query};
use serde_json::{json, Value as Json};

/// A run that spent 1 s in `cell` (speed, rain); `light` too when given.
fn run(speed: &str, rain: &str, light: Option<&str>, passed: bool) -> Json {
    let (attrs, cell) = match light {
        Some(l) => (json!(["speed", "rain", "light"]), json!([speed, rain, l])),
        None => (json!(["speed", "rain"]), json!([speed, rain])),
    };
    let mut items = vec![
        json!({"attribute": "speed", "buckets": ["slow", "fast"], "outside_odd": []}),
        json!({"attribute": "rain", "buckets": ["none", "heavy"], "outside_odd": []}),
    ];
    if light.is_some() {
        items.push(json!({"attribute": "light", "buckets": ["day", "night"], "outside_odd": []}));
    }
    json!({
        "schema": "openodd.coverage/1", "scenario": "s", "items": items,
        "odd": {"name": "o", "structure": "abc",
                "cells": {"attributes": attrs, "rows": [[cell, 10, 1.0, 5.0]]}},
        "case": {"values": {"rain": rain, "speed": speed}, "overrides": [format!("rain={rain}"), format!("speed={speed}")],
                 "space": [{"name": "rain", "labels": ["none", "heavy"]}, {"name": "speed", "labels": ["slow", "fast"]}]},
        "outcome": {"passed": passed},
    })
}

#[test]
fn both_sides_are_counted_on_the_same_axes() {
    let base = vec![
        run("slow", "none", None, true),
        run("fast", "heavy", None, true),
        run("fast", "heavy", None, true),
    ];
    // The other set also measured light: the matrix may lay it out, and the
    // baseline, which did not, still has a cell for every one of its cells.
    let other = vec![
        run("slow", "none", Some("day"), true),
        run("fast", "heavy", Some("night"), false),
        run("fast", "heavy", Some("night"), false),
        run("fast", "none", Some("day"), true),
    ];
    let cmp = Comparison::new(&base, &other);
    assert!(!cmp.odd.same());
    assert_eq!(cmp.odd.attributes[0].only_in, Some(Side::Other));
    let m = cmp.matrix(&Query {
        x: Some("speed".into()),
        y: Some("rain".into()),
        ..Default::default()
    });
    assert_eq!(m.base.cells.len(), m.other.cells.len());
    let at = |speed: usize, rain: usize| m.deltas[rain][speed].change;
    assert_eq!(at(0, 0), Change::Same);
    assert_eq!(at(1, 1), Change::Regressed);
    assert_eq!(at(1, 0), Change::New);
    assert_eq!(at(0, 1), Change::Neither);
    assert_eq!(m.counts()["regressed"], 1);
    assert_eq!(m.total.change, Change::Regressed);
    // An axis only one side has: the other side's cells are all empty.
    let l = cmp.matrix(&Query {
        x: Some("light".into()),
        y: Some("".into()),
        ..Default::default()
    });
    assert_eq!(l.union.x.as_ref().unwrap().name, "light");
    assert!(l.base.cells[0].iter().all(|c| c.runs == 0));
    assert_eq!(l.deltas[0][0].change, Change::New);
    let j = m.to_json();
    assert_eq!(j["cells"][1][1]["change"], "regressed");
    assert_eq!(j["cells"][1][1]["base"]["runs"], 2);
    assert_eq!(j["runs"]["other"], 4);

    // The cases: matched by their overrides across the sets.
    let p = cmp.plot(&CaseQuery::default());
    assert_eq!(p.cases.len(), 3);
    let fast_heavy = p
        .plot
        .cases
        .iter()
        .position(|c| c.values["rain"] == "heavy")
        .unwrap();
    let cc = &p.cases[fast_heavy];
    assert_eq!(cc.delta.change, Change::Regressed);
    assert_eq!(
        (
            cc.base.as_ref().unwrap().runs.len(),
            cc.other.as_ref().unwrap().runs.len()
        ),
        (2, 2)
    );
    assert_eq!(p.changed()[0], fast_heavy);
    assert_eq!(p.counts()["new"], 1);
    assert_eq!(cmp.cases_of(Side::Base).runs(), 3);
}
