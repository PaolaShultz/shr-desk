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
                self.session.dispatch_fx(r.clone())?;
                if r.context != request.context { continue; }
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
