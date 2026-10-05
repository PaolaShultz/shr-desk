//! Successor checks consume only explicitly supplied producer output. No successor
//! accepted fixture is synthesized or substituted for actual provider execution.
use serde_json::Value;
use shr_desk::{audio, processing, scopes};
#[test]
fn extended_scope_wire_tags_are_canonical_and_separate() {
    assert_eq!(scopes::value("monitor1").unwrap(), "monitor1");
    assert_eq!(
        scopes::value("monitor3").unwrap(),
        serde_json::json!({"monitor":3})
    );
    assert_eq!(
        scopes::value("monitor65535").unwrap(),
        serde_json::json!({"monitor":65535})
    );
    assert_eq!(
        scopes::value("pa_configuration").unwrap(),
        "pa_configuration"
    );
    assert_eq!(scopes::value("output_routes").unwrap(), "output_routes");
    for scope in ["monitor0", "monitor03", "monitor65536", "pa", "unknown"] {
        assert!(scopes::value(scope).is_err());
    }
}
#[test]
fn gp14_supplied_producer_documents() {
    let directory = std::env::var("GP14_CORPUS")
        .unwrap_or_else(|_| format!("{}/tests/fixtures/gp14/v1", env!("CARGO_MANIFEST_DIR")));
    for count in [16, 17, 32, 48] {
        let bytes = std::fs::read(format!("{directory}/profile-{count}.json")).unwrap();
        let document: Value = serde_json::from_slice(&bytes).unwrap();
        let initial =
            audio::decode_reply(&serde_json::to_vec(&document["snapshot"]).unwrap()).unwrap();
        let snapshot = initial.snapshot.unwrap();
        assert_eq!(snapshot.authority.inputs.len(), count);
        let mut session = audio::Session::new_version(
            &snapshot.authority.show_id,
            snapshot.authority.epoch.parse().unwrap(),
            "desk-profile-codec",
            "foh",
            2,
        )
        .unwrap();
        session.ingest_snapshot(snapshot.clone(), 0).unwrap();
        let mut legacy = audio::Session::new(
            &snapshot.authority.show_id,
            snapshot.authority.epoch.parse().unwrap(),
            "desk-legacy-refusal",
            "foh",
        )
        .unwrap();
        assert!(legacy.ingest_snapshot(snapshot, 0).is_err());
        let final_reply =
            processing::decode_reply(&serde_json::to_vec(&document["final"]).unwrap()).unwrap();
        let processing = final_reply.snapshot.unwrap();
        assert_eq!(processing.channels.len(), count);
        session.ingest_processing(processing, 1).unwrap();
        let rendered =
            audio::decode_snapshot(&serde_json::to_vec(&document["rendered_after"]).unwrap())
                .unwrap();
        assert_eq!(rendered.coefficients.len(), count);
        assert!(
            rendered
                .coefficients
                .iter()
                .all(|c| c.current_nanogain.len() == 4 + rendered.authority.monitors.len())
        );
        assert_eq!(rendered.topology.as_ref().unwrap().inputs.len(), count);
        assert_eq!(
            rendered.clock.as_ref().unwrap().adat_lock,
            shr_desk::topology::LockEvidence::Unknown
        );
        println!(
            "supplied producer profile {count}: decoded exact dynamic inventory and all processing channels"
        );
    }
}

#[test]
fn gp14_supplied_structural_documents() {
    let directory = std::env::var("GP14_CORPUS")
        .unwrap_or_else(|_| format!("{}/tests/fixtures/gp14/v1", env!("CARGO_MANIFEST_DIR")));
    for count in [16, 32, 48] {
        let bytes = std::fs::read(format!("{directory}/structure-{count}.json")).unwrap();
        let snapshot = shr_desk::structure::decode_snapshot(&bytes).unwrap();
        assert_eq!(snapshot.topology.inputs.len(), count);
        assert_eq!(snapshot.pa_status.as_ref().unwrap()["version"], 2);
        assert_eq!(
            snapshot.pa_capabilities.as_ref().unwrap()["physical_io_owned"],
            0
        );
        let mut draft = shr_desk::structure::Draft::new(&snapshot, "pa_configuration", 1).unwrap();
        draft.selected = draft
            .fields
            .iter()
            .position(|p| p.ends_with("/gain_db"))
            .unwrap();
        draft.adjust(-1, &snapshot).unwrap();
        shr_desk::structure::validate_body("pa_set", &draft.body().unwrap(), Some(&snapshot))
            .unwrap();
        let mut patch = shr_desk::structure::Draft::new(&snapshot, "output_routes", 1).unwrap();
        patch.text("{\"kind\":\"pa\",\"index\":0}").unwrap();
        shr_desk::structure::validate_body("output_patch", &patch.body().unwrap(), Some(&snapshot))
            .unwrap();
        println!(
            "actual owner structural profile{count}: exact PA/patch/clock decoded; detached edit validated"
        );
    }
}
