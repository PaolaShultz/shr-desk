//! Dedicated owner observations share ordinary authority, request IDs and pending slot.
use super::*;
use crate::fx as w;
impl Session {
    pub fn fx_request(&self)->Request{let mut r=self.snapshot_request();r.kind="fx_snapshot".into();r}
    pub fn fx_fresh(&self,now:u64)->bool{
        self.fresh(now) && self.fx.receipt.is_some_and(|t|now>=t && now-t<=250)
            && self.fx.snapshot.as_ref().is_some_and(|s|s.available && s.pending.is_none() && s.observation.as_ref().is_some_and(w::Observation::settled)
                && self.snapshot.as_ref().is_some_and(|raw|s.show_id==raw.authority.show_id && s.epoch==raw.authority.epoch && s.revision==raw.authority.revision))
    }
    pub(super) fn validate_fx_command(&self,body:&Value,now:u64)->Result<(),String>{
        if self.version!=2 || self.scope!="fx_configuration" || !self.fx_fresh(now) || self.fx.unknown {return fail("fresh explicitly reconciled FX owner and dedicated lease required");}
        let m:w::Mutation=serde_json::from_value(body.clone()).map_err(|e|e.to_string())?;m.validate_basis(self.fx.snapshot.as_ref().and_then(|s|s.observation.as_ref()).ok_or("FX owner missing")?)
    }
    pub(crate) fn dispatch_fx(&mut self,r:w::Reply)->Result<bool,String>{
        if self.fx.evidence.contains(&r){return Ok(false);}
        let p=self.pending.as_ref().filter(|p|p.request.kind=="fx_configure" && p.request.context==r.context).ok_or("FX reply correlation; unrelated pending retained")?;
        let m:w::Mutation=serde_json::from_value(p.request.body.clone()).map_err(|e|e.to_string())?;
        let frame=provider::counter(&r.apply_frame)?;
        if frame<=p.observed_frame || p.ticket.as_ref().is_some_and(|t|t!=&r.ticket) || p.timing.as_ref().is_some_and(|(f,_)|f!=&r.apply_frame){return fail("FX ticket/apply frame changed");}
        let prior=self.fx.evidence.iter().rev().find(|x|x.context==r.context);
        let rank=|s:&str|match s{"preparing"=>0,"permitted"=>1,"applied"=>2,"settled"=>3,_=>4};
        if prior.is_some_and(|x|rank(&r.state)<rank(&x.state)){return fail("FX stage regression");}
        if matches!(r.state.as_str(),"preparing"|"permitted"|"applied"|"settled") {
            let o=r.observation.as_ref().ok_or("FX missing owner evidence")?;
            if o.binding!=m.binding {return fail("FX lifetime mismatch");}
            if matches!(r.state.as_str(),"applied"|"settled") {
                let expected_generation=provider::counter(&m.expected_generation)?.checked_add(u64::from(m.configuration_json.is_some())).ok_or("FX generation exhausted")?;
                let expected_reset=provider::counter(&m.expected_reset_count)?.checked_add(u64::from(m.panic_mask.is_some())).ok_or("FX reset exhausted")?;
                let basis=self.fx.snapshot.as_ref().and_then(|s|s.observation.as_ref()).ok_or("FX retained reviewed basis")?;
                let target=m.configuration_json.as_deref().unwrap_or(&basis.owner_json);
                if provider::counter(&o.generation)?!=expected_generation || provider::counter(&o.reset_count)?!=expected_reset
                    || w::Configuration::decode(&o.owner_json)?!=w::Configuration::decode(target)?
                    || provider::counter(&o.next_source_frame)?<frame
                    || (m.panic_mask.is_some() && o.applied_source_frame!=basis.applied_source_frame)
                    || (m.configuration_json.is_some() && o.applied_source_frame!=r.apply_frame)
                    || (r.state=="settled" && (!o.settled() || (m.configuration_json.is_some() && provider::counter(&o.settled_source_frame)?!=frame.checked_add(960).ok_or("FX settlement overflow")?))) {
                    return fail("FX target/generation/reset/frame evidence mismatch");
                }
            } else {
                let basis=self.fx.snapshot.as_ref().and_then(|s|s.observation.as_ref()).ok_or("FX preparation reviewed basis missing")?;
                if !basis.same_basis(o) || o.applied_source_frame!=basis.applied_source_frame || o.settled_source_frame!=basis.settled_source_frame || provider::counter(&o.next_source_frame)?<provider::counter(&basis.next_source_frame)? {return fail("FX preparation basis changed");}
            }
        }
        let terminal=matches!(r.state.as_str(),"settled"|"refused"|"cancelled"|"unknown");
        self.last_result=format!("FX {} ticket {} apply {} authority {} (permit is authorization, settlement is not silence)",r.state,r.ticket,r.apply_frame,r.revision);
        self.fx.unknown=r.state=="unknown";
        if self.fx.evidence.len()==64{self.fx.evidence.remove(0);}
        self.fx.evidence.push(r.clone());
        if terminal {self.pending=None;self.fx.receipt=None;self.needs_snapshot=true;self.preview=None;}else{let p=self.pending.as_mut().unwrap();p.state=PendingState::Accepted;p.ticket=Some(r.ticket);p.timing=Some((r.apply_frame,0));}
        Ok(true)
    }
    pub(crate) fn fx_unknown(&mut self,reason:&str){
        self.fx.unknown=true;self.fx.receipt=None;self.last_result=format!("FX UNKNOWN: {reason}; fresh explicit probe and new review required; no replay");
        if let Some(p)=self.pending.as_mut(){p.state=PendingState::Uncertain;}
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn corpus()->Value{serde_json::from_str(include_str!("../../tests/fixtures/gp21/v1/owner-relay.json")).unwrap()}
    pub(crate) fn pending()->Session{
        let c=corpus();let v:Value=serde_json::from_str(include_str!("../../tests/fixtures/gp14/v1/profile-48.json")).unwrap();
        let mut raw=decode_reply(&serde_json::to_vec(&v["snapshot"]).unwrap()).unwrap().snapshot.unwrap();raw.authority.epoch="9".into();raw.authority.revision="0".into();raw.frame="0".into();raw.clock.as_mut().unwrap().epoch=9;raw.clock.as_mut().unwrap().next_frame=0;
        let context:Context=serde_json::from_value(c["preparing"]["context"].clone()).unwrap();
        let mut s=Session::new_version(&context.show_id,9,context.writer.as_deref().unwrap(),"fx_configuration",2).unwrap();
        s.ingest_snapshot(raw,0).unwrap();s.lease=Some(Lease{token:"1".into(),deadline:5000,renew_at:4000});s.next_id=2;s.input_released();
        s.fx.snapshot=Some(w::Snapshot::decode(c["snapshot"].clone()).unwrap());s.fx.receipt=Some(0);
        s.begin("fx_configure",c["request"]["body"].clone(),0).unwrap();s
    }
    #[test]
    fn fx_permit_is_not_success_and_later_authority_revision_is_valid(){
        let c=corpus();let mut s=pending();
        for k in ["preparing","permitted","applied"]{s.dispatch_fx(w::Reply::decode(c[k].clone()).unwrap()).unwrap();assert!(s.pending.is_some());}
        let mut v=c["settled"].clone();v["revision"]=json!("7");s.dispatch_fx(w::Reply::decode(v).unwrap()).unwrap();assert!(s.pending.is_none());assert!(s.fx.receipt.is_none());
    }
    #[test]
    fn fx_refuses_wrong_context_ticket_target_generation_reset_and_settlement(){
        let c=corpus();
        for (path,value) in [(vec!["context","request_id"],json!("9")),(vec!["observation","generation"],json!("2")),(vec!["observation","reset_count"],json!("1")),(vec!["observation","settled_source_frame"],json!("5808")),(vec!["observation","binding","owner_instance"],json!("2")),(vec!["observation","owner_json"],json!(c["snapshot"]["observation"]["owner_json"]))]{
            let mut s=pending();s.dispatch_fx(w::Reply::decode(c["preparing"].clone()).unwrap()).unwrap();let mut bad=c["settled"].clone();let mut slot=&mut bad;for key in path{slot=&mut slot[key];}*slot=value;
            assert!(s.dispatch_fx(w::Reply::decode(bad).unwrap()).is_err());assert!(s.pending.is_some());
        }
        let mut s=pending();s.dispatch_fx(w::Reply::decode(c["preparing"].clone()).unwrap()).unwrap();let mut bad=c["settled"].clone();bad["ticket"]=json!("2");assert!(s.dispatch_fx(w::Reply::decode(bad).unwrap()).is_err());assert!(s.pending.is_some());
    }
    #[test]
    fn unrelated_fx_reply_cannot_clear_ordinary_pending(){
        let mut s=pending();s.pending.as_mut().unwrap().request.kind="renew".into();let c=corpus();assert!(s.dispatch_fx(w::Reply::decode(c["settled"].clone()).unwrap()).is_err());assert_eq!(s.pending.as_ref().unwrap().request.kind,"renew");
    }
    #[test]
    fn panic_requires_reset_increment_but_keeps_prior_configuration_frame(){
        let c=corpus();let mut s=pending();let basis=s.fx.snapshot.as_ref().unwrap().observation.clone().unwrap();
        s.pending.as_mut().unwrap().request.body=w::body(&w::Mutation::from_basis(&basis,None,Some(1))).unwrap();
        let mut terminal=c["settled"].clone();terminal["observation"]=serde_json::to_value(&basis).unwrap();terminal["observation"]["reset_count"]=json!("1");terminal["observation"]["next_source_frame"]=json!("4800");
        let mut bad=terminal.clone();bad["observation"]["applied_source_frame"]=json!("48");assert!(s.dispatch_fx(w::Reply::decode(bad).unwrap()).is_err());assert!(s.pending.is_some());
        s.dispatch_fx(w::Reply::decode(terminal).unwrap()).unwrap();assert!(s.pending.is_none());assert_eq!(s.fx.evidence.last().unwrap().observation.as_ref().unwrap().applied_source_frame,"0");
    }
    #[test]
    fn fx_unknown_blocks_edits_even_if_targets_match(){
        let mut s=pending();s.fx_unknown("lost permit ACK");assert!(s.fx.unknown);assert_eq!(s.pending.as_ref().unwrap().state,PendingState::Uncertain);assert!(s.validate_fx_command(&s.pending.as_ref().unwrap().request.body,0).is_err());
    }
}
#[cfg(test)]
mod duplicate_regression {
    use super::*;
    #[test]
    fn retained_final_a_cannot_finish_pending_b(){
        let c:Value=serde_json::from_str(include_str!("../../tests/fixtures/gp21/v1/owner-relay.json")).unwrap();
        let r=w::Reply::decode(c["settled"].clone()).unwrap();
        let mut s=Session::new_version(&r.context.show_id,9,r.context.writer.as_deref().unwrap(),"fx_configuration",2).unwrap();
        s.fx.evidence.push(r.clone());
        let mut context=r.context.clone();context.request_id=Some("3".into());context.expected_revision=Some("1".into());
        s.pending=Some(Pending{monitor_device:None,request:Request{version:2,context:context.clone(),kind:"fx_configure".into(),body:c["request"]["body"].clone()},first_send:0,state:PendingState::Sent,ticket:None,timing:None,observed_frame:5760,retry:0});
        assert!(!s.dispatch_fx(r.clone()).unwrap());
        assert_ne!(r.context,context);assert_eq!(s.pending.as_ref().unwrap().request.context,context);
    }
}
