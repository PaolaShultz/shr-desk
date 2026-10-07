//! UI adversaries derived from the retained real PA owner graph. Modified bus
//! maps below are unit-test inputs, not new producer/hardware acceptance.
use serde_json::{Value, json};
use shr_desk::structure::{Draft, Snapshot, decode_snapshot};

fn snapshot() -> Snapshot {
    let mut s = decode_snapshot(include_bytes!("fixtures/gp14/v1/structure-16.json")).unwrap();
    s.pa_program_buses = vec![0, 1];
    s
}
fn select(d: &mut Draft, suffix: &str) {
    d.selected = d.fields.iter().position(|p| p.ends_with(suffix)).unwrap();
}
#[test]
fn master_eq_linked_edits_preserve_every_unedited_owner_field() {
    let mut s = snapshot();
    let mut config: Value =
        serde_json::from_str(s.pa_configuration_json.as_ref().unwrap()).unwrap();
    config["inputs"][1]["eq"][0]["db"] = json!(3.);
    s.pa_configuration_json = Some(config.to_string());
    let mut d = Draft::new_master_eq(&s, 1).unwrap();
    let original = d.document.clone();
    select(&mut d, "/eq/0/db");
    assert!(
        d.master_eq
            .as_ref()
            .unwrap()
            .display(&d.document, &d.fields[d.selected])
            .contains("L ")
    );
    d.text("-2.5").unwrap();
    let mut expected = original;
    for side in 0..2 {
        expected["configuration"]["inputs"][side]["eq"][0]["db"] = json!(-2.5);
    }
    assert_eq!(d.document, expected);
    assert_eq!(d.body().unwrap()["program_buses"], json!([0, 1]));
    assert_eq!(
        serde_json::from_str::<Value>(d.body().unwrap()["configuration_json"].as_str().unwrap())
            .unwrap(),
        expected["configuration"]
    );
}
#[test]
fn master_eq_maps_actual_main_inputs_without_assuming_first_two_or_changing_monitors() {
    let mut s = snapshot();
    let mut config: Value =
        serde_json::from_str(s.pa_configuration_json.as_ref().unwrap()).unwrap();
    let extra = config["inputs"][0].clone();
    config["inputs"].as_array_mut().unwrap().push(extra);
    s.pa_configuration_json = Some(config.to_string());
    s.pa_program_buses = vec![2, 1, 0];
    let mut d = Draft::new_master_eq(&s, 1).unwrap();
    let original = d.document.clone();
    d.eq_view(false).unwrap();
    select(&mut d, "/geq_db/30");
    d.text("4.5").unwrap();
    assert_eq!(
        d.document["configuration"]["inputs"][0],
        original["configuration"]["inputs"][0]
    );
    for side in [1, 2] {
        assert_eq!(
            d.document["configuration"]["inputs"][side]["geq_db"][30],
            json!(4.5)
        );
    }
}
#[test]
fn master_eq_views_do_not_copy_settings_and_single_channel_is_explicit() {
    let s = snapshot();
    let mut d = Draft::new_master_eq(&s, 1).unwrap();
    let original = d.document.clone();
    d.eq_view(true).unwrap();
    d.eq_view(false).unwrap();
    assert_eq!(d.document, original);
    select(&mut d, "/geq_db/0");
    d.adjust(1, &s).unwrap();
    assert_eq!(
        d.document["configuration"]["inputs"][0]["geq_db"][0],
        json!(0.5)
    );
    assert_eq!(
        d.document["configuration"]["inputs"][1],
        original["configuration"]["inputs"][1]
    );
    d.eq_view(true).unwrap();
    d.text("-0.5").unwrap();
    assert_eq!(
        d.document["configuration"]["inputs"][1]["geq_db"][0],
        json!(-0.5)
    );
}
#[test]
fn master_eq_invalid_edits_and_import_leave_complete_draft_unchanged() {
    let s = snapshot();
    let mut d = Draft::new_master_eq(&s, 1).unwrap();
    for (path, text) in [
        ("/eq/0/db", "12.5"),
        ("/eq/0/hz", "20001"),
        ("/eq/0/q", "0"),
        ("/eq/0/slope", "1.1"),
        ("/eq/0/kind", "not-a-filter"),
        ("/eq_enabled", "0"),
    ] {
        select(&mut d, path);
        let original = d.document.clone();
        assert!(d.text(text).is_err());
        assert_eq!(d.document, original);
    }
    select(&mut d, "/eq/0/db");
    d.text("12").unwrap();
    let original = d.document.clone();
    assert!(d.adjust(1, &s).is_err());
    assert_eq!(d.document, original);
    assert!(d.import(&original.to_string()).is_err());
    assert_eq!(d.document, original);
    select(&mut d, "/eq/0/kind");
    d.text("low_shelf").unwrap();
    assert_eq!(
        d.document["configuration"]["inputs"][1]["eq"][0]["kind"],
        "low_shelf"
    );
}
#[test]
fn master_eq_refuses_missing_ambiguous_or_unsupported_owner_descriptors() {
    for buses in [vec![2, 3], vec![0, 0], vec![0], vec![0, 1, 1]] {
        let mut s = snapshot();
        s.pa_program_buses = buses;
        assert!(Draft::new_master_eq(&s, 1).is_err());
    }
    for damage in ["version", "bands", "graphic", "type", "missing"] {
        let mut s = snapshot();
        let mut c: Value = serde_json::from_str(s.pa_configuration_json.as_ref().unwrap()).unwrap();
        match damage {
            "version" => c["version"] = json!(3),
            "bands" => {
                c["inputs"][0]["eq"].as_array_mut().unwrap().pop();
            }
            "graphic" => {
                c["inputs"][1]["geq_db"].as_array_mut().unwrap().pop();
            }
            "type" => c["inputs"][0]["eq"][0]["kind"] = json!("unknown"),
            _ => {
                c["inputs"][1]
                    .as_object_mut()
                    .unwrap()
                    .remove("geq_enabled");
            }
        }
        s.pa_configuration_json = Some(c.to_string());
        assert!(Draft::new_master_eq(&s, 1).is_err(), "{damage}");
    }
}

#[test]
fn master_eq_owner_ranges_include_disabled_fields_and_sample_rate_ceiling() {
    for rate in [8000, 44100, 48000, 192000] {
        let s = snapshot();
        let mut c: Value = serde_json::from_str(s.pa_configuration_json.as_ref().unwrap()).unwrap();
        c["sample_rate"] = json!(rate);
        let mut document = json!({"configuration":c,"program_buses":[0,1]});
        let view = shr_desk::master_eq::View::new(&document).unwrap();
        let prefix = "/configuration/inputs/0/eq/0";
        for (field, min, max) in [
            ("hz", 20., (rate as f64 * 0.45).min(20000.)),
            ("db", -12., 12.),
            ("q", 0.1, 15.909),
            ("slope", 0.1, 1.),
        ] {
            let path = format!("{prefix}/{field}");
            for value in [min, max] {
                view.set(&mut document, &path, json!(value)).unwrap();
            }
            for value in [min - 0.001, max + 0.001] {
                let before = document.clone();
                assert!(view.set(&mut document, &path, json!(value)).is_err());
                assert_eq!(document, before);
            }
        }
        document["configuration"]["inputs"][1]["eq_enabled"] = json!(false);
        document["configuration"]["inputs"][1]["eq"][0]["q"] = json!(0.);
        assert!(shr_desk::master_eq::View::new(&document).is_err());
    }
}

#[test]
fn master_eq_exact_json_and_producer_dimensions_are_admitted_before_editing() {
    for damage in [
        "duplicate",
        "unknown-band",
        "unknown-input",
        "unknown-config",
        "rate",
        "block-small",
        "block-large",
        "outputs",
    ] {
        let mut s = snapshot();
        let mut c: Value = serde_json::from_str(s.pa_configuration_json.as_ref().unwrap()).unwrap();
        match damage {
            "duplicate" => {}
            "unknown-band" => c["inputs"][1]["eq"][0]["bypass"] = json!(false),
            "unknown-input" => c["inputs"][0]["unknown"] = json!(0),
            "unknown-config" => c["unknown"] = json!(0),
            "rate" => c["sample_rate"] = json!(44100),
            "block-small" => c["max_block"] = json!(47),
            "block-large" => c["max_block"] = json!(8193),
            "outputs" => {
                c["outputs"].as_array_mut().unwrap().pop();
            }
            _ => unreachable!(),
        }
        let text = c.to_string();
        s.pa_configuration_json = Some(if damage == "duplicate" {
            text.replacen("\"db\":", "\"db\":0,\"db\":", 1)
        } else {
            text
        });
        assert!(Draft::new_master_eq(&s, 1).is_err(), "{damage}");
    }
}

#[test]
fn master_eq_stereo_failure_is_atomic_and_submission_proves_preservation() {
    let s = snapshot();
    let mut d = Draft::new_master_eq(&s, 1).unwrap();
    select(&mut d, "/eq/0/db");
    d.document["configuration"]["inputs"][1]["eq"][1]["q"] = json!(0.);
    let before = d.document.clone();
    assert!(d.text("3").is_err());
    assert_eq!(d.document, before);
    assert!(d.body().is_err());
    for path in ["routing", "protection", "monitor", "map"] {
        let mut d = Draft::new_master_eq(&s, 1).unwrap();
        match path {
            "routing" => d.document["configuration"]["outputs"][0]["source"] = Value::Null,
            "protection" => d.document["configuration"]["outputs"][0]["muted"] = json!(true),
            "monitor" => d.document["configuration"]["inputs"][0]["gain_db"] = json!(2.),
            "map" => d.document["program_buses"] = json!([1, 0]),
            _ => unreachable!(),
        }
        assert!(d.body().is_err(), "{path}");
    }
}
