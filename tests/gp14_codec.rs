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

#[test]
fn owner_json_routes_enums_null_and_full_import_remain_detached() {
    use shr_desk::structure::{Draft, decode_snapshot, validate_body};
    let snapshot = decode_snapshot(include_bytes!("fixtures/gp14/v1/structure-16.json")).unwrap();
    let mut draft = Draft::new(&snapshot, "pa_configuration", 7).unwrap();
    let original = draft.document.clone();
    for (path, text) in [
        ("/configuration/outputs/0/source", r#"{"node":0}"#),
        (
            "/configuration/nodes",
            r#"[{"routes":[{"source":{"input":0},"weight":0.5},{"source":{"input":1},"weight":-0.25}]}]"#,
        ),
        ("/configuration/inputs/0/eq/0/kind", r#""high_shelf""#),
        ("/program_buses", "[0,2]"),
    ] {
        draft.selected = draft.fields.iter().position(|p| p == path).unwrap();
        draft.text(text).unwrap();
        assert_eq!(
            draft.document.pointer(path).unwrap(),
            &serde_json::from_str::<Value>(text).unwrap()
        );
    }
    // Desk transports exact owner route intent; only the real owner admits DSP topology.
    validate_body("pa_set", &draft.body().unwrap(), Some(&snapshot)).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(snapshot.pa_configuration_json.as_ref().unwrap()).unwrap(),
        original["configuration"]
    );
    let document = serde_json::to_string(&draft.document).unwrap();
    let mut imported = Draft::new(&snapshot, "pa_configuration", 7).unwrap();
    imported.import(&document).unwrap();
    assert_eq!(imported.body().unwrap(), draft.body().unwrap());
    let saved = imported.document.clone();
    assert!(
        imported
            .import(r#"{"configuration":{},"program_buses":[],"unexpected":1}"#)
            .is_err()
    );
    assert_eq!(saved, imported.document);
    let mut patch = Draft::new(&snapshot, "output_routes", 7).unwrap();
    patch.text("null").unwrap();
    validate_body("output_patch", &patch.body().unwrap(), Some(&snapshot)).unwrap();
    assert!(patch.document["outputs"][0]["source"].is_null());
}

#[test]
fn explicit_import_admits_regular_files_and_refuses_symlinks_and_fifo_without_blocking() {
    use std::{
        fs,
        os::unix::fs::{OpenOptionsExt, symlink},
        sync::mpsc,
        time::Duration,
    };
    let dir = std::env::temp_dir().join(format!("desk-import-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let regular = dir.join("owner.json");
    fs::write(&regular, "{}").unwrap();
    assert_eq!(shr_desk::structure::read_import(&regular).unwrap(), "{}");
    let link = dir.join("link.json");
    symlink(&regular, &link).unwrap();
    assert!(shr_desk::structure::read_import(&link).is_err());
    assert!(shr_desk::structure::read_import(&dir).is_err());
    let fifo = dir.join("fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let (tx, rx) = mpsc::channel();
    let path = fifo.clone();
    let child = std::thread::spawn(move || {
        tx.send(shr_desk::structure::read_import(&path)).unwrap();
    });
    let result = rx.recv_timeout(Duration::from_secs(1));
    if result.is_err() {
        // Unblock a regressed blocking open before joining/returning the failure.
        let _writer = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .unwrap();
        let _ = rx.recv_timeout(Duration::from_secs(1));
    }
    child.join().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(result.unwrap().is_err());
}

#[test]
fn final_producer_reference_labels_and_unassigned_ports_preserve_independent_counts() {
    use shr_desk::{structure, topology::Topology};
    for count in [16, 32, 48] {
        let path = format!(
            "{}/tests/fixtures/gp14/final/structure-{count}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let snapshot = structure::decode_snapshot(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(snapshot.topology.inputs.len(), count);
    }
    for (pa, monitors) in [(6, 12), (8, 10)] {
        let path = format!(
            "{}/tests/fixtures/gp14/final/reference-unpatched-{pa}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let topology: Topology = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        topology.validate().unwrap();
        assert_eq!(topology.inputs.len(), 16);
        assert_eq!(topology.outputs.len(), 18);
        assert_eq!(topology.capture_channels, 18);
        assert_eq!(topology.playback_channels, 20);
        assert_eq!(topology.monitors, monitors);
        assert_eq!(topology.pa_outputs, pa);
        assert!(topology.outputs.iter().all(|o| o.source.is_none()));
        assert!(topology.outputs[0].physical_port.contains("main"));
        assert!(topology.outputs[2].physical_port.contains("line"));
    }
}
