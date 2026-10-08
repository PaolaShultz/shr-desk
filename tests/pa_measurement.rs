use serde_json::Value;
use shr_desk::pa_measurement::{Measurement, Proposal};

fn measured() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/pa-measurement-v1/target-result.json"
    ))
    .unwrap()
}
fn proposed() -> Value {
    serde_json::from_str(include_str!("fixtures/pa-measurement-v1/proposal.json")).unwrap()
}
#[test]
fn owner_measurements_keep_units_uncertainty_and_complete_spectrum_visible() {
    let v = measured();
    let m = Measurement::decode(&v).unwrap();
    let lines = m.lines();
    assert_eq!(lines.len(), 7 + m.spectrum.len());
    assert!(lines.iter().any(|s| s.contains("acoustic verification")));
    assert!(
        lines
            .iter()
            .any(|s| s.contains("ms") && s.contains("samples"))
    );
    assert!(
        lines
            .iter()
            .any(|s| s.contains("Source epoch") && s.contains("PA generation"))
    );
    let mut unqualified = v;
    unqualified["proposal_eligible"] = false.into();
    unqualified["reasons"] = serde_json::json!(["ambiguous_or_reflected_arrival"]);
    assert!(
        Measurement::decode(&unqualified)
            .unwrap()
            .lines()
            .iter()
            .any(|s| s.contains("Not eligible") && s.contains("ambiguous"))
    );
}
#[test]
fn owner_result_identity_bounds_and_quality_refuse_corruption() {
    for (key, value) in [
        ("version", 2.into()),
        ("samples", 65537.into()),
        ("arrival_samples", 2049.into()),
        ("segments", 0.into()),
        ("signed_correlation", 1.5.into()),
        ("extra", true.into()),
    ] {
        let mut v = measured();
        v[key] = value;
        assert!(Measurement::decode(&v).is_err(), "{key}");
    }
    for (key, value) in [
        ("source_epoch", "01".into()),
        ("configuration_revision", "0".into()),
        ("first_frame", u64::MAX.to_string().into()),
        ("dropped_frames", "1".into()),
        ("timing_verified", false.into()),
        ("clipped_mic", true.into()),
    ] {
        let mut v = measured();
        v["capture"][key] = value;
        assert!(Measurement::decode(&v).is_err(), "{key}");
    }
    let mut v = measured();
    v["spectrum"][1] = v["spectrum"][0].clone();
    assert!(Measurement::decode(&v).is_err());
}
#[test]
fn owner_proposal_has_complete_changes_and_separate_muted_apply_rearm() {
    let p = Proposal::decode(&proposed()).unwrap();
    let lines = p.lines();
    assert_eq!(
        lines
            .iter()
            .filter(|s| s.starts_with("Output index"))
            .count(),
        p.changes.len()
    );
    assert!(lines.iter().any(|s| s.contains("Rearm is separate")));
    for (key, value) in [
        ("version", 2.into()),
        ("verification_required", false.into()),
        ("basis_configuration_revision", "2".into()),
        ("status", "applied".into()),
        ("added_latency_samples", 481.into()),
    ] {
        let mut v = proposed();
        v[key] = value;
        assert!(Proposal::decode(&v).is_err(), "{key}");
    }
    let mut unchanged = proposed();
    unchanged["status"] = "no_change".into();
    unchanged["changes"] = serde_json::json!([]);
    unchanged["added_latency_samples"] = 0.into();
    unchanged["reason"] = "already_aligned".into();
    assert!(Proposal::decode(&unchanged).unwrap().lines()[0].contains("no_change"));
}
#[test]
fn owner_candidate_rejects_unrelated_dsp_or_mismatched_change() {
    let p = Proposal::decode(&proposed()).unwrap();
    let owner: Value =
        serde_json::from_str(include_str!("fixtures/pa-measurement-v1/candidate.json")).unwrap();
    let mut candidate: Value =
        serde_json::from_str(owner["configuration_json"].as_str().unwrap()).unwrap();
    p.validate_candidate(&p.basis_configuration, &candidate)
        .unwrap();
    candidate["outputs"][0]["muted"] = true.into();
    assert!(
        p.validate_candidate(&p.basis_configuration, &candidate)
            .is_err()
    );
    assert!(
        p.validate_candidate(&serde_json::json!({}), &candidate)
            .is_err()
    );
}
#[test]
fn opaque_owner_documents_preserve_strict_envelope_and_duplicate_refusal() {
    use shr_desk::pa_measurement::{owner_document, owner_lines};
    assert!(
        owner_lines(include_str!("fixtures/pa-measurement-v1/refusal.json")).unwrap()[0]
            .contains("refused")
    );
    assert!(owner_document(r#"{"contract":"C-PA-MEASUREMENT-REFUSAL","contract":"C-PA-MEASUREMENT-REFUSAL","version":1,"status":"refused","reason":"bad"}"#).is_err());
    assert!(owner_document(&" ".repeat(65537)).is_err());
}

#[test]
fn exact_gp20_producer_corpus_and_corruptions() {
    use shr_desk::pa_measurement::wire::Reply;
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/gp20/v1/producer.json")).unwrap();
    let mut replies = 0;
    for exchange in corpus["exchanges"].as_array().unwrap() {
        for key in ["reply", "completion"] {
            if let Some(v) = exchange.get(key) {
                Reply::decode(&serde_json::to_vec(v).unwrap()).unwrap();
                replies += 1;
                let mut bad = v.clone();
                bad["version"] = serde_json::json!(2);
                assert!(Reply::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
                let mut bad = v.clone();
                bad["unexpected"] = serde_json::json!(0);
                assert!(Reply::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
            }
        }
    }
    assert!(replies >= 15);
}
#[test]
fn detached_editor_validates_units_limits_and_identifiers() {
    use shr_desk::pa_measurement::editor::{Editor, parse};
    assert!(parse(Editor::Capture, "a input-01 16 0 p1 32768 2048 100 10000").is_ok());
    for bad in [
        "a input-01 16 0 p1 1 2048 100 10000",
        "a input-01 -1 0 p1 32768 2048 100 10000",
        "a input-01 16 0 p1 32768 2049 100 10000",
        "a input-01 16 0 p1 32768 2048 20000 100",
    ] {
        assert!(parse(Editor::Capture, bad).is_err());
    }
    assert!(parse(Editor::Propose, "p a b c d").is_ok());
    assert!(parse(Editor::Propose, "p a a").is_err());
}

#[test]
fn unchanged_candidate_is_exact_basis_and_remains_nonapplyable() {
    let mut value = proposed();
    value["status"] = "no_change".into();
    value["changes"] = serde_json::json!([]);
    value["added_latency_samples"] = 0.into();
    value["reason"] = "already_aligned".into();
    let p = Proposal::decode(&value).unwrap();
    p.validate_candidate(&p.basis_configuration, &p.basis_configuration)
        .unwrap();
    let mut changed = p.basis_configuration.clone();
    changed["outputs"][0]["muted"] = true.into();
    assert!(
        p.validate_candidate(&p.basis_configuration, &changed)
            .is_err()
    );
    assert_ne!(p.status, "proposed");
}

#[test]
fn gp20_current_basis_epoch_fenced_but_historical_results_retained() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/gp20/v1/producer.json")).unwrap();
    let mut checked = false;
    for e in corpus["exchanges"].as_array().unwrap() {
        let Some(v) = e.get("reply") else {
            continue;
        };
        if !v["snapshot"]["current_basis"].is_null() {
            shr_desk::pa_measurement::wire::Reply::decode(&serde_json::to_vec(v).unwrap()).unwrap();
            let mut bad = v.clone();
            bad["snapshot"]["current_basis"]["source_epoch"] = serde_json::json!("999");
            assert!(
                shr_desk::pa_measurement::wire::Reply::decode(&serde_json::to_vec(&bad).unwrap())
                    .is_err()
            );
            checked = true;
        }
    }
    assert!(checked);
}
