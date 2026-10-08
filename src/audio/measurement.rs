//! GP20 shares the session's lease, revision and pending request slot.
use super::*;
use crate::pa_measurement::wire as w;
impl Session {
    pub fn measurement_request(&self, id: Option<&str>) -> Request {
        let mut r = self.snapshot_request();
        r.kind = if id.is_some() {
            "measurement_result"
        } else {
            "measurement_snapshot"
        }
        .into();
        r.body = id.map_or_else(|| json!({}), |id| json!({"id":id}));
        r
    }
    pub fn measurement_fresh(&self, now: u64) -> bool {
        self.fresh(now)
            && self
                .measurement
                .receipt
                .is_some_and(|t| now >= t && now - t <= 250)
            && self.measurement.snapshot.as_ref().is_some_and(|s| {
                s.available
                    && s.current_basis.as_ref().is_some_and(|b| {
                        self.snapshot.as_ref().is_some_and(|raw| {
                            self.measurement.revision.as_ref() == Some(&raw.authority.revision)
                                && raw.topology.as_ref().is_some_and(|t| {
                                    provider::counter(&b.map_revision).ok() == Some(t.map_revision)
                                })
                        })
                    })
            })
    }
    pub(crate) fn dispatch_measurement(&mut self, r: w::Reply) -> Result<(), String> {
        if r.context == self.measurement_request(None).context {
            return Ok(());
        }
        if self.measurement.final_reply.as_ref() == Some(&r) {
            return Ok(());
        }
        let p = self
            .pending
            .as_ref()
            .filter(|p| w::mutation(&p.request.kind) && p.request.context == r.context)
            .ok_or("measurement reply correlation")?;
        if r.effective_frame
            .as_deref()
            .is_some_and(|f| provider::counter(f).unwrap() <= p.observed_frame)
            || p.timing
                .as_ref()
                .is_some_and(|(f, _)| r.reason.is_none() && r.effective_frame.as_ref() != Some(f))
        {
            return Err("measurement boundary correlation".into());
        }
        if r.state == "pending" {
            let p = self.pending.as_mut().unwrap();
            p.state = PendingState::Accepted;
            p.timing = r.effective_frame.map(|f| (f, 0));
            return Ok(());
        }
        self.last_result = if let Some(reason) = &r.reason {
            format!("{} REFUSED {reason}", p.request.kind)
        } else {
            format!(
                "{} accepted at frame {}; analysis completion requires result readback",
                p.request.kind,
                r.effective_frame.as_deref().unwrap_or("?")
            )
        };
        self.measurement.final_reply = Some(r);
        self.measurement.receipt = None;
        self.pending = None;
        self.needs_snapshot = true;
        self.preview = None;
        Ok(())
    }
}
