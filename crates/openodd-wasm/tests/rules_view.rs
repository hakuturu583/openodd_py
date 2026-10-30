//! The rules view on the demo's sample: natively (`cargo test`) and as
//! WebAssembly in Node (`wasm-pack test --node crates/openodd-wasm`).

use openodd_wasm::{script, styles, RulesView};
use serde_json::Value as Json;
use wasm_bindgen_test::wasm_bindgen_test;

const SAMPLE: &str = include_str!("../../../examples/wasm-viewer/sample_rules.yaml");

fn view() -> RulesView {
    RulesView::from_text(SAMPLE, "sample_rules.yaml").expect("the sample reads")
}

#[wasm_bindgen_test]
#[test]
fn it_reads_the_rules_and_refuses_bad_ones() {
    let v = view();
    let q: Vec<String> = serde_json::from_str(&v.quantities()).unwrap();
    assert_eq!(q, vec!["tire_friction", "visibility"]);
    let inputs: Json = serde_json::from_str(&v.inputs_str("visibility").unwrap()).unwrap();
    let names: Vec<&str> = inputs
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["fog_density", "precipitation", "sun_altitude_angle"]
    );
    let e = RulesView::from_text(
        &SAMPLE.replace("(water IS wet)", "(water IS soaked)"),
        "x.yaml",
    )
    .err()
    .unwrap();
    assert!(
        e.contains("x.yaml: ") && e.contains("no term 'soaked'"),
        "{e}"
    );
}

#[wasm_bindgen_test]
#[test]
fn it_evaluates_as_the_core_does() {
    let v = view();
    let out: Json =
        serde_json::from_str(&v.evaluate_str("tire_friction", r#"{"water": 70}"#).unwrap())
            .unwrap();
    assert!((out["low"].as_f64().unwrap() - 0.57).abs() < 1e-12);
    assert!((out["high"].as_f64().unwrap() - 0.77).abs() < 1e-12);
}

#[wasm_bindgen_test]
#[test]
fn the_component_shows_terms_rules_outputs_and_the_response() {
    let v = view();
    let html = v
        .rules_html_str(
            r#"{"quantity": "visibility", "inputs": {"fog_density": 100, "sun_altitude_angle": -30}, "x": "fog_density"}"#,
            r#"{"id": "rv", "labels": {"response": "応答"}, "samples": 41}"#,
        )
        .unwrap();
    assert!(html
        .starts_with("<div class=\"oo-cov oo-rules\" id=\"rv\" data-oo-quantity=\"visibility\">"));
    // A slider per input, at the value asked (or the default, or the middle).
    assert!(html.contains("data-oo-input=\"fog_density\" min=\"0\" max=\"100\""));
    assert!(html.contains("data-oo-input=\"precipitation\""));
    assert_eq!(html.matches("data-oo-input=").count(), 3);
    // Pickers: the quantity (two of them) and the input moved.
    assert!(html.contains("data-oo-rules=\"quantity\"") && html.contains("data-oo-rules=\"x\""));
    // Dense fog at night: the fog bounds it (15-40 m), the winning rule base marked.
    assert!(html.contains("<td class=\"oo-num oo-rules-value\">15</td>"));
    assert!(html.contains("<td class=\"oo-num oo-win\">15</td>"));
    // A membership chart per input of each rule base, each term a line.
    assert_eq!(html.matches("<figure class=\"oo-rules-mf\">").count(), 3);
    assert_eq!(html.matches("class=\"oo-rules-line ").count(), 4 + 4 + 2);
    // Which rules fire: fog's dense alone.
    assert!(html.contains(
        "<tr class=\"oo-on\"><td class=\"oo-num\">4</td><td><code>(fog_density IS dense)</code>"
    ));
    assert!(html.contains("応答"));
    // The response: the combined band, and each rule base dashed.
    assert!(html.contains("class=\"oo-rules-band\""));
    assert_eq!(html.matches("class=\"oo-rules-base ").count(), 3 * 2);
}

#[wasm_bindgen_test]
#[test]
fn a_query_the_rules_cannot_answer_is_refused() {
    let v = view();
    assert!(v
        .rules_html_str(r#"{"quantity": "grip"}"#, "{}")
        .unwrap_err()
        .contains("unknown quantity 'grip'"));
    assert!(v
        .rules_html_str(r#"{"quantity": "tire_friction", "x": "fog_density"}"#, "{}")
        .unwrap_err()
        .contains("reads no input 'fog_density'"));
    assert!(v
        .rules_html_str("{}", r#"{"labels": {"nope": "x"}}"#)
        .unwrap_err()
        .contains("no such label: nope"));
}

#[wasm_bindgen_test]
#[test]
fn the_data_is_the_same_as_json() {
    let v = view();
    let data: Json = serde_json::from_str(
        &v.rules_json_str(
            r#"{"quantity": "tire_friction", "inputs": {"water": 25}}"#,
            r#"{"samples": 11}"#,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(data["quantity"], "tire_friction");
    assert_eq!(data["combine"], "min");
    let base = &data["rule_bases"][0];
    let firing: Vec<f64> = base["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["firing"].as_f64().unwrap())
        .collect();
    assert_eq!(firing, vec![0.5, 0.5, 0.0]);
    assert_eq!(
        base["inputs"][0]["terms"][0]["curve"]
            .as_array()
            .unwrap()
            .len(),
        11
    );
    assert_eq!(data["response"]["xs"].as_array().unwrap().len(), 11);
    assert!((data["combined"]["low"].as_f64().unwrap() - 0.775).abs() < 1e-12);
}

#[wasm_bindgen_test]
#[test]
fn a_later_file_replaces_quantities() {
    let mut v = view();
    let grip_only = SAMPLE
        .split("\n  # How far a driver sees")
        .next()
        .unwrap()
        .replace("0.45", "0.30");
    v.merge_str(&grip_only, "measured.yaml").unwrap();
    let out: Json = serde_json::from_str(
        &v.evaluate_str("tire_friction", r#"{"water": 100}"#)
            .unwrap(),
    )
    .unwrap();
    assert!((out["low"].as_f64().unwrap() - 0.30).abs() < 1e-12);
    let q: Vec<String> = serde_json::from_str(&v.quantities()).unwrap();
    assert_eq!(q.len(), 2);
}

#[wasm_bindgen_test]
#[test]
fn the_page_assets_include_the_rules_view() {
    assert!(styles().contains(".oo-cov.oo-rules"));
    assert!(script().contains("openodd:rules"));
}
