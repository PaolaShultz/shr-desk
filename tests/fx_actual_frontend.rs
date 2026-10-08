//! Explicit private TLS software episode; caller owns ready provider/Brain and sample witness.
use serde_json::{Value,json};
use shr_desk::{actions::Action,frontend::{Config,Event,Frontend},fx::{self,Observation}};
use std::{fs,path::PathBuf,thread,time::{Duration,Instant}};
fn wait(f:&mut Frontend,end:Instant,label:&str,ready:impl Fn(&Frontend)->bool){
    eprintln!("WAIT {label}");let end=end.min(Instant::now()+Duration::from_secs(8));
    loop{f.pump();if ready(f){return;}assert!(Instant::now()<end,"{label}: {} / {:?}",f.message,f.state.as_ref().map(|s|(&s.status,&s.last_operation)));thread::sleep(Duration::from_millis(5));}
}
fn tap(f:&mut Frontend,key:&str){for pressed in [true,false]{f.enqueue(Event::Key{key:key.into(),pressed}).unwrap();f.pump();}}
fn observation(f:&Frontend)->Observation{f.state.as_ref().unwrap().fx.snapshot.as_ref().unwrap().observation.clone().unwrap()}
fn probe(f:&mut Frontend,end:Instant){let old=f.state.as_ref().and_then(|s|s.fx.receipt);tap(f,"w");wait(f,end,"explicit owner probe",|f|f.state.as_ref().is_some_and(|s|s.fx.receipt!=old && s.fx_is_fresh()));}
fn confirm(f:&mut Frontend,end:Instant,root:&std::path::Path,label:&str){
    wait(f,end,"material review",|f|f.state.as_ref().is_some_and(|s|s.review.is_some()));f.synchronize_review();
    let before=observation(f);
    for n in 0..f.review_pages(){if n>0{tap(f,"PageDown");}let scene=f.scene();assert!(scene.in_bounds());fs::write(root.join(format!("{label}-review-{n}.svg")),shr_desk::render::svg(&scene)).unwrap();f.mark_presented();}
    assert!(observation(f).same_basis(&before));tap(f,"Enter");
    wait(f,end,"correlated owner settlement and fresh readback",|f|f.state.as_ref().is_some_and(|s|s.review.is_none() && s.last_operation.as_ref().is_some_and(|r|r.starts_with("FX settled")) && s.fx_is_fresh()));
    let s=f.state.as_ref().unwrap();let last=s.fx.evidence.last().unwrap();assert_eq!(last.state,"settled");assert!(s.fx.evidence.iter().any(|r|r.context==last.context && r.ticket==last.ticket && r.state=="preparing"));assert!(s.fx.evidence.iter().any(|r|r.context==last.context && r.ticket==last.ticket && r.state=="permitted"));
    assert!(last.observation.as_ref().unwrap().same_basis(&observation(f)));
    fs::write(root.join(format!("{label}-desk.json")),serde_json::to_vec_pretty(&json!({"evidence":s.fx.evidence.iter().map(|r|json!({"state":r.state,"ticket":r.ticket,"context":r.context,"apply_frame":r.apply_frame,"revision":r.revision,"observation":r.observation})).collect::<Vec<_>>(),"fresh":s.fx_is_fresh(),"snapshot":s.fx.snapshot.as_ref().map(|s|json!({"show":s.show_id,"epoch":s.epoch,"revision":s.revision,"frame":s.frame,"available":s.available,"owner":s.observation}))})).unwrap()).unwrap();
}
#[test]
#[ignore="requires explicitly launched hash-pinned GP21 provider and Brain, fresh readiness and private TLS"]
fn actual_fx_keyboard_review_owner_readback(){
    let root=PathBuf::from(std::env::var("GP21_EPISODE_DIR").unwrap());let c:Value=serde_json::from_slice(&fs::read(root.join("desk-config.json")).unwrap()).unwrap();let cfg=Config{wire_version:2,remote:Some(serde_json::from_value(c["remote"].clone()).unwrap()),endpoint:PathBuf::new(),show:c["show"].as_str().unwrap().into(),epoch:c["epoch"].as_u64().unwrap(),writer:"actual-fx-desk".into(),scope:"fx_configuration".into()};let end=Instant::now()+Duration::from_secs(55);
    let mut f=Frontend::new(cfg);wait(&mut f,end,"attach read-only",Frontend::fresh);assert!(f.state.as_ref().unwrap().fx.snapshot.is_none());probe(&mut f,end);
    let initial=observation(&f);tap(&mut f,"g");wait(&mut f,end,"explicit FX grant",|f|f.state.as_ref().is_some_and(|s|s.writer_granted()));probe(&mut f,end);
    tap(&mut f,"l");tap(&mut f,"e");for c in "2 0.2 0.3 0.4".chars(){tap(&mut f,&c.to_string());}tap(&mut f,"Enter");confirm(&mut f,end,&root,"left-config");
    let changed=observation(&f);let before=fx::Configuration::decode(&initial.owner_json).unwrap();let after=fx::Configuration::decode(&changed.owner_json).unwrap();assert_eq!(after.channels[0].delay_ms,2.0);assert_eq!(after.channels[1],before.channels[1]);assert_eq!(after.channels[0].feedback,0.2);
    probe(&mut f,end);tap(&mut f,"e");for c in "2 0.2 0.3 0".chars(){tap(&mut f,&c.to_string());}tap(&mut f,"Enter");confirm(&mut f,end,&root,"left-zero-wet");
    let zero=observation(&f);let until=zero.settled_source_frame.parse::<u64>().unwrap()+24000;
    wait(&mut f,end,"streamed sample window",|f|observation(f).next_source_frame.parse::<u64>().unwrap()>=until);
    fs::write(root.join("left-zero-window.json"),serde_json::to_vec_pretty(&json!({"owner":zero,"end_owner":observation(&f),"minimum_source_frame":until})).unwrap()).unwrap();
    probe(&mut f,end);tap(&mut f," ");confirm(&mut f,end,&root,"left-bypass");let bypassed=observation(&f);let c=fx::Configuration::decode(&bypassed.owner_json).unwrap();assert!(c.channels[0].bypass);assert_eq!(c.channels[1],before.channels[1]);
    probe(&mut f,end);tap(&mut f,"p");confirm(&mut f,end,&root,"left-panic");let panic=observation(&f);assert_eq!(panic.reset_count.parse::<u64>().unwrap(),bypassed.reset_count.parse::<u64>().unwrap()+1);assert_eq!(panic.generation,bypassed.generation);assert_eq!(fx::Configuration::decode(&panic.owner_json).unwrap(),c);
    probe(&mut f,end);tap(&mut f,"b");confirm(&mut f,end,&root,"both-panic");
    probe(&mut f,end);tap(&mut f,"e");for c in "3 0.1 0.2 0.3".chars(){tap(&mut f,&c.to_string());}tap(&mut f,"Enter");wait(&mut f,end,"cancelable unsent review",|f|f.state.as_ref().is_some_and(|s|s.review.is_some()));let unchanged=observation(&f);tap(&mut f,"Esc");wait(&mut f,end,"unsent review cancelled",|f|f.state.as_ref().is_some_and(|s|s.review.is_none()));assert!(unchanged.same_basis(&observation(&f)));
    f.inject_controller(Action::Fx(fx::Action::Edit)).unwrap();f.pump();f.enqueue(Event::Focus(false)).unwrap();f.pump();assert!(f.fx_ui.basis.is_none());
    f.enqueue(Event::Focus(true)).unwrap();f.pump();tap(&mut f,"Esc");tap(&mut f,"F5");wait(&mut f,end,"read-only reconnect",|f|f.fresh() && f.state.as_ref().is_some_and(|s|!s.writer_granted() && s.fx.snapshot.is_none()));
    let scene=f.scene();assert!(scene.in_bounds());fs::write(root.join("fx-page.svg"),shr_desk::render::svg(&scene)).unwrap();
    fs::write(root.join("desk-acceptance.json"),serde_json::to_vec_pretty(&json!({"physical":false,"selected_channel":true,"bypass":true,"selected_and_both_panic":true,"cancel_unsent":true,"focus_fence":true,"read_only_reconnect":true,"initial":initial,"after_config":changed,"after_bypass":bypassed,"after_panic":panic,"sample_witness":"independent runner reports required; this test alone proves controls/readback"})).unwrap()).unwrap();
}
