//! Explicit GP21 queries and reviewed writes over the existing AuthorityConnection.
use super::*;
use crate::fx as w;
impl Operator {
    pub(crate) fn refresh_fx(&mut self,explicit:bool)->Result<(),String>{
        self.check_guard()?;
        if self.session.snapshot_request().version!=2{return Err("FX requires explicit dynamic provider".into());}
        let deadline=Instant::now()+Duration::from_millis(250);let mut budget=64;
        while let Some(bytes)=self.transport.receive_available_until(deadline)? {
            if budget==0 || Instant::now()>=deadline{return Err("FX read backlog bound".into());}budget-=1;
            self.processing_document(crate::provider::StrictDocument::parse(&bytes)?)?;
        }
        let sent=self.now();let request=self.session.fx_request();
        self.transport.send_frame_until(&self.session.snapshot_request().encode()?,deadline)?;
        self.transport.send_frame_until(&request.encode()?,deadline)?;
        let mut raw=false;let mut fx=None;
        while budget>0 && Instant::now()<deadline {
            self.check_guard()?;
            let Some(d)=self.transport.receive_document_until(deadline)? else{continue;};budget-=1;
            if d.value()["contract"]==w::CONTRACT && d.value()["state"]=="snapshot" {fx=Some(w::Snapshot::decode(d.value().clone())?);}
            else if let Some(d)=self.processing_document(d)? {
                let r=audio::decode_reply_document(d)?;
                if r.context==self.session.snapshot_request().context {if let Some(s)=r.snapshot{raw=self.session.ingest_snapshot(s,sent)?;}}
                else {self.session.accept(r,self.now())?;}
            }
            if raw && let Some(s)=fx.take(){
                let r=self.session.snapshot.as_ref().ok_or("raw FX pair missing")?;
                if s.show_id!=r.authority.show_id || s.epoch!=r.authority.epoch || s.revision!=r.authority.revision{return Err("FX/raw exact context mismatch".into());}
                if let Some(old)=self.session.fx.snapshot.as_ref().and_then(|s|s.observation.as_ref()) && let Some(new)=&s.observation
                    && old.binding==new.binding && (crate::provider::counter(&new.generation)?<crate::provider::counter(&old.generation)? || crate::provider::counter(&new.reset_count)?<crate::provider::counter(&old.reset_count)? || crate::provider::counter(&new.next_source_frame)?<crate::provider::counter(&old.next_source_frame)?) {return Err("FX owner observation regression".into());}
                let reconciled=s.available && s.pending.is_none() && s.observation.as_ref().is_some_and(w::Observation::settled);
                self.session.fx.snapshot=Some(s);self.session.fx.receipt=Some(sent);
                if explicit && reconciled && self.session.fx.unknown {
                    // This is read-only reconciliation, never a reconstructed DSP completion.
                    self.session.fx.unknown=false;
                    if self.session.pending.as_ref().is_some_and(|p|p.request.kind=="fx_configure" && p.state==audio::PendingState::Uncertain){self.session.pending=None;}
                    self.session.last_result="FX fresh owner reconciled; historical operation outcome remains unknown; explicit new review required".into();
                }
                return Ok(());
            }
        }
        Err("FX read deadline/queue bound".into())
    }
    pub(crate) fn stage_fx(&mut self,basis:w::Observation,configuration_json:Option<String>,panic_mask:Option<u32>)->Result<(),String>{
        if self.draft.is_some(){return Err("confirm or cancel existing review".into());}
        let context=(self.session.snapshot.as_ref().ok_or("raw observation")?.authority.revision.clone(),self.session.generation());
        self.refresh_fx(false)?;
        let current=self.session.fx.snapshot.as_ref().and_then(|s|s.observation.as_ref()).ok_or("FX unavailable")?;
        if !basis.same_basis(current) || !self.session.fx_fresh(self.now()) || self.session.fx.unknown {return Err("FX reviewed basis stale/unavailable/unknown; explicit probe and new edit required".into());}
        let mutation=w::Mutation::from_basis(&basis,configuration_json,panic_mask);let body=w::body(&mutation)?;
        if context!=(self.session.snapshot.as_ref().unwrap().authority.revision.clone(),self.session.generation()){return Err("FX review authority changed".into());}
        self.stage("fx_configure",body)?;self.draft.as_mut().unwrap().fx_basis=Some(basis);Ok(())
    }
    pub(crate) fn mutate_fx(&mut self,body:Value)->Result<(),String>{
        let result=self.mutate_fx_once(body);
        if let Err(e)=&result && self.session.pending.as_ref().is_some_and(|p|p.request.kind=="fx_configure") {self.session.fx_unknown(e);}
        result
    }
    fn mutate_fx_once(&mut self,body:Value)->Result<(),String>{
        let deadline=Instant::now()+Duration::from_millis(1900);
        self.check_guard()?;let request=self.session.begin("fx_configure",body,self.now())?;
        self.transport.send_frame_until(&request.encode()?,deadline.min(Instant::now()+Duration::from_millis(200)))?;
        let mut budget=128;
        while budget>0 && Instant::now()<deadline {
            self.check_guard()?;
            let Some(d)=self.transport.receive_document_until(deadline)? else {continue;};budget-=1;
            if d.value()["contract"]==w::CONTRACT {
                if d.value()["state"]=="authorization_replayed" {return Err("FX authorization replay is not DSP completion".into());}
                let r=w::Reply::decode(d.value().clone())?;
                let accepted_current=self.session.dispatch_fx(r.clone())?;
                if !accepted_current || r.context != request.context { continue; }
                if matches!(r.state.as_str(),"settled"|"refused"|"cancelled"|"unknown") {
                    if r.state!="settled"{return Err(format!("FX {} {:?}",r.state,r.reason));}
                    self.refresh_fx(false).map_err(|e|format!("correlated FX settlement; fresh readback unavailable: {e}"))?;
                    if self.session.pending.is_some() || !self.session.fx_fresh(self.now()) { return Err("FX settled reply did not reconcile current pending operation".into()); }
                    let o=self.session.fx.snapshot.as_ref().and_then(|s|s.observation.as_ref()).ok_or("FX final owner missing")?;
                    let final_owner=r.observation.as_ref().ok_or("FX final evidence missing")?;
                    if !o.same_basis(final_owner) || crate::provider::counter(&o.next_source_frame)?<crate::provider::counter(&final_owner.next_source_frame)? {return Err("FX settled evidence/readback mismatch".into());}
                    return Ok(());
                }
            }else if let Some(d)=self.processing_document(d)?{let r=audio::decode_reply_document(d)?;if !self.telemetry(&r)?{self.session.accept(r,self.now())?;}}
        }
        Err("FX uncertain deadline/queue bound; no retry or DSP claim".into())
    }
}
#[cfg(test)]
mod regression {
    use super::*;
    use std::{collections::VecDeque,sync::{Arc,atomic::{AtomicBool,Ordering}}};
    struct Fixture {
        queue:VecDeque<Vec<u8>>, old:Value, stages:Vec<Value>, raw:Value, owner:Value,
        saw_current_settled:Arc<AtomicBool>,
    }
    impl AuthorityConnection for Fixture {
        fn send_frame_until(&mut self,bytes:&[u8],_:Instant)->Result<(),String>{
            let request:Value=serde_json::from_slice(bytes).unwrap();
            match request["kind"].as_str().unwrap(){
                "fx_configure"=>{
                    self.queue.push_back(serde_json::to_vec(&self.old).unwrap());
                    for v in &self.stages{self.queue.push_back(serde_json::to_vec(v).unwrap());}
                },
                "snapshot"=>self.queue.push_back(serde_json::to_vec(&self.raw).unwrap()),
                "fx_snapshot"=>self.queue.push_back(serde_json::to_vec(&self.owner).unwrap()),
                _=>panic!("unexpected request"),
            }
            Ok(())
        }
        fn receive_until(&mut self,_:Instant)->Result<Option<Vec<u8>>,String>{
            let b=self.queue.pop_front().ok_or("fixture exhausted before current completion")?;
            let v:Value=serde_json::from_slice(&b).unwrap();
            if v["context"]["request_id"]=="3" && v["state"]=="settled"{self.saw_current_settled.store(true,Ordering::Release);}
            Ok(Some(b))
        }
        fn receive_available(&mut self)->Result<Option<Vec<u8>>,String>{Ok(None)}
    }
    #[test]
    fn retained_terminal_cannot_finish_later_fx_transport_wait(){
        let c:Value=serde_json::from_str(include_str!("../../tests/fixtures/gp21/v1/owner-relay.json")).unwrap();
        for old_state in ["settled","refused","unknown"]{
            let mut session=audio::fx_test_pending();
            for k in ["preparing","permitted","applied","settled"]{session.dispatch_fx(w::Reply::decode(c[k].clone()).unwrap()).unwrap();}
            let mut old=c["settled"].clone();old["state"]=json!(old_state);if old_state!="settled"{old["reason"]=json!("historical outcome");session.fx.evidence.push(w::Reply::decode(old.clone()).unwrap());}
            let mut initial=session.snapshot.clone().unwrap();initial.authority.revision="1".into();initial.frame="5760".into();initial.clock.as_mut().unwrap().next_frame=5760;
            session.ingest_snapshot(initial.clone(),0).unwrap();session.fx.snapshot=Some(w::Snapshot::decode(c["final_snapshot"].clone()).unwrap());session.fx.receipt=Some(0);
            let mut config=w::Configuration::decode(&session.fx.snapshot.as_ref().unwrap().observation.as_ref().unwrap().owner_json).unwrap();config.channels[0].wet_gain=0.;
            let target=serde_json::to_string(&config).unwrap();let mutation=w::Mutation::from_basis(session.fx.snapshot.as_ref().unwrap().observation.as_ref().unwrap(),Some(target.clone()),None);
            let mut stages=Vec::new();
            for k in ["preparing","permitted","applied","settled"]{
                let mut r=c[k].clone();r["context"]["request_id"]=json!("3");r["context"]["expected_revision"]=json!("1");r["ticket"]=json!("2");r["apply_frame"]=json!("10560");r["revision"]=json!(if k=="preparing"{"1"}else{"2"});
                if matches!(k,"preparing"|"permitted"){r["observation"]=c["final_snapshot"]["observation"].clone();}
                else{
                    r["observation"]["owner_json"]=json!(target);r["observation"]["generation"]=json!("2");r["observation"]["applied_source_frame"]=json!("10560");
                    r["observation"]["next_source_frame"]=json!(if k=="applied"{"10560"}else{"11520"});
                    r["observation"]["settled_generation"]=json!(if k=="applied"{"1"}else{"2"});
                    r["observation"]["settled_source_frame"]=json!(if k=="applied"{"5760"}else{"11520"});
                }
                stages.push(r);
            }
            let mut owner=c["final_snapshot"].clone();owner["revision"]=json!("2");owner["frame"]=json!("11520");owner["observation"]=stages.last().unwrap()["observation"].clone();
            let mut raw:Value=serde_json::from_str(include_str!("../../tests/fixtures/gp14/v1/profile-48.json")).unwrap()["snapshot"].clone();
            initial.authority.revision="2".into();initial.authority.sequence=(crate::provider::counter(&initial.authority.sequence).unwrap()+1).to_string();initial.frame="11520".into();initial.clock.as_mut().unwrap().next_frame=11520;
            let body=serde_json::to_value(&initial).unwrap();raw["snapshot"]=body.clone();raw["context"]["epoch"]=json!("9");raw["outcome"]["epoch"]=json!("9");raw["outcome"]["body"]["revision"]=json!("2");raw["outcome"]["body"]["snapshot"]=body["authority"].clone();
            let saw=Arc::new(AtomicBool::new(false));
            let mut op=Operator::from_document_connection(Box::new(Fixture{queue:VecDeque::new(),old,stages,raw,owner,saw_current_settled:saw.clone()}),&initial.authority.show_id,9,"unused","fx_configuration",2).unwrap();op.session=session;
            op.mutate_fx(w::body(&mutation).unwrap()).unwrap();
            assert!(saw.load(Ordering::Acquire));assert!(op.session.pending.is_none());assert_eq!(op.session.fx.evidence.last().unwrap().context.request_id.as_deref(),Some("3"));assert!(op.session.fx_fresh(op.now()));
        }
    }
}
