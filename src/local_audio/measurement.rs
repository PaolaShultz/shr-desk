//! Bounded GP20 reads use the existing connection and original deadline.
use super::*;
use crate::pa_measurement::wire as w;
impl Operator {
    pub(crate) fn refresh_measurement(&mut self, id: Option<&str>) -> Result<(), String> {
        self.check_guard()?;
        if self.session.snapshot_request().version != 2 {
            return Err("measurement requires explicit dynamic provider".into());
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut budget = 64;
        while let Some(bytes) = self.transport.receive_available_until(deadline)? {
            if budget == 0 || Instant::now() >= deadline {
                return Err("measurement backlog bound".into());
            }
            budget -= 1;
            self.processing_document(crate::provider::StrictDocument::parse(&bytes)?)?;
        }
        let sent = self.now();
        let request = self.session.measurement_request(id);
        w::validate_body(&request.kind, &request.body, None)?;
        self.transport
            .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
        self.transport
            .send_frame_until(&request.encode()?, deadline)?;
        let mut raw = false;
        let mut reply = None;
        while budget > 0 && Instant::now() < deadline {
            self.check_guard()?;
            let Some(d) = self.transport.receive_document_until(deadline)? else {
                continue;
            };
            budget -= 1;
            if d.value()["contract"] == w::CONTRACT {
                let r = w::Reply::decode(&d.into_bytes()?)?;
                if r.context == request.context
                    && r.state == if id.is_some() { "result" } else { "snapshot" }
                {
                    if let Some(id) = id
                        && r.result.as_ref().is_some_and(|x| x.summary.id != id)
                    {
                        return Err("result identity mismatch".into());
                    }
                    reply = Some(r);
                } else {
                    self.session.dispatch_measurement(r)?;
                }
            } else if let Some(d) = self.processing_document(d)? {
                let r = audio::decode_reply_document(d)?;
                if r.context == self.session.snapshot_request().context {
                    if let Some(s) = r.snapshot {
                        raw = self.session.ingest_snapshot(s, sent)?;
                    }
                } else {
                    self.session.accept(r, self.now())?;
                }
            }
            if raw && let Some(r) = reply.take() {
                if self
                    .session
                    .snapshot
                    .as_ref()
                    .is_none_or(|s| s.authority.revision != r.revision)
                {
                    return Err("measurement/raw revision mismatch".into());
                }
                if let Some(reason) = r.reason {
                    return Err(format!("measurement unavailable: {reason}"));
                }
                if id.is_some() {
                    self.session.measurement.result =
                        Some(r.result.ok_or("missing measurement result")?);
                } else {
                    self.session.measurement.snapshot =
                        Some(r.snapshot.ok_or("missing measurement snapshot")?);
                    self.session.measurement.revision = Some(r.revision);
                    self.session.measurement.receipt = Some(sent);
                }
                return Ok(());
            }
        }
        Err("measurement read deadline/queue bound".into())
    }
    pub(crate) fn stage_measurement(&mut self, kind: &str, body: Value) -> Result<(), String> {
        self.refresh_measurement(None)?;
        if !self.session.measurement_fresh(self.now()) {
            return Err("measurement capability unavailable".into());
        }
        w::validate_body(kind, &body, self.session.measurement.snapshot.as_ref())?;
        let basis = self
            .session
            .measurement
            .snapshot
            .as_ref()
            .and_then(|s| s.current_basis.clone())
            .ok_or("measurement basis")?;
        self.stage(kind, body)?;
        self.draft.as_mut().unwrap().measurement_basis = Some(basis);
        Ok(())
    }
    pub(crate) fn stage_measurement_apply(&mut self, id: &str) -> Result<(), String> {
        self.refresh_measurement(Some(id))?;
        let r = self
            .session
            .measurement
            .result
            .clone()
            .ok_or("proposal result")?;
        if r.summary.state != "proposed" {
            return Err("owner did not propose a change".into());
        }
        self.refresh_measurement(None)?;
        if self
            .session
            .measurement
            .snapshot
            .as_ref()
            .and_then(|s| s.current_basis.as_ref())
            != Some(&r.basis)
        {
            return Err("proposal basis changed; acquire new measurements".into());
        }
        let cfg = r
            .candidate_configuration_json
            .ok_or("owner candidate missing")?;
        let proposal =
            crate::pa_measurement::Proposal::decode(&crate::pa_measurement::owner_document(
                r.owner_result_json
                    .as_deref()
                    .ok_or("owner proposal missing")?,
            )?)?;
        proposal.validate_candidate(
            &crate::master_eq::parse_owner(&r.basis.configuration_json)?,
            &crate::master_eq::parse_owner(&cfg)?,
        )?;
        self.stage(
            "pa_set",
            json!({"configuration_json":cfg,"program_buses":r.basis.program_buses}),
        )?;
        if self.session.structural.as_ref().is_none_or(|s| {
            !s.outputs_quiesced
                || s.pa_configuration_json.as_ref() != Some(&r.basis.configuration_json)
                || s.pa_program_buses != r.basis.program_buses
        }) {
            self.draft = None;
            return Err("muted exact PA configuration and program bus basis required".into());
        }
        self.draft.as_mut().unwrap().measurement_basis = Some(r.basis);
        Ok(())
    }
}
