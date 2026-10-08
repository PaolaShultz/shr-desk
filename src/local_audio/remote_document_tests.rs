use super::*;
struct CompleteDocument(Option<Vec<u8>>);
impl AuthorityConnection for CompleteDocument {
    fn send_frame_until(&mut self, _: &[u8], _: Instant) -> Result<(), String> {
        Ok(())
    }
    fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.take())
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.take())
    }
}
#[test]
fn actual_gp07_v4_48_paged_document_reaches_central_dispatch() {
    use sha2::{Digest, Sha256};
    let bytes = include_bytes!("../../tests/fixtures/gp18/v1-corrected/gp07v4-48.json");
    assert!(bytes.len() > crate::provider::MAX_BYTES);
    let hash = format!("{:x}", Sha256::digest(bytes));
    let chunks: Vec<_> = bytes.chunks(8192).collect();
    let mut assembly = crate::pages::Assembly::default();
    let mut operator = Operator::from_document_connection(
        Box::new(CompleteDocument(None)),
        "11111111-1111-4111-8111-111111111111",
        9,
        "v4-transport",
        "foh",
        2,
    )
    .unwrap();
    for (index, chunk) in chunks.iter().enumerate() {
        let page=serde_json::to_vec(&json!({"contract":"GP14-snapshot-pages","version":1,"identity":hash,"index":index,"count":chunks.len(),"total_bytes":bytes.len(),"payload":std::str::from_utf8(chunk).unwrap()})).unwrap();
        if let Some(document) = assembly.offer_document(page, Instant::now()).unwrap() {
            assert!(operator.processing_document(document).unwrap().is_none());
        }
    }
    assert_eq!(
        operator.session.processing.as_ref().unwrap().channels.len(),
        48
    );
    let old = operator.session.processing_age(operator.now()).unwrap();
    let duplicate = crate::provider::StrictDocument::parse(bytes).unwrap();
    operator.processing_document(duplicate).unwrap();
    assert!(operator.session.processing_age(operator.now()).unwrap() >= old);
}
#[test]
fn remote_complete_payload_does_not_pass_through_unix_frame_size_gate_again() {
    let corpus: Value = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/gp14/v1/profile-48.json"
    ))
    .unwrap();
    let bytes = serde_json::to_vec(&corpus["snapshot"]).unwrap();
    assert!(bytes.len() > crate::provider::MAX_BYTES);
    let snapshot = audio::decode_reply(&bytes).unwrap().snapshot.unwrap();
    let mut operator = Operator::from_document_connection(
        Box::new(CompleteDocument(Some(bytes.clone()))),
        &snapshot.authority.show_id,
        snapshot.authority.epoch.parse().unwrap(),
        "remote-doc-test",
        "foh",
        2,
    )
    .unwrap();
    // This regression protects document-vs-Unix-frame transport selection,
    // not debug schema throughput. Exercise the actual constructor's transport
    // under a deadline; keep strict decoding outside refresh's separate250ms
    // production budget (covered by the focused deadline/readback tests).
    let received = operator
        .transport
        .receive_available_until(Instant::now() + Duration::from_millis(250))
        .unwrap()
        .unwrap();
    assert_eq!(received, bytes);
    let reply = audio::decode_reply(&received).unwrap();
    operator.telemetry(&reply).unwrap();
    assert_eq!(
        operator.session.snapshot.unwrap().authority.inputs.len(),
        48
    );
}
