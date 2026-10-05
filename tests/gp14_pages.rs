//! Transport adversaries, not accepted producer fixtures.
use serde_json::json;
use sha2::{Digest, Sha256};
use shr_desk::pages::Assembly;
use std::time::{Duration, Instant};
fn frames(bytes: &[u8]) -> Vec<Vec<u8>> {
    let identity = format!("{:x}", Sha256::digest(bytes));
    let chunks: Vec<_> = bytes.chunks(8192).collect();
    chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| {
            serde_json::to_vec(&json!({
        "contract":"GP14-snapshot-pages","version":1,"identity":identity,"index":index,
        "count":chunks.len(),"total_bytes":bytes.len(),"payload":std::str::from_utf8(chunk).unwrap()
    })).unwrap()
        })
        .collect()
}
#[test]
fn immutable_document_requires_complete_order_hash_and_deadline() {
    let document = serde_json::to_vec(&json!({"transport_test":"x".repeat(70000)})).unwrap();
    let pages = frames(&document);
    let now = Instant::now();
    let mut assembly = Assembly::default();
    for page in &pages[..pages.len() - 1] {
        assert!(assembly.offer(page.clone(), now).unwrap().is_none());
    }
    assert_eq!(
        assembly
            .offer(pages.last().unwrap().clone(), now)
            .unwrap()
            .unwrap(),
        document
    );
    for malformed in ["duplicate", "reorder", "mixed", "expired", "corrupt"] {
        let mut assembly = Assembly::default();
        assembly.offer(pages[0].clone(), now).unwrap();
        let mut page: serde_json::Value = serde_json::from_slice(&pages[1]).unwrap();
        let time = if malformed == "expired" {
            now + Duration::from_millis(2001)
        } else {
            now
        };
        match malformed {
            "duplicate" => page["index"] = json!(0),
            "reorder" => page["index"] = json!(2),
            "mixed" => page["identity"] = json!("a".repeat(64)),
            "corrupt" => page["payload"] = json!("z".repeat(8192)),
            _ => (),
        }
        let result = assembly.offer(serde_json::to_vec(&page).unwrap(), time);
        if malformed == "corrupt" {
            assert!(result.unwrap().is_none());
            for p in &pages[2..pages.len() - 1] {
                assembly.offer(p.clone(), now).unwrap();
            }
            assert!(assembly.offer(pages.last().unwrap().clone(), now).is_err());
        } else {
            assert!(result.is_err(), "{malformed}");
        }
        // Rejected assembly cannot poison a subsequent independently complete frame.
        assert_eq!(
            assembly.offer(b"{}".to_vec(), now).unwrap(),
            Some(b"{}".to_vec())
        );
    }
}
#[test]
fn incomplete_interleaved_and_oversized_documents_are_not_observations() {
    let bytes = vec![b'x'; 70000];
    let pages = frames(&bytes);
    let now = Instant::now();
    let mut assembly = Assembly::default();
    assert!(assembly.offer(pages[1].clone(), now).is_err());
    assembly.offer(pages[0].clone(), now).unwrap();
    assert!(assembly.offer(b"{}".to_vec(), now).is_err());
    let mut page: serde_json::Value = serde_json::from_slice(&pages[0]).unwrap();
    page["total_bytes"] = json!(1048577);
    assert!(
        assembly
            .offer(serde_json::to_vec(&page).unwrap(), now)
            .is_err()
    );
    page["total_bytes"] = json!(70000);
    page["count"] = json!(usize::MAX);
    assert!(
        assembly
            .offer(serde_json::to_vec(&page).unwrap(), now)
            .is_err()
    );
}
