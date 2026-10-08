use serde_json::{Value, json};
use shr_desk::fx::*;
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/gp21/v1/owner-relay.json")).unwrap()
}
#[test]
fn frozen_actual_owner_corpus_and_outer_integer_boundary() {
    let c = corpus();
    for k in ["snapshot", "final_snapshot"] {
        Snapshot::decode(c[k].clone()).unwrap();
    }
    for k in ["preparing", "permitted", "applied", "settled"] {
        Reply::decode(c[k].clone()).unwrap();
    }
    assert!(shr_desk::provider::StrictDocument::parse(br#"{"fraction":0.5}"#).is_err());
    let mutation: Mutation = serde_json::from_value(c["request"]["body"].clone()).unwrap();
    mutation.validate().unwrap();
    for k in ["preparing", "permitted", "applied", "settled"] {
        let mut v = c[k].clone();
        v["unexpected"] = json!(0);
        assert!(Reply::decode(v).is_err());
    }
    let mut v = c["settled"].clone();
    v["revision"] = json!("7");
    Reply::decode(v).unwrap();
}
#[test]
fn opaque_fractional_config_is_strict_and_bounded() {
    let c = corpus();
    let text = c["snapshot"]["observation"]["owner_json"].as_str().unwrap();
    Configuration::decode(text).unwrap();
    for bad in [
        text.replace("20.0", "0.0"),
        text.replace("0.25", "0.86"),
        text.replace("0.35", "1.0"),
        text.replace("0.5", "1.01"),
        text.replace("\"delay_ms\":20.0", "\"delay_ms\":20.0,\"delay_ms\":30.0"),
        text.replace("\"bypass\":false", "\"bypass\":false,\"unknown\":0"),
        text.replace("20.0", "1e999"),
        " ".repeat(4097),
    ] {
        assert!(Configuration::decode(&bad).is_err(), "{bad}");
    }
}
#[test]
fn selected_edit_preserves_partner_and_bypass_and_enforces_ranges() {
    let o: Observation =
        serde_json::from_value(corpus()["snapshot"]["observation"].clone()).unwrap();
    let before = Configuration::decode(&o.owner_json).unwrap();
    let after = Configuration::decode(&edit_channel(&o, 0, "1e2 0.8 0.9 1").unwrap()).unwrap();
    assert_eq!(before.channels[1], after.channels[1]);
    assert_eq!(after.channels[0].delay_ms, 100.0);
    assert_eq!(before.channels[0].bypass, after.channels[0].bypass);
    for t in [
        "0 0.2 0.3 0.4",
        "500.1 0.2 0.3 0.4",
        "2 NaN 0.3 0.4",
        "2 0.2 inf 0.4",
        "2 0.2 0.3 1.1",
        "2 0.2 0.3",
    ] {
        assert!(edit_channel(&o, 1, t).is_err());
    }
    for mask in 1..=3 {
        Mutation::from_basis(&o, None, Some(mask))
            .validate()
            .unwrap();
    }
    assert!(Mutation::from_basis(&o, None, Some(0)).validate().is_err());
    let mut stale = o.clone();
    stale.binding.map_generation = "2".into();
    assert!(!o.same_basis(&stale));
    stale = o.clone();
    stale.reset_count = "1".into();
    assert!(!o.same_basis(&stale));
}
#[test]
fn unavailable_owner_retains_description_but_cannot_advertise_missing_owner() {
    let mut v = corpus()["snapshot"].clone();
    v["available"] = json!(false);
    v["observation"] = Value::Null;
    let s = Snapshot::decode(v.clone()).unwrap();
    assert!(!s.available);
    v["available"] = json!(true);
    assert!(Snapshot::decode(v).is_err());
    let mut bad = corpus()["snapshot"].clone();
    bad["observation"]["binding"]["channels"] = json!(["foh-right", "foh-left"]);
    assert!(Snapshot::decode(bad).is_err());
    let mut replay = corpus()["settled"].clone();
    replay["state"] = json!("authorization_replayed");
    assert!(Reply::decode(replay).is_err());
}
