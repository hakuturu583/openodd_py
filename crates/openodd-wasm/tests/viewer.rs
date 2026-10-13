//! The viewer on the demo's sample: natively (`cargo test`) and as
//! WebAssembly in Node (`wasm-pack test --node crates/openodd-wasm`).

use openodd_wasm::{script, styles, Viewer};
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
