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
