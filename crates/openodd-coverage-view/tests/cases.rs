//! The case plot over synthetic coverage documents, and its HTML.

use openodd_coverage_view::case_plot::{self, Options};
use openodd_coverage_view::cases::{AxisKind, CaseQuery, CaseSet, Verdict};
use openodd_coverage_view::html::MAX_AXIS_PAIRS;
use openodd_coverage_view::prediction::{Grid, GridAxis, Precomputed, Predicted};
use serde_json::{json, Value as Json};

fn items() -> Json {
    json!([
        {"name": "odd.gap", "attribute": "gap", "unit": "m",
         "buckets": ["[0, 10)", "[10, 20)", "[20, 30)", "[30, 50)"]},
        {"name": "odd.speed", "attribute": "speed", "unit": "km/h",
         "buckets": ["[-15, -5)", "[-5, 5)"]},
        {"name": "odd.rain", "attribute": "rain", "buckets": ["none", "heavy"]},
    ])
}

fn space() -> Json {
    json!([
        {"name": "rain", "labels": ["none", "heavy"]},
        {"name": "gap", "range": [6.0, 30.0]},
        {"name": "speed", "range": [-15.0, 5.0]},
    ])
}

/// A run of the case drawn with these values, and its outcome.
fn run(gap: f64, speed: f64, rain: &str, passed: Option<bool>, chosen: &str) -> Json {
    let flaky = if chosen == "instability" {
        json!({"pass_probability": 0.5, "flip_probability": 0.3,
               "instability_confidence": 0.9, "robustness_radius": 0.04})
    } else {
        Json::Null
    };
    let mut doc = json!({
        "schema": "openodd.coverage/1", "scenario": "cut_in", "items": items(),
        "case": {
            "index": 0, "situation": null,
            "values": {"rain": rain, "gap": gap, "speed": speed},
            "buckets": {"rain": rain, "gap": "[10, 20)", "speed": "[-15, -5)"},
            "overrides": [format!("gap={gap}"), format!("speed={speed}"), format!("rain={rain}")],
            "chosen_for": chosen, "flaky": flaky, "space": space(),
        },
    });
    if let Some(p) = passed {
        doc["outcome"] = json!({"passed": p, "environment": "e1"});
    }
    doc
}

fn docs() -> Vec<Json> {
    vec![
        run(20.4, 3.9, "none", Some(true), "coverage"),
        run(19.1, -13.1, "heavy", Some(false), "coverage"),
        run(20.4, 3.9, "none", Some(true), "coverage"),
        run(19.1, -13.1, "heavy", Some(false), "coverage"),
        // Two cases at nearly the same values: they must not hide each other.
        run(13.0, -7.0, "none", Some(true), "instability"),
        run(13.2, -7.0, "none", Some(false), "instability"),
        // Run twice: once passed, once failed.
        run(15.1, -0.3, "heavy", Some(true), "coverage"),
        run(15.1, -0.3, "heavy", Some(false), "coverage"),
        // Not placed.
        run(25.0, 0.0, "none", None, "coverage"),
        json!({"schema": "openodd.coverage/1", "scenario": "cut_in", "items": items(), "case": null}),
    ]
}

#[test]
fn cases_are_grouped_and_judged() {
    let set = CaseSet::from_documents(&docs());
    assert_eq!(set.cases.len(), 5);
    assert_eq!(set.runs(), 8);
    assert_eq!((set.without_case, set.without_outcome), (1, 1));
    assert_eq!(set.attributes(), vec!["rain", "gap", "speed"]);
    let verdicts: Vec<Verdict> = set.cases.iter().map(|c| c.verdict()).collect();
    assert_eq!(
        verdicts,
        vec![
            Verdict::Pass,
            Verdict::Fail,
            Verdict::Pass,
            Verdict::Fail,
            Verdict::Flaky
        ]
    );
    assert!(set.cases[2].for_instability());
    assert_eq!(set.axes["gap"].kind, AxisKind::Numeric);
    assert_eq!(set.axes["gap"].edges(), vec![10.0, 20.0, 30.0]);
    assert_eq!(set.axes["rain"].kind, AxisKind::Categorical);
}

#[test]
fn default_axes_are_numeric_and_asked_ones_are_kept() {
    let set = CaseSet::from_documents(&docs());
    let plot = set.plot(&CaseQuery::default());
    assert_eq!(plot.x.as_ref().unwrap().name, "gap");
    assert_eq!(plot.y.as_ref().unwrap().name, "speed");
    let plot = set.plot(&CaseQuery {
        x: Some("speed".into()),
        y: Some("rain".into()),
        ..Default::default()
    });
    assert_eq!(plot.x.as_ref().unwrap().name, "speed");
    assert_eq!(plot.y.as_ref().unwrap().name, "rain");
    // An unknown axis falls back; the same one twice does not repeat.
    let plot = set.plot(&CaseQuery {
        x: Some("nope".into()),
        y: Some("gap".into()),
        ..Default::default()
    });
    assert_eq!(plot.x.as_ref().unwrap().name, "gap");
    assert_eq!(plot.y.as_ref().unwrap().name, "speed");
    let repeated: Vec<Verdict> = plot.repeated().iter().map(|c| c.verdict()).collect();
    assert_eq!(repeated, vec![Verdict::Flaky, Verdict::Fail, Verdict::Pass]);
    let env = set.plot(&CaseQuery {
        environment: Some(Some("other".into())),
        ..Default::default()
    });
    assert!(env.cases.is_empty());
}

#[test]
fn the_json_has_every_case() {
    let set = CaseSet::from_documents(&docs());
    let j = set.plot(&CaseQuery::default()).to_json();
    assert_eq!(j["cases"].as_array().unwrap().len(), 5);
    assert_eq!(j["counts"]["flaky"], 1);
    assert_eq!(j["x"]["kind"], "numeric");
    assert_eq!(j["x"]["domain"], json!([6.0, 30.0]));
    assert_eq!(j["documents_without_outcome"], 1);
    let flaky = j["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["verdict"] == "flaky")
        .unwrap();
    assert_eq!(flaky["passes"], 1);
    assert_eq!(flaky["runs"].as_array().unwrap().len(), 2);
}

/// The marks' positions in one rendering (`oo-plot-wide` or `-compact`).
fn marks(html: &str, class: &str) -> Vec<(f64, f64)> {
    let start = html.find(&format!("oo-svg {class}")).unwrap();
    let end = start + html[start..].find("</svg>").unwrap();
    html[start..end]
        .split("<g class=\"oo-case ")
        .skip(1)
        .map(|g| {
            let t = &g[g.find("translate(").unwrap() + 10..];
            let t = &t[..t.find(')').unwrap()];
            let mut it = t.split(' ').map(|v| v.parse::<f64>().unwrap());
            (it.next().unwrap(), it.next().unwrap())
        })
        .collect()
}

#[test]
fn the_fragment_draws_each_case_once_apart() {
    let set = CaseSet::from_documents(&docs());
    let plot = set.plot(&CaseQuery {
        x: Some("speed".into()),
        y: Some("gap".into()),
        ..Default::default()
    });
    let opts = Options {
        id: "cp".into(),
        link: Some("/cases?{axes}".into()),
        instability_rings: true,
        ..Default::default()
    };
    let html = case_plot::fragment(&set, &plot, &opts);
    assert!(html.starts_with("<div class=\"oo-cov oo-cases\" id=\"cp\" data-oo-mode=\"host\">"));
    for class in ["oo-plot-wide", "oo-plot-compact"] {
        let at = marks(&html, class);
        assert_eq!(at.len(), 5);
        for i in 0..at.len() {
            for j in i + 1..at.len() {
                let d = (at[i].0 - at[j].0).hypot(at[i].1 - at[j].1);
                assert!(d > 10.0, "{class}: marks {i} and {j} are {d} apart");
            }
        }
    }
    // Shapes, the instability outline, the n/m labels, the leader lines.
    assert!(html.contains("oo-case oo-flaky"));
    assert!(html.contains("oo-case oo-pass oo-instab"));
    assert!(html.contains("oo-case oo-fail oo-instab"));
    // Without the option, no outline: the card and the table still say why.
    let plain = case_plot::fragment(&set, &plot, &Options::default());
    assert!(!plain.contains("oo-instab") && !plain.contains("class=\"oo-inst\""));
    assert!(plain.contains("[\"Chosen for\",\"instability\"]"));
    assert!(plain.contains("<th scope=\"col\">Chosen for</th>"));
    assert!(html.contains(">1/2</text>") && html.contains(">0/2</text>"));
    assert!(html.contains("class=\"oo-lead\""));
    // The pickers and the cards.
    assert!(html.contains("data-oo-view=\"cases\" data-oo-link=\"/cases?{axes}\""));
    assert!(html.contains("data-oo-axis=\"x\" data-oo-current=\"speed\""));
    assert!(html.contains("<script type=\"application/json\" class=\"oo-tips\">"));
    assert!(html.contains("Robustness radius"));
    // The table and the notes.
    assert!(html.contains("Repeated cases") && html.contains("oo-o oo-o-fail"));
    assert!(html.contains("2 coverage files without a case or an outcome"));
}

#[test]
fn text_is_escaped() {
    let mut d = docs();
    d[0]["case"]["values"]["rain"] = json!("<b>&</b>");
    d[0]["case"]["space"][0]["labels"] = json!(["<b>&</b>", "none", "heavy"]);
    d[0]["case"]["buckets"]["rain"] = json!("</script>");
    let set = CaseSet::from_documents(&d);
    let plot = set.plot(&CaseQuery {
        x: Some("rain".into()),
        ..Default::default()
    });
    let html = case_plot::fragment(&set, &plot, &Options::default());
    assert!(!html.contains("<b>&</b>"));
    assert!(html.contains("&lt;b&gt;&amp;&lt;/b&gt;"));
    assert_eq!(html.matches("</script>").count(), 1);
}

#[test]
fn the_explorer_renders_every_pair() {
    let set = CaseSet::from_documents(&docs());
    let html = case_plot::explorer(
        &set,
        &CaseQuery {
            x: Some("speed".into()),
            y: Some("gap".into()),
            ..Default::default()
        },
        &Options::default(),
    );
    assert_eq!(html.matches("<section class=\"oo-panel\"").count(), 6);
    assert_eq!(html.matches(" hidden>").count(), 5);
    assert!(html.contains("data-oo-map="));
    assert!(html.contains("oo-cases-p5"));
    assert_eq!(html.matches("class=\"oo-tips\"").count(), 1);
    let page = case_plot::page("Cases", &html);
    assert!(page.starts_with("<!doctype html>") && page.contains("oo-tip-i"));
}

#[test]
fn many_attributes_cap_the_pairs() {
    let names: Vec<String> = (0..14).map(|i| format!("a{i}")).collect();
    let doc = json!({
        "scenario": "s",
        "case": {
            "values": names.iter().map(|n| (n.clone(), json!(1.0))).collect::<serde_json::Map<_, _>>(),
            "overrides": ["x=1"], "chosen_for": "coverage",
            "space": names.iter().map(|n| json!({"name": n, "range": [0.0, 2.0]})).collect::<Vec<_>>(),
        },
        "outcome": {"passed": true},
    });
    let set = CaseSet::from_documents(&[doc]);
    let html = case_plot::explorer(&set, &CaseQuery::default(), &Options::default());
    let panels = html.matches("<section class=\"oo-panel\"").count();
    assert!(panels <= MAX_AXIS_PAIRS && panels > 50, "{panels}");
}

#[test]
fn nothing_to_plot_still_renders() {
    let set = CaseSet::from_documents(&[json!({"scenario": "s", "case": null})]);
    let plot = set.plot(&CaseQuery::default());
    assert!(plot.x.is_none());
    let html = case_plot::fragment(&set, &plot, &Options::default());
    assert!(html.contains("No case with drawn values"));
    assert!(case_plot::table(&plot, &Options::default()).contains("No case was run more than once"));
}

#[test]
fn labels_translate() {
    let set = CaseSet::from_documents(&docs());
    let plot = set.plot(&CaseQuery::default());
    let mut opts = Options {
        table: false,
        ..Default::default()
    };
    assert!(opts.labels.set("title", "抽選ケース"));
    assert!(opts.labels.set("state_flaky", "不安定"));
    let html = case_plot::fragment(&set, &plot, &opts);
    assert!(html.contains("抽選ケース") && html.contains("不安定"));
    assert!(!html.contains("Repeated cases"));
}

fn cell(p: f64, state: Verdict) -> Option<Predicted> {
    Some(Predicted {
        pass_probability: p,
        flip_probability: 0.1,
        instability_confidence: 0.2,
        state,
    })
}

/// A grid over speed (2 cells) x gap (2 cells): fails slow and near, passes
/// fast and far, flaky between.
fn grid() -> Grid {
    Grid {
        x: GridAxis::Numeric {
            name: "speed".into(),
            edges: vec![-15.0, -5.0, 5.0],
        },
        y: GridAxis::Numeric {
            name: "gap".into(),
            edges: vec![6.0, 18.0, 30.0],
        },
        cells: vec![
            vec![cell(0.1, Verdict::Fail), cell(0.5, Verdict::Flaky)],
            vec![cell(0.2, Verdict::Fail), cell(0.9, Verdict::Pass)],
        ],
        held: vec![("rain".into(), "none".into())],
        runs: 8,
        flip_threshold: 0.1,
        flaky_confidence: 0.5,
    }
}

#[test]
fn the_background_is_what_the_predictor_gives() {
    let set = CaseSet::from_documents(&docs());
    let plot = set.plot(&CaseQuery {
        x: Some("speed".into()),
        y: Some("gap".into()),
        ..Default::default()
    });
    let mut cases = std::collections::HashMap::new();
    cases.insert(plot.cases[0].key.clone(), 0.875);
    let predictor = Precomputed {
        grids: vec![grid()],
        cases,
        unavailable: None,
    };
    let html = case_plot::fragment_with(&set, &plot, &Options::default(), Some(&predictor));
    // Drawn in both sizes, behind the marks, coloured by the pass
    // probability to the nearest 5%: 0.1 -> step 2, 0.5 -> 10, 0.2 -> 4,
    // 0.9 -> 18.
    assert_eq!(html.matches("<g class=\"oo-pred\"").count(), 2);
    for k in [2, 10, 4, 18] {
        assert_eq!(
            html.matches(&format!("<rect class=\"oo-pp-{k}\"")).count(),
            2,
            "{k}"
        );
    }
    assert!(html.find("<rect class=\"oo-pp-").unwrap() < html.find("role=\"img\"").unwrap());
    // Dashed lines where it crosses 30% (between 0.2 and 0.9, 0.1 and 0.5)
    // and 70% (between 0.5 and 0.9, 0.2 and 0.9).
    assert_eq!(html.matches("class=\"oo-pred-contour\"").count(), 4);
    assert!(html.contains("Predicted pass rate") && html.contains("50% unstable"));
    assert!(!html.contains("Predicted: flaky"));
    assert!(html.contains("fitted to these 8 runs"));
    assert!(html.contains("held at the cases&#39; typical values: rain = none"));
    assert!(html.contains("[\"Pass probability (model, now)\",\"88%\"]"));
    assert_eq!(grid().to_json()["cells"][1][1]["state"], "pass");
    // Another pair: no grid for it, and the reason.
    let other = set.plot(&CaseQuery {
        x: Some("gap".into()),
        y: Some("rain".into()),
        ..Default::default()
    });
    let none = Precomputed {
        unavailable: Some("every run passed, so there is nothing to tell apart".into()),
        ..Default::default()
    };
    let html = case_plot::fragment_with(&set, &other, &Options::default(), Some(&none));
    assert!(!html.contains("<g class=\"oo-pred\""));
    assert!(html.contains("No predicted background: every run passed"));
    // No predictor, no background and no note.
    let html = case_plot::fragment(&set, &plot, &Options::default());
    assert!(!html.contains("oo-pred"));
}

#[test]
fn the_card_shrinks_with_its_container() {
    // No fixed width anywhere on the root, the panels or the plots: the
    // SVGs scale by their viewBox, and the table's unwrapped width is kept
    // out of the card's minimum (it scrolls in its own box), so a host's
    // grid or flex column does not grow to fit it.
    let set = CaseSet::from_documents(&docs());
    let html = case_plot::explorer(&set, &CaseQuery::default(), &Options::default());
    assert!(!html.contains("style=\"width") && !html.contains("min-width"));
    for svg in html.split("<svg class=\"oo-svg ").skip(1) {
        let tag = &svg[..svg.find('>').unwrap()];
        assert!(tag.contains("viewBox") && !tag.contains("width="), "{tag}");
    }
    let css = openodd_coverage_view::html::styles();
    assert!(css.contains(".oo-cov.oo-cases { min-width: 0; max-width: 100%; }"));
    let scroll = &css[css.find(".oo-cov .oo-table-scroll {").unwrap()..];
    assert!(scroll[..scroll.find('}').unwrap()].contains("container-type: inline-size"));
}
