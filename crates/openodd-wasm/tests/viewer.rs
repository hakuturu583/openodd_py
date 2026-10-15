//! The viewer on the demo's sample: natively (`cargo test`) and as
//! WebAssembly in Node (`wasm-pack test --node crates/openodd-wasm`).

use openodd_wasm::{script, styles, Comparison, Viewer};
use serde_json::Value as Json;
use wasm_bindgen_test::wasm_bindgen_test;

const SAMPLE: &str = include_str!("../../../examples/wasm-viewer/sample.json");

fn viewer() -> Viewer {
    Viewer::from_json(SAMPLE).expect("the sample reads")
}

#[wasm_bindgen_test]
#[test]
fn it_reads_the_documents() {
    let v = viewer();
    assert_eq!(v.documents(), 35);
    assert_eq!(v.cases(), 30);
    let attrs: Vec<String> = serde_json::from_str(&v.case_attributes()).unwrap();
    assert_eq!(attrs, vec!["gap", "relative_speed", "rain"]);
    let odd: Vec<String> = serde_json::from_str(&v.attributes()).unwrap();
    assert!(odd.contains(&"gap".to_string()));
    assert!(Viewer::from_json("42").is_err());
}

#[wasm_bindgen_test]
#[test]
fn the_case_plot_has_its_background_computed_here() {
    let v = viewer();
    let html = v
        .cases_html_str(
            r#"{"x": "relative_speed", "y": "gap"}"#,
            r#"{"id": "cp", "labels": {"predicted_scale": "予測成功率"}}"#,
        )
        .unwrap();
    assert!(html.starts_with("<div class=\"oo-cov oo-cases\" id=\"cp\""));
    assert!(html.contains("<g class=\"oo-pred\"") && html.contains("予測成功率"));
    // A gradient: short gaps predicted to fail, long ones to pass.
    let steps: Vec<usize> = html
        .split("<rect class=\"oo-pp-")
        .skip(1)
        .map(|s| s[..s.find('"').unwrap()].parse().unwrap())
        .collect();
    assert!(steps.iter().min().unwrap() < &8 && steps.iter().max().unwrap() > &12);
    // Another pair: rendered from the same model (cached), no background
    // when asked not to.
    let fog = v
        .cases_html_str(r#"{"x": "relative_speed", "y": "rain"}"#, "{}")
        .unwrap();
    assert!(fog.contains("data-oo-current=\"rain\""));
    let plain = v
        .cases_html_str(r#"{"x": "gap"}"#, r#"{"prediction": false}"#)
        .unwrap();
    assert!(!plain.contains("oo-pred"));
    assert!(v
        .cases_html_str("{}", r#"{"labels": {"nope": "x"}}"#)
        .is_err());
}

#[wasm_bindgen_test]
#[test]
fn the_json_has_the_full_grid() {
    let v = viewer();
    let data: Json = serde_json::from_str(
        &v.cases_json_str(r#"{"x": "gap", "y": "rain"}"#, true)
            .unwrap(),
    )
    .unwrap();
    let grid = &data["prediction"];
    assert_eq!(grid["x"]["name"], "gap");
    let cell = &grid["cells"][0][0];
    assert!(cell["state"].is_string() && cell["instability_confidence"].is_number());
    assert!(data["cases"][0]["predicted_pass_probability"].is_number());
    // The same documents give the same grid (a deterministic seed).
    let again: Json = serde_json::from_str(
        &viewer()
            .cases_json_str(r#"{"x": "gap", "y": "rain"}"#, true)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(again["prediction"], data["prediction"]);
}

#[wasm_bindgen_test]
#[test]
fn the_coverage_matrix_and_the_assets() {
    let v = viewer();
    let html = v
        .coverage_html_str(r#"{"x": "gap", "y": "rain"}"#, r#"{"id": "cv"}"#)
        .unwrap();
    assert!(html.starts_with("<div class=\"oo-cov\" id=\"cv\" data-oo-mode=\"host\">"));
    let m: Json =
        serde_json::from_str(&v.coverage_json_str(r#"{"x": "gap", "y": "rain"}"#).unwrap())
            .unwrap();
    assert_eq!(m["x"]["name"], "gap");
    assert!(styles().contains(".oo-cases") && script().contains("openodd:axes"));
}

#[wasm_bindgen_test]
#[test]
fn the_drift_view_finds_the_samples_offset() {
    let v = viewer();
    let d: Json = serde_json::from_str(&v.drift_json_str("{}", "{}").unwrap()).unwrap();
    let gap = d["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["attribute"] == "gap")
        .unwrap();
    // Staged 1.5 m long; the first sample is taken a tick into the run.
    let b = gap["fit"]["b"].as_f64().unwrap();
    assert!(b > 1.0 && b < 1.6, "{b}");
    assert_eq!(gap["calibration"]["status"], "suggest");
    // With the knob's current offset, the corrected one: 11 - b.
    let with: Json = serde_json::from_str(
        &v.drift_json_str("{}", r#"{"knobs": {"gap": [1, 11]}}"#)
            .unwrap(),
    )
    .unwrap();
    let knob = with["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["attribute"] == "gap")
        .unwrap()["calibration"]["knob"]
        .clone();
    let a = gap["fit"]["a"].as_f64().unwrap();
    let o = knob["corrected"]["offset"].as_f64().unwrap();
    assert!((o - (11.0 - b / a)).abs() < 1e-9, "{o}");
    let html = v
        .drift_html_str("{}", r#"{"knobs": {"gap": [1, 11]}}"#)
        .unwrap();
    assert!(html.contains("Scenario realisation") && html.contains("oo-calib-suggest"));
    let measured = v
        .cases_html_str(r#"{"x": "gap", "y": "rain", "basis": "measured"}"#, "{}")
        .unwrap();
    assert!(measured.contains("data-oo-basis-current=\"measured\""));
}

const NEXT: &str = include_str!("../../../examples/wasm-viewer/sample_compare.json");

fn comparison() -> Comparison {
    Comparison::from_json(SAMPLE, NEXT, ["v1", "v2"]).expect("both samples read")
}

#[wasm_bindgen_test]
#[test]
fn a_comparison_reads_both_sets_on_one_odd() {
    let c = comparison();
    assert_eq!((c.documents(), c.split(), c.cases()), (70, 35, 30));
    let odd: Json = serde_json::from_str(&c.odd()).unwrap();
    assert_eq!(odd["same"], true, "{odd}");
    assert_eq!(odd["base"][0]["name"], "cut_in");
}

#[wasm_bindgen_test]
#[test]
fn the_compared_cases_find_the_next_versions_changes() {
    let c = comparison();
    let data: Json = serde_json::from_str(
        &c.cases_json_str(r#"{"x": "gap", "y": "rain"}"#, false)
            .unwrap(),
    )
    .unwrap();
    // The sample's next version: worse in heavy rain, better in none.
    assert_eq!(data["changes"]["regressed"], 2, "{}", data["changes"]);
    assert_eq!(data["changes"]["improved"], 3);
    assert_eq!(data["changes"]["new"], 0);
    assert_eq!(data["runs"]["base"], 35);
    for case in data["cases"].as_array().unwrap() {
        let rain = case["values"]["rain"].as_str().unwrap();
        match case["change"].as_str().unwrap() {
            "regressed" => assert_eq!(rain, "heavy"),
            "improved" => assert_eq!(rain, "none"),
            other => assert_eq!(other, "same"),
        }
        assert_eq!(case["base"]["runs"], case["other"]["runs"]);
    }
    let html = c
        .cases_html_str(
            r#"{"x": "gap", "y": "rain"}"#,
            r#"{"id": "cmp", "labels": {"regressed": "悪化"}}"#,
        )
        .unwrap();
    assert!(html.starts_with("<div class=\"oo-cov oo-cases oo-cmp\" id=\"cmp\""));
    assert!(html.contains("oo-cmp-case oo-chg-regressed") && html.contains("悪化"));
    assert!(html.contains("v1") && html.contains("v2"));
    // The change of the predicted pass rate behind the marks, by default.
    assert!(html.contains("<rect class=\"oo-pd-") && html.contains("oo-delta-bar"));
    let one = c
        .cases_html_str(r#"{"x": "gap", "y": "rain"}"#, r#"{"background": "other"}"#)
        .unwrap();
    assert!(one.contains("<rect class=\"oo-pp-") && !one.contains("<rect class=\"oo-pd-"));
    let none = c.cases_html_str("{}", r#"{"background": "none"}"#).unwrap();
    assert!(!none.contains("oo-pred"));
    assert!(c.cases_html_str("{}", r#"{"background": "x"}"#).is_err());
    assert!(c
        .cases_html_str("{}", r#"{"labels": {"nope": "x"}}"#)
        .is_err());
    let with: Json = serde_json::from_str(
        &c.cases_json_str(r#"{"x": "gap", "y": "rain"}"#, true)
            .unwrap(),
    )
    .unwrap();
    assert!(with["prediction"]["delta"]["cells"].is_array());
    assert!(with["cases"][0]["predicted_pass_probability"]["other"].is_number());
}

#[wasm_bindgen_test]
#[test]
fn the_compared_matrix_splits_each_cell() {
    let c = comparison();
    let m: Json =
        serde_json::from_str(&c.coverage_json_str(r#"{"x": "gap", "y": "rain"}"#).unwrap())
            .unwrap();
    assert_eq!(m["x"]["name"], "gap");
    assert_eq!(m["names"], serde_json::json!(["v1", "v2"]));
    let rows = m["y"]["labels"].as_array().unwrap();
    let row = |l: &str| rows.iter().position(|r| r == l).unwrap();
    let changes = |r: usize| -> Vec<String> {
        m["cells"][r]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["change"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(changes(row("heavy")).contains(&"regressed".to_string()));
    assert!(changes(row("none")).contains(&"improved".to_string()));
    assert!(!changes(row("light"))
        .iter()
        .any(|c| c == "regressed" || c == "improved"));
    let cell = &m["cells"][0][0];
    assert!(cell["base"]["runs"].is_number() && cell["other"]["state"].is_string());
    let html = c
        .coverage_html_str(r#"{"x": "gap", "y": "rain"}"#, r#"{"id": "cm"}"#)
        .unwrap();
    assert!(html.starts_with("<div class=\"oo-cov oo-cmp\" id=\"cm\" data-oo-mode=\"host\">"));
    assert!(html.contains("oo-half-base") && html.contains("oo-chg-improved"));
    assert!(html.contains("data-oo-drill") && !html.contains("oo-cmp-odd"));
}

#[wasm_bindgen_test]
#[test]
fn a_comparison_says_when_the_odds_differ() {
    // The next version's gap cut at other edges, and one run without rain.
    let mut next: Vec<Json> = serde_json::from_str(NEXT).unwrap();
    for doc in &mut next {
        doc["odd"]["structure"] = "another".into();
        for item in doc["items"].as_array_mut().unwrap() {
            if item["attribute"] == "gap" {
                item["buckets"] = serde_json::json!(["[0, 15)", "[15, 50]"]);
            }
        }
    }
    let c = Comparison::from_json(SAMPLE, &Json::Array(next).to_string(), ["a", "b"]).unwrap();
    let odd: Json = serde_json::from_str(&c.odd()).unwrap();
    assert_eq!(odd["same"], false);
    assert_eq!(odd["attributes"][0]["attribute"], "gap");
    let html = c.coverage_html_str("{}", "{}").unwrap();
    assert!(html.contains("oo-cmp-odd"));
}

#[wasm_bindgen_test]
#[test]
fn the_models_keep_all_environments_apart_from_none_recorded() {
    // The sample's runs all recorded an environment: those that recorded
    // none are no runs, and no model; every environment's model still is.
    let q_none = r#"{"x": "gap", "y": "rain", "environment": null}"#;
    let q_all = r#"{"x": "gap", "y": "rain"}"#;
    let v = viewer();
    assert!(!v
        .cases_html_str(q_none, "{}")
        .unwrap()
        .contains("<g class=\"oo-pred\""));
    assert!(v
        .cases_html_str(q_all, "{}")
        .unwrap()
        .contains("<g class=\"oo-pred\""));
    let c = comparison();
    assert!(!c.cases_html_str(q_none, "{}").unwrap().contains("oo-pd-"));
    assert!(c
        .cases_html_str(q_all, "{}")
        .unwrap()
        .contains("<rect class=\"oo-pd-"));
}
