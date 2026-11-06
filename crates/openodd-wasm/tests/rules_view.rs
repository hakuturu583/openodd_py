//! The rules view on the demo's sample: natively (`cargo test`) and as
//! WebAssembly in Node (`wasm-pack test --node crates/openodd-wasm`).

use openodd_wasm::{script, styles, RulesView};
use serde_json::Value as Json;
use wasm_bindgen_test::wasm_bindgen_test;

const SAMPLE: &str = include_str!("../../../examples/wasm-viewer/sample_rules.yaml");

fn view() -> RulesView {
    RulesView::from_text(SAMPLE, "sample_rules.yaml").expect("the sample reads")
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
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
    // The universe: the outermost points; no default in the sample.
    assert_eq!(inputs[2]["universe"], serde_json::json!([-6.0, 0.0]));
    assert!(inputs[2]["default"].is_null());
    let e = RulesView::from_text(
        &SAMPLE.replace("low:  [0.90, 0.65, 0.45]", "low:  [0.90, 0.95, 0.45]"),
        "x.yaml",
    )
    .err()
    .unwrap();
    assert!(
        e.contains("x.yaml: ") && e.contains("must not be above high"),
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
    assert!(close(out["low"].as_f64().unwrap(), 0.57));
    assert!(close(out["high"].as_f64().unwrap(), 0.77));
    assert!(close(out["median"].as_f64().unwrap(), 0.67));
    assert!(close(out["mean"].as_f64().unwrap(), 0.67));
    // The value at a quantile, and the share below a value.
    let at = v
        .quantile_value("tire_friction", r#"{"water": 70}"#, 0.25)
        .unwrap();
    assert!(close(at, 0.62));
    let below = v
        .cdf_value("tire_friction", r#"{"water": 70}"#, 0.67)
        .unwrap();
    assert!(close(below, 0.5));
    // Parts move together: the least of fog's, rain's and light's.
    let p = v
        .cdf_value(
            "visibility",
            r#"{"fog_density": 0, "precipitation": 0, "sun_altitude_angle": -20}"#,
            100.0,
        )
        .unwrap();
    assert!(close(p, 0.5));
    assert!(v
        .quantile_value("tire_friction", "{}", 1.5)
        .unwrap_err()
        .contains("p must be from 0 to 1"));
    assert!(v
        .quantile_value("tire_friction", "{}", 0.5)
        .unwrap_err()
        .contains("reads water, which was not given and has no default"));
}

#[wasm_bindgen_test]
#[test]
fn the_component_shows_the_outputs_the_distribution_the_response_and_the_parts() {
    let v = view();
    let html = v
        .rules_html_str(
            r#"{"quantity": "visibility", "inputs": {"fog_density": 100, "sun_altitude_angle": -30}, "x": "fog_density", "p": 0.5}"#,
            r#"{"id": "rv", "labels": {"response": "応答"}, "samples": 41}"#,
        )
        .unwrap();
    assert!(html
        .starts_with("<div class=\"oo-cov oo-rules\" id=\"rv\" data-oo-quantity=\"visibility\">"));
    // A slider per input (over its outermost points), and the quantile's.
    assert!(html.contains("data-oo-input=\"fog_density\" min=\"0\" max=\"100\""));
    assert!(html.contains("data-oo-input=\"sun_altitude_angle\" min=\"-6\" max=\"0\""));
    assert_eq!(html.matches("data-oo-input=").count(), 3);
    assert!(html.contains("data-oo-p min=\"0\" max=\"1\" step=\"0.01\" value=\"0.5\""));
    // Pickers: the quantity (two of them) and the input moved.
    assert!(html.contains("data-oo-rules=\"quantity\"") && html.contains("data-oo-rules=\"x\""));
    // Dense fog at night: fog bounds it (15-40 m, median 27.5) and decides.
    assert!(html.contains(
        "<tr class=\"oo-rules-total\"><th>visibility</th><td class=\"oo-num oo-rules-value\">15</td>\
         <td class=\"oo-num oo-rules-value\">40</td><td class=\"oo-num oo-rules-value\">27.5</td>"
    ));
    assert!(html.contains("<tr class=\"oo-rules-deciding\"><th><i class=\"oo-rules-key oo-rules-s0\"></i>fog <span class=\"oo-rules-tag\">decides</span></th>"));
    assert!(html.contains("<td class=\"oo-num oo-win\">27.5</td>"));
    assert_eq!(html.matches("oo-rules-tag").count(), 2); // the table's and fog's chart
                                                         // The distribution: its CDF, each part's dashed, the value at p marked.
    assert!(html.contains("<section class=\"oo-rules-dist\">"));
    assert_eq!(html.matches("class=\"oo-rules-dot ").count(), 1);
    // The response: the band, the median and the value at p, each part's
    // low and high dashed.
    assert!(html.contains("class=\"oo-rules-band\""));
    assert!(html.contains("応答"));
    assert_eq!(html.matches("class=\"oo-rules-out ").count(), 1 + 2);
    assert_eq!(html.matches("class=\"oo-rules-base ").count(), 3 + 3 * 2);
    // A chart per part: its band, median, value at p, and the points written.
    assert_eq!(html.matches("<figure class=\"oo-rules-part").count(), 3);
    assert_eq!(html.matches("class=\"oo-rules-line ").count(), 3);
    assert_eq!(html.matches("class=\"oo-rules-pline\"").count(), 3);
    assert_eq!(
        html.matches("class=\"oo-rules-pt ").count(),
        (4 + 4 + 2) * 3
    );
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
        .rules_html_str(r#"{"p": 2}"#, "{}")
        .unwrap_err()
        .contains("p: from 0 to 1"));
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
            r#"{"quantity": "tire_friction", "inputs": {"water": 25}, "p": 0.25}"#,
            r#"{"samples": 11}"#,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(data["quantity"], "tire_friction");
    assert_eq!(data["combine"], "min");
    assert_eq!(data["p"], 0.25);
    let out = &data["outputs"];
    assert!(close(out["low"].as_f64().unwrap(), 0.775));
    assert!(close(out["high"].as_f64().unwrap(), 0.975));
    assert!(close(out["at_p"].as_f64().unwrap(), 0.825));
    assert_eq!(data["deciding"], "water");
    let part = &data["parts"][0];
    assert_eq!(part["family"], "uniform");
    assert!(close(part["params"]["low"].as_f64().unwrap(), 0.775));
    assert_eq!(part["chart"]["x"], "water");
    assert_eq!(part["chart"]["xs"].as_array().unwrap().len(), 11);
    assert_eq!(part["chart"]["points"].as_array().unwrap().len(), 3);
    assert!(close(
        part["chart"]["points"][1]["low"].as_f64().unwrap(),
        0.65
    ));
    let cdf = data["distribution"]["cdf"].as_array().unwrap();
    assert_eq!(cdf.len(), 11);
    assert_eq!(cdf[0], 0.0);
    assert_eq!(cdf[10], 1.0);
    assert_eq!(data["response"]["xs"].as_array().unwrap().len(), 11);
    assert!(close(
        data["response"]["curves"]["high"][0].as_f64().unwrap(),
        1.10
    ));
}

#[wasm_bindgen_test]
#[test]
fn a_later_file_changes_only_what_it_names() {
    let mut v = view();
    // Only the part it changes: the visibility is the sample's.
    let grip_only = "version: 2\nquantities:\n  tire_friction:\n    parts:\n      water:\n        uniform:\n          low: [0.9, 0.65, 0.3]\n          high: [1.1, 0.85, 0.65]\n";
    v.merge_str(grip_only, "measured.yaml", "").unwrap();
    let out: Json = serde_json::from_str(
        &v.evaluate_str("tire_friction", r#"{"water": 100}"#)
            .unwrap(),
    )
    .unwrap();
    assert!(close(out["low"].as_f64().unwrap(), 0.30));
    let q: Vec<String> = serde_json::from_str(&v.quantities()).unwrap();
    assert_eq!(q.len(), 2);
    let html = v.rules_html_str("{}", "{}").unwrap();
    assert!(html.contains("measured.yaml"));
}

#[wasm_bindgen_test]
#[test]
fn the_page_assets_include_the_rules_view() {
    assert!(styles().contains(".oo-cov.oo-rules"));
    assert!(script().contains("openodd:rules") && script().contains("data-oo-p"));
}

#[wasm_bindgen_test]
#[test]
fn a_file_extending_others_reads_them_from_what_the_page_fetched() {
    let child = "version: 2\nextends: ../sample_rules.yaml\ninputs:\n  water: {default: 100}\nquantities:\n  tire_friction:\n    parts:\n      water:\n        uniform:\n          low: [0.95, 0.6, 0.3]\n          high: [1.05, 0.8, 0.5]\n";
    let source = "https://example.com/rules/vehicles/mini.yaml";
    let refs: Vec<String> =
        serde_json::from_str(&RulesView::references(child, source).unwrap()).unwrap();
    assert_eq!(refs, vec!["https://example.com/rules/sample_rules.yaml"]);
    assert_eq!(
        RulesView::taxonomy_reference(child, source).unwrap(),
        "null"
    );
    // A version 3 file's taxonomy is fetched apart, and not followed as a rule file.
    let v3 =
        "version: 3\nextends: ../sample_rules.yaml\ntaxonomy: ../taxonomy.yaml\nquantities: {}\n";
    let refs: Vec<String> =
        serde_json::from_str(&RulesView::references(v3, source).unwrap()).unwrap();
    assert_eq!(refs, vec!["https://example.com/rules/sample_rules.yaml"]);
    assert_eq!(
        RulesView::taxonomy_reference(v3, source).unwrap(),
        "\"https://example.com/rules/taxonomy.yaml\""
    );
    let files =
        serde_json::json!({ "https://example.com/rules/sample_rules.yaml": SAMPLE }).to_string();
    let v = RulesView::from_files(child, source, &files).unwrap();
    // The input's default: what an input not given reads, and the slider's.
    let out: Json = serde_json::from_str(&v.evaluate_str("tire_friction", "{}").unwrap()).unwrap();
    assert!(close(out["low"].as_f64().unwrap(), 0.3));
    let inputs: Json = serde_json::from_str(&v.inputs_str("tire_friction").unwrap()).unwrap();
    assert_eq!(inputs[0]["default"], 100.0);
    let html = v.rules_html_str("{}", "{}").unwrap();
    assert!(
        html.contains("data-oo-input=\"water\" min=\"0\" max=\"100\" step=\"0.5\" value=\"100\"")
    );
    // The visibility is the extended file's.
    let q: Vec<String> = serde_json::from_str(&v.quantities()).unwrap();
    assert_eq!(q, vec!["tire_friction", "visibility"]);
    let read: Json = serde_json::from_str(&v.files_str()).unwrap();
    assert_eq!(read.as_array().unwrap().len(), 2);
    // Not fetched: an error naming it.
    let e = RulesView::from_files(child, source, "{}").err().unwrap();
    assert!(
        e.contains("https://example.com/rules/sample_rules.yaml: not given"),
        "{e}"
    );
}

#[wasm_bindgen_test]
#[test]
fn a_chain_is_ordered_sampled_and_its_quantity_inputs_flagged() {
    let chain = include_str!("../../../examples/wasm-viewer/chain_rules.yaml");
    let v = RulesView::from_text(chain, "chain_rules.yaml").unwrap();
    let order: Vec<String> = serde_json::from_str(&v.order_str()).unwrap();
    assert_eq!(
        order,
        [
            "tire_friction",
            "visibility",
            "sight_speed",
            "following_gap"
        ]
    );
    let inputs: Json = serde_json::from_str(&v.inputs_str("sight_speed").unwrap()).unwrap();
    assert_eq!(inputs[0]["name"], "visibility");
    assert_eq!(inputs[0]["quantity"], true);
    let weather =
        r#"{"water": 70, "fog_density": 80, "precipitation": 30, "sun_altitude_angle": 10}"#;
    let all: Json = serde_json::from_str(&v.sample_str(weather, "").unwrap()).unwrap();
    assert!(close(all["tire_friction"].as_f64().unwrap(), 0.67));
    // The visibility at its median, and the speed read at it.
    let vis = all["visibility"].as_f64().unwrap();
    let seen = format!(r#"{{"visibility": {vis}}}"#);
    let speed = v.quantile_value("sight_speed", &seen, 0.5).unwrap();
    assert!(close(all["sight_speed"].as_f64().unwrap(), speed));
    // Each quantity at its own quantile.
    let low: Json =
        serde_json::from_str(&v.sample_str(weather, r#"{"visibility": 0}"#).unwrap()).unwrap();
    assert!(low["visibility"].as_f64().unwrap() < vis);
    // The component shows a chained quantity like any other.
    let html = v
        .rules_html_str(r#"{"quantity": "sight_speed"}"#, "{}")
        .unwrap();
    assert!(html.contains("visibility"));
}

#[wasm_bindgen_test]
#[test]
fn a_part_reading_no_input_is_drawn() {
    // A root of the network (`gap`, no input at all) and a quantity with a
    // part that reads no input beside one that does (`grip`).
    let text = "version: 2\nquantities:\n  gap:\n    parts:\n      base:\n        uniform: {low: 2, high: 30}\n  grip:\n    combine: min\n    parts:\n      floor:\n        uniform: {low: 0.8, high: 1.0}\n      water:\n        at: {water: [0, 100]}\n        uniform: {low: [0.9, 0.4], high: [1.1, 0.6]}\n";
    let v = RulesView::from_text(text, "root.yaml").expect("reads");
    let html = v.rules_html_str(r#"{"quantity": "gap"}"#, "{}").unwrap();
    assert!(html.contains("base"), "{html}");
    assert!(!html.contains("oo-rules-response"), "no input, no response");
    let data: Json =
        serde_json::from_str(&v.rules_json_str(r#"{"quantity": "gap"}"#, "{}").unwrap()).unwrap();
    assert!(data["response"]["x"].is_null());
    assert!(close(data["outputs"]["median"].as_f64().unwrap(), 16.0));
    let data: Json =
        serde_json::from_str(&v.rules_json_str(r#"{"quantity": "grip"}"#, "{}").unwrap()).unwrap();
    let floor = &data["parts"][0];
    assert_eq!(floor["chart"]["x"], "water");
    let lows: Vec<f64> = floor["chart"]["curves"]["low"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    assert!(lows.iter().all(|x| close(*x, 0.8)), "flat: {lows:?}");
    assert!(v
        .rules_html_str(r#"{"quantity": "grip"}"#, "{}")
        .unwrap()
        .contains("oo-rules-response"));
}

#[wasm_bindgen_test]
#[test]
fn a_categorical_input_is_drawn_at_its_labels() {
    let text = "version: 2\ninputs:\n  time_of_day: {labels: [day, dusk, night, dawn], default: day}\nquantities:\n  sun:\n    parts:\n      time:\n        at: {time_of_day: [day, dusk, night, dawn]}\n        uniform: {low: [10, -6, -30, -6], high: [70, 0, -6, 0]}\n  sight:\n    parts:\n      light:\n        at: {time_of_day: [day, night]}\n        uniform: {low: [250, 50], high: [400, 150]}\n";
    let v = RulesView::from_text(text, "sun.yaml").expect("reads");
    let data: Json =
        serde_json::from_str(&v.rules_json_str(r#"{"quantity": "sun"}"#, "{}").unwrap()).unwrap();
    // The response at the four labels only.
    assert_eq!(
        data["response"]["xs"],
        serde_json::json!([0.0, 1.0, 2.0, 3.0])
    );
    assert_eq!(
        data["parts"][0]["chart"]["xs"],
        serde_json::json!([0.0, 1.0, 2.0, 3.0])
    );
    // A slider between two labels reads the nearest.
    let at: Json = serde_json::from_str(
        &v.rules_json_str(
            r#"{"quantity": "sun", "inputs": {"time_of_day": 1.2}}"#,
            "{}",
        )
        .unwrap(),
    )
    .unwrap();
    assert!(close(at["outputs"]["low"].as_f64().unwrap(), -6.0));
    // A part written at two of them responds there alone.
    let sight: Json =
        serde_json::from_str(&v.rules_json_str(r#"{"quantity": "sight"}"#, "{}").unwrap()).unwrap();
    assert_eq!(sight["response"]["xs"], serde_json::json!([0.0, 2.0]));
    assert!(v.rules_html_str(r#"{"quantity": "sun"}"#, "{}").is_ok());
}

#[wasm_bindgen_test]
#[test]
fn a_categorical_slider_starts_at_a_written_label() {
    // No default, and the part skips dusk: it starts at day, and a query
    // at dusk reads the nearest written label.
    let text = "version: 2\ninputs:\n  time_of_day: {labels: [day, dusk, night]}\nquantities:\n  sight:\n    parts:\n      light:\n        at: {time_of_day: [day, night]}\n        uniform: {low: [250, 50], high: [400, 150]}\n";
    let v = RulesView::from_text(text, "sight.yaml").expect("reads");
    let data: Json =
        serde_json::from_str(&v.rules_json_str(r#"{"quantity": "sight"}"#, "{}").unwrap()).unwrap();
    assert!(close(data["inputs"][0]["value"].as_f64().unwrap(), 0.0));
    assert!(close(data["outputs"]["low"].as_f64().unwrap(), 250.0));
    assert!(v.rules_html_str(r#"{"quantity": "sight"}"#, "{}").is_ok());
}

#[wasm_bindgen_test]
#[test]
fn a_categorical_slider_starts_where_every_part_is_written() {
    // One part at day and night, another at night alone: the quantity has a
    // distribution at night only, so it starts there.
    let text = "version: 2\ninputs:\n  time_of_day: {labels: [day, dusk, night]}\nquantities:\n  sight:\n    parts:\n      light:\n        at: {time_of_day: [day, night]}\n        uniform: {low: [250, 50], high: [400, 150]}\n      glare:\n        at: {time_of_day: [night]}\n        uniform: {low: 40, high: 200}\n";
    let v = RulesView::from_text(text, "sight.yaml").expect("reads");
    let data: Json =
        serde_json::from_str(&v.rules_json_str(r#"{"quantity": "sight"}"#, "{}").unwrap()).unwrap();
    assert!(close(data["inputs"][0]["value"].as_f64().unwrap(), 2.0));
    // A query at day snaps to night, where the quantity has a distribution.
    let at: Json = serde_json::from_str(
        &v.rules_json_str(
            r#"{"quantity": "sight", "inputs": {"time_of_day": 0}}"#,
            "{}",
        )
        .unwrap(),
    )
    .unwrap();
    assert!(close(at["inputs"][0]["value"].as_f64().unwrap(), 2.0));
    assert!(v.rules_html_str(r#"{"quantity": "sight"}"#, "{}").is_ok());
}
