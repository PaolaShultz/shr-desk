use serde_json::Value;
use sha2::{Digest, Sha256};
use shr_desk::eq_response;
#[test]
fn exact_accepted_owner_coefficients_and_responses_at_all_rates() {
    let directory = std::path::Path::new("tests/fixtures/pa-eq/v1");
    let sums = std::fs::read(directory.join("SHA256SUMS")).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&sums)),
        "28de42314a4d0284390568e1b99c6d7b57666367d21b59bcbd9cbd750a076ce8"
    );
    for line in std::str::from_utf8(&sums).unwrap().lines() {
        let (hash, name) = line.split_once(' ').unwrap();
        let bytes = std::fs::read(directory.join(name.trim_start_matches([' ', '*']))).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), hash);
    }
    for rate in [8000, 48000, 192000] {
        for profile in ["bell", "low_shelf", "high_shelf", "bypass"] {
            let v: Value = serde_json::from_slice(
                &std::fs::read(directory.join(format!("{rate}-{profile}.json"))).unwrap(),
            )
            .unwrap();
            for state in ["current", "target"] {
                for side in 0..2 {
                    let bank = eq_response::coefficients(&v[state][side], rate).unwrap();
                    for (n, c) in bank.iter().enumerate() {
                        for (j, x) in c.iter().enumerate() {
                            let expected = v[format!("{state}_coefficients")][side][n][j]
                                .as_f64()
                                .unwrap();
                            assert!(
                                (x - expected).abs() < 1e-12,
                                "{rate}/{profile}/{state}/{side}/{n}/{j}: {x} vs {expected}"
                            );
                        }
                    }
                    if state == "target" {
                        for point in v["target_response"][side].as_array().unwrap() {
                            let (db, phase) =
                                eq_response::response(&bank, rate, point["hz"].as_f64().unwrap())
                                    .unwrap();
                            assert!((db - point["magnitude_db"].as_f64().unwrap()).abs() < 1e-8);
                            assert!(
                                (phase - point["phase_radians"].as_f64().unwrap()).abs() < 1e-8
                            );
                        }
                    }
                }
            }
        }
    }
}
