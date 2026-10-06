//! Constant-size authenticated held authority. Never a full UI snapshot.
use crate::{brain, provider};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
pub const CONTRACT: &str = "GP15-held-proof";
pub const MAX_BYTES: usize = 8192;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub session: String,
    pub epoch: String,
    pub capability: String,
    pub map: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub contract: String,
    pub version: u32,
    pub kind: String,
    pub query_id: String,
    pub show_id: String,
    pub module: String,
    pub epoch: String,
    pub authenticated_session: String,
    pub writer: String,
    pub capability_generation: String,
    pub map_generation: String,
    pub scope: String,
    pub lease: String,
    pub expected_config_digest: String,
}
impl Request {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_BYTES {
            return Err("held proof capacity".into());
        }
        Ok(bytes)
    }
    fn validate(&self) -> Result<(), String> {
        if self.contract != CONTRACT
            || self.version != 1
            || self.kind != "held_proof"
            || self.module != "audio"
            || self.scope != "talkback_destinations"
            || !provider::uuid(&self.show_id)
            || !provider::id(&self.writer)
            || !digest_valid(&self.expected_config_digest)
        {
            return Err("held proof identity/scope".into());
        }
        for n in [
            &self.query_id,
            &self.epoch,
            &self.authenticated_session,
            &self.capability_generation,
            &self.map_generation,
            &self.lease,
        ] {
            if provider::counter(n)? == 0 {
                return Err("held proof zero identity".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Brain {
    pub selection_generation: String,
    pub hold_generation_counter: String,
    pub held_generation: Option<String>,
    pub hold_deadline_ms: Option<String>,
    pub source: brain::Source,
    pub monitor_armed: bool,
    pub monitor_mute: bool,
    pub monitor_dim: bool,
    pub talkback_mute: bool,
    pub talkback_foh: bool,
    pub monitor_path_ready: bool,
    pub talkback_path_ready: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Witness {
    pub revision: String,
    pub source_frame: String,
    pub config_digest: String,
    pub lease_remaining_ms: u32,
    pub brain: Brain,
    pub foh_authorized: bool,
    pub media_authorized: bool,
    pub heartbeat_ms: u64,
    pub deadman_ms: u64,
    pub fade_frames: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub contract: String,
    pub version: u32,
    pub state: String,
    pub reason: Option<String>,
    pub context: Request,
    pub witness: Option<Witness>,
}
impl Reply {
    pub fn decode(bytes: &[u8], dimensions: [u32; 6]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("held proof capacity".into());
        }
        let value = provider::parse(bytes)?;
        provider::keys(
            &value,
            &[
                "contract", "version", "state", "reason", "context", "witness",
            ],
        )?;
        provider::keys(
            &value["context"],
            &[
                "contract",
                "version",
                "kind",
                "query_id",
                "show_id",
                "module",
                "epoch",
                "authenticated_session",
                "writer",
                "capability_generation",
                "map_generation",
                "scope",
                "lease",
                "expected_config_digest",
            ],
        )?;
        if !value["witness"].is_null() {
            provider::keys(
                &value["witness"],
                &[
                    "revision",
                    "source_frame",
                    "config_digest",
                    "lease_remaining_ms",
                    "brain",
                    "foh_authorized",
                    "media_authorized",
                    "heartbeat_ms",
                    "deadman_ms",
                    "fade_frames",
                ],
            )?;
            provider::keys(
                &value["witness"]["brain"],
                &[
                    "selection_generation",
                    "hold_generation_counter",
                    "held_generation",
                    "hold_deadline_ms",
                    "source",
                    "monitor_armed",
                    "monitor_mute",
                    "monitor_dim",
                    "talkback_mute",
                    "talkback_foh",
                    "monitor_path_ready",
                    "talkback_path_ready",
                ],
            )?;
        }
        let reply: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
        reply.context.validate()?;
        if reply.contract != CONTRACT || reply.version != 1 {
            return Err("held proof contract".into());
        }
        match (
            &reply.witness,
            reply.reason.as_deref(),
            reply.state.as_str(),
        ) {
            (
                None,
                Some(
                    "identity" | "permission" | "scope" | "lease" | "clock" | "query_id"
                    | "config_changed" | "capacity",
                ),
                "refused",
            ) => (),
            (Some(w), None, "proof") => {
                if w.config_digest != reply.context.expected_config_digest
                    || !digest_valid(&w.config_digest)
                    || !(1..=2000).contains(&w.lease_remaining_ms)
                    || w.heartbeat_ms != 50
                    || w.deadman_ms != 150
                    || w.fade_frames != 240
                    || w.brain.held_generation.is_some() != w.brain.hold_deadline_ms.is_some()
                {
                    return Err("held proof witness".into());
                }
                provider::counter(&w.revision)?;
                if !provider::counter(&w.source_frame)?.is_multiple_of(48)
                    || provider::counter(&w.brain.selection_generation)? == 0
                {
                    return Err("held proof source boundary".into());
                }
                let counter = provider::counter(&w.brain.hold_generation_counter)?;
                if let Some(g) = &w.brain.held_generation {
                    let g = provider::counter(g)?;
                    if g == 0 || g > counter {
                        return Err("held proof generation".into());
                    }
                }
                if let Some(d) = &w.brain.hold_deadline_ms
                    && provider::counter(d)? == 0
                {
                    return Err("held proof deadline".into());
                }
                if (w.media_authorized
                    && (w.brain.held_generation.is_none()
                        || w.brain.talkback_mute
                        || (w.brain.talkback_foh && !w.foh_authorized)))
                    || (w.brain.talkback_path_ready && !w.media_authorized)
                    || (w.brain.monitor_path_ready
                        && (!w.brain.monitor_armed
                            || w.brain.monitor_mute
                            || matches!(w.brain.source, brain::Source::None)))
                {
                    return Err("held proof readiness".into());
                }
                w.brain
                    .source
                    .validate(dimensions[0] as usize, dimensions[1] as usize)?;
            }
            _ => return Err("held proof reply state".into()),
        }
        Ok(reply)
    }
}
pub(crate) struct Matched {
    request: Request,
    witness: Witness,
    sent: Instant,
    generation: u64,
}
impl Matched {
    pub(crate) fn admit(
        reply: Reply,
        expected: &Request,
        sent: Instant,
        generation: u64,
    ) -> Result<Self, String> {
        if &reply.context != expected {
            return Err("held proof correlation".into());
        }
        let witness = reply
            .witness
            .ok_or_else(|| format!("held proof refused: {:?}", reply.reason))?;
        Ok(Self {
            request: expected.clone(),
            witness,
            sent,
            generation,
        })
    }
    pub(crate) fn witness(&self) -> &Witness {
        &self.witness
    }
    pub(crate) fn request(&self) -> &Request {
        &self.request
    }
    pub(crate) fn sent(&self) -> Instant {
        self.sent
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn fresh(&self) -> bool {
        self.sent.elapsed() <= Duration::from_millis(30)
            && self.sent.elapsed()
                < Duration::from_millis(u64::from(self.witness.lease_remaining_ms))
    }
}
#[derive(Clone, Debug)]
pub struct Status {
    pub generation: Option<String>,
    pub source_frame: String,
    pub revision: String,
    pub talkback_path_ready: bool,
    pub media_authorized: bool,
    pub observed: Instant,
    pub valid_until: Instant,
}
impl Status {
    pub fn fresh(&self) -> bool {
        self.observed.elapsed() <= Duration::from_millis(50) && Instant::now() < self.valid_until
    }
}
pub fn digest_valid(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn configuration_digest(
    show: &str,
    identity: &Identity,
    dimensions: [u32; 6],
    state: &brain::Snapshot,
) -> Result<String, String> {
    let mut destinations = state.talkback_monitors.clone();
    destinations.sort_unstable();
    if destinations.windows(2).any(|v| v[0] == v[1]) {
        return Err("held proof duplicate destination".into());
    }
    let mut d = Sha256::new();
    d.update(b"GP15-HELD-PROOF/TB-DESTINATIONS/v1\0");
    d.update(
        u32::try_from(destinations.len())
            .map_err(|_| "held proof dimensions")?
            .to_be_bytes(),
    );
    for n in destinations {
        d.update(
            u32::try_from(n)
                .map_err(|_| "held proof index")?
                .to_be_bytes(),
        );
    }
    let mut h = Sha256::new();
    h.update(b"GP15-HELD-PROOF/TB-CONFIG/v1\0");
    h.update(
        u32::try_from(show.len())
            .map_err(|_| "held proof show")?
            .to_be_bytes(),
    );
    h.update(show.as_bytes());
    for n in [&identity.epoch, &identity.capability, &identity.map] {
        h.update(provider::counter(n)?.to_be_bytes());
    }
    for n in dimensions {
        h.update(n.to_be_bytes());
    }
    h.update(d.finalize());
    h.update(state.talkback_gain_cdb.to_be_bytes());
    h.update([u8::from(state.talkback_mute), u8::from(state.talkback_foh)]);
    Ok(format!("{:x}", h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/held-proof-v1")
                .join(format!("{name}.json")),
        )
        .expect("actual producer fixture must be installed")
    }
    #[test]
    fn actual_producer_held_renewal_foh_and_refusal_corpus_is_strict() {
        for name in [
            "held",
            "renewed",
            "configured-source-boundary",
            "unrelated-monitor-change",
            "foh-authorized",
            "foh-expired-independent",
            "foh-config-changed",
            "capability",
            "clock",
            "configuration",
            "duplicate",
            "epoch",
            "expired",
            "map",
            "permission",
            "session",
            "show",
        ] {
            let bytes = fixture(name);
            let reply = Reply::decode(&bytes, [16, 5, 0, 16, 18, 48000])
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(bytes.len() <= MAX_BYTES);
            if let Some(w) = reply.witness {
                assert_eq!(w.config_digest, reply.context.expected_config_digest);
            } else {
                assert_eq!(reply.state, "refused");
            }
        }
        // This consumer never originates the unsupported scope and cannot use its refusal as authority.
        assert!(Reply::decode(&fixture("scope"), [16, 5, 0, 16, 18, 48000]).is_err());
    }
    #[test]
    fn actual_producer_baselines_recompute_exact_digest_and_proofs() {
        for inputs in [16, 32, 48] {
            let raw =
                crate::audio::decode_snapshot(&fixture(&format!("baseline-raw-{inputs}"))).unwrap();
            let state: brain::Snapshot =
                serde_json::from_slice(&fixture(&format!("baseline-brain-{inputs}"))).unwrap();
            state
                .validate(raw.authority.inputs.len(), raw.authority.monitors.len())
                .unwrap();
            let request: Request =
                serde_json::from_slice(&fixture(&format!("request-{inputs}"))).unwrap();
            request.encode().unwrap();
            let t = raw.topology.as_ref().unwrap();
            let dims = [
                raw.authority.inputs.len() as u32,
                raw.authority.monitors.len() as u32,
                t.pa_outputs as u32,
                t.capture_channels as u32,
                t.playback_channels as u32,
                t.sample_rate,
            ];
            let identity = Identity {
                session: request.authenticated_session.clone(),
                epoch: request.epoch.clone(),
                capability: request.capability_generation.clone(),
                map: request.map_generation.clone(),
            };
            assert_eq!(
                configuration_digest(&raw.authority.show_id, &identity, dims, &state).unwrap(),
                request.expected_config_digest
            );
            let bytes = fixture(&format!("proof-{inputs}"));
            let reply = Reply::decode(&bytes, dims).unwrap();
            assert_eq!(reply.context, request);
            assert_eq!(reply.witness.as_ref().unwrap().lease_remaining_ms, 1999);
            let mut changed = state.clone();
            changed.monitor_mute = !changed.monitor_mute;
            assert_eq!(
                configuration_digest(&raw.authority.show_id, &identity, dims, &changed).unwrap(),
                request.expected_config_digest
            );
            changed.talkback_foh = !changed.talkback_foh;
            assert_ne!(
                configuration_digest(&raw.authority.show_id, &identity, dims, &changed).unwrap(),
                request.expected_config_digest
            );
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            for key in ["reason", "witness"] {
                let mut bad = value.clone();
                bad.as_object_mut().unwrap().remove(key);
                assert!(Reply::decode(&serde_json::to_vec(&bad).unwrap(), dims).is_err());
            }
            for key in ["held_generation", "hold_deadline_ms"] {
                let mut bad = value.clone();
                bad["witness"]["brain"].as_object_mut().unwrap().remove(key);
                assert!(Reply::decode(&serde_json::to_vec(&bad).unwrap(), dims).is_err());
            }
            for (key, bad_value) in [
                ("unknown", serde_json::json!(1)),
                ("version", serde_json::json!(1.0)),
            ] {
                let mut bad = value.clone();
                bad[key] = bad_value;
                assert!(Reply::decode(&serde_json::to_vec(&bad).unwrap(), dims).is_err());
            }
            assert!(Reply::decode(&[b' '; MAX_BYTES + 1], dims).is_err());
            let text = std::str::from_utf8(&bytes).unwrap();
            let duplicate = text.replacen("{", "{\"version\":1,", 1);
            assert!(Reply::decode(duplicate.as_bytes(), dims).is_err());
            let mut deep = value.clone();
            let mut leaf = serde_json::json!(0);
            for _ in 0..40 {
                leaf = serde_json::json!([leaf]);
            }
            deep["witness"]["revision"] = leaf;
            assert!(Reply::decode(&serde_json::to_vec(&deep).unwrap(), dims).is_err());

            let mut mismatch = reply;
            mismatch.context.query_id = "999".into();
            assert!(Matched::admit(mismatch, &request, Instant::now(), 0).is_err());
        }
    }
}
