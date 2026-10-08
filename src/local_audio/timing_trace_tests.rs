use super::*;
#[test]
fn trace_tail_and_cumulative_counters_are_bounded_and_overflow_is_distinct() {
    let mut trace = ProbeTiming::default();
    for n in 0..64 {
        trace.record(ProbeFrameTiming {
            bytes: n,
            micros: [1, 2, 3, 4],
            ..Default::default()
        });
    }
    assert_eq!(trace.frames, 64);
    assert_eq!(trace.overwritten, 60);
    assert_eq!(trace.totals, [64, 128, 192, 256]);
    assert_eq!(trace.tail.map(|f| f.bytes), [60, 61, 62, 63]);
    assert!(!trace.overflow);
    trace.totals[0] = u64::MAX;
    trace.record(ProbeFrameTiming {
        micros: [1, 0, 0, 0],
        ..Default::default()
    });
    assert_eq!(trace.overwritten, 61);
    assert!(trace.overflow);
    assert_eq!(trace.totals[0], u64::MAX);
}
