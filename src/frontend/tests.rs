use super::*;
#[test]
fn complete_large_review_requires_all_rendered_pages() {
    let mut f = Frontend::new(Config {
        wire_version: 1,
        remote: None,
        endpoint: "/nonexistent/desk-test.sock".into(),
        show: "11111111-1111-4111-8111-111111111111".into(),
        epoch: 9,
        writer: "review-test".into(),
        scope: "foh".into(),
    });
    let corpus: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/gp03/v1/e03-rendered.json"
    ))
    .unwrap();
    let review=(0..80).map(|i|format!("target input-{i:02} current -6000 target -3000 delta3000 show/epoch/revision/scope ")).collect::<String>();
    f.state = Some(Update {
        device: None,
        device_final: None,
        device_fresh: false,
        device_age_ms: Some(0),
        brain: None,
        brain_final: None,
        brain_fresh: false,
        brain_age_ms: None,
        held_status: None,
        held_baseline_ready: false,
        held_transport_authenticated: false,
        brain_status: "disabled".into(),
        generation: 1,
        attachment_generation: 1,
        last_operation: None,
        writer_lease_remaining_ms: None,
        snapshot: Some(
            crate::audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap())
                .unwrap(),
        ),
        fx: Default::default(),
        fx_age_ms: None,
        measurement: Default::default(),
        live_eq: None,
        live_eq_age_ms: None,
        live_eq_final: None,
        sends: None,
        sends_age_ms: None,
        sends_final: None,
        processing: None,
        processing_age_ms: None,
        processing_status: "disabled".into(),
        structural: None,
        structural_final: None,
        structural_fresh: false,
        structural_age_ms: Some(0),
        processing_final: None,
        fresh: true,
        snapshot_age_ms: Some(0),
        status: "review".into(),
        review: Some((42, review.clone())),
        received: Instant::now(),
    });
    f.synchronize_review();
    assert!(f.key("Enter").unwrap_err().contains("every displayed page"));
    let mut recovered = String::new();
    let pages = f.review_lines().len().div_ceil(32);
    for page in 0..pages {
        let s = f.scene();
        assert!(s.in_bounds());
        for p in s.primitives {
            if let Primitive::Text { y, value, .. } = p
                && (96..864).contains(&y)
            {
                recovered.push_str(&value);
            }
        }
        f.mark_presented();
        assert!(f.review_seen.contains(&page));
        if page + 1 < pages {
            f.key("PageDown").unwrap();
        }
    }
    assert_eq!(recovered, review);
    assert_eq!(f.review_seen.len(), pages);
}
#[test]
fn active_bank_and_channel_selection_stay_visible_without_detail_overlap() {
    let mut f = Frontend::new(Config {
        wire_version: 1,
        remote: None,
        endpoint: "/nonexistent/desk-bank-test.sock".into(),
        show: "11111111-1111-4111-8111-111111111111".into(),
        epoch: 9,
        writer: "bank-test".into(),
        scope: "foh".into(),
    });
    let corpus: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/gp03/v1/e03-rendered.json"
    ))
    .unwrap();
    let mut snapshot =
        crate::audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap()).unwrap();
    // Layout-only future-capacity fixture, not an accepted GP03 snapshot.
    snapshot.authority.inputs = (1..=36).map(|n| format!("input-{n:02}")).collect();
    f.state = Some(Update {
        device: None,
        device_final: None,
        device_fresh: false,
        device_age_ms: Some(0),
        brain: None,
        brain_final: None,
        brain_fresh: false,
        brain_age_ms: None,
        held_status: None,
        held_baseline_ready: false,
        held_transport_authenticated: false,
        brain_status: "disabled".into(),
        generation: 1,
        attachment_generation: 1,
        last_operation: None,
        writer_lease_remaining_ms: None,
        snapshot: Some(snapshot),
        fx: Default::default(),
        fx_age_ms: None,
        measurement: Default::default(),
        live_eq: None,
        live_eq_age_ms: None,
        live_eq_final: None,
        sends: None,
        sends_age_ms: None,
        sends_final: None,
        processing: None,
        processing_age_ms: None,
        processing_status: "disabled".into(),
        structural: None,
        structural_final: None,
        structural_fresh: false,
        structural_age_ms: Some(0),
        processing_final: None,
        fresh: true,
        snapshot_age_ms: Some(0),
        status: "layout fixture".into(),
        review: None,
        received: Instant::now(),
    });
    f.selected = 13;
    let scene = f.scene();
    assert!(scene.in_bounds());
    let rows: Vec<_> = scene
        .primitives
        .iter()
        .filter_map(|p| {
            if let Primitive::Text { y, value, .. } = p
                && (192..600).contains(y)
            {
                Some((*y, value))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(rows.len(), 12);
    assert!(rows.iter().any(|(_, s)| s.starts_with("> input-14")));
    assert!(rows.iter().all(|(y, _)| y + 24 < 600));
    f.page = Page::Channel;
    let scene = f.scene();
    assert!(
        scene
            .primitives
            .iter()
            .any(|p| matches!(p,Primitive::Text{value,..} if value.starts_with("> input-14")))
    );
}
