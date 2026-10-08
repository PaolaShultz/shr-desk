//! Bounded batch command entry; all transaction authority remains in Operator.
use super::*;
use std::io::Read;

pub fn run(args: &[String]) -> Result<(), String> {
    if args.len() != 5 && args.len() != 7 {
        return Err(
            "usage: --audio-local ENDPOINT SHOW EPOCH WRITER SCOPE [--script FILE|-]".into(),
        );
    }
    let mut commands = String::new();
    if args.len() == 7 {
        if args[5] != "--script" {
            return Err("expected --script".into());
        }
        if args[6] == "-" {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut bytes = Vec::new();
            let mut input = std::io::stdin().lock();
            loop {
                if Instant::now() >= deadline {
                    return Err("stdin batch EOF deadline10s; no connection/grant made".into());
                }
                let mut ready = PollFd {
                    fd: 0,
                    events: 1,
                    revents: 0,
                };
                let result = unsafe { poll(&mut ready, 1, 100) };
                if result < 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                if result == 0 {
                    continue;
                }
                let mut chunk = [0u8; 1024];
                let n = input.read(&mut chunk).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..n]);
                if bytes.len() > 65536 {
                    return Err("stdin batch exceeds64KiB".into());
                }
            }
            commands = String::from_utf8(bytes).map_err(|_| "stdin UTF8")?;
        } else {
            if !std::fs::metadata(&args[6])
                .map_err(|e| e.to_string())?
                .is_file()
            {
                return Err("script must be regular file".into());
            }
            std::fs::File::open(&args[6])
                .map_err(|e| e.to_string())?
                .take(65537)
                .read_to_string(&mut commands)
                .map_err(|e| e.to_string())?;
        }
    }
    if commands.len() > 65536 || commands.lines().count() > 256 {
        return Err("script exceeds64KiB/256commands".into());
    }
    let mut op = Operator {
        paired_nonce: 0,
        paired_enabled: false,
        maintenance_replies: std::collections::VecDeque::new(),
        transport: Box::new(Transport::connect(Path::new(&args[0]))?),
        session: Session::new(
            &args[1],
            args[2].parse().map_err(|_| "epoch")?,
            &args[3],
            &args[4],
        )?,
        start: Instant::now(),
        draft: None,
        scope: args[4].clone(),
        guard: None,
        brain_signal: None,
        brain_probes: BrainProbes::default(),
        held_baseline: None,
        held_query: None,
        held_matched: None,
        held_reuse: None,
        held_nonce: 0,
        held_highwater: 0,
        held_observation: None,
        held_refusal: None,
        last_brain_send: None,
        trace_timing: trace_timing_enabled(),
    };
    op.refresh()?;
    let session_deadline = Instant::now() + Duration::from_secs(30);
    for line in commands.lines() {
        if Instant::now() >= session_deadline {
            return Err("script30sdeadline".into());
        }
        let w: Vec<_> = line.split_whitespace().collect();
        match w.as_slice(){
            []=>{},["status"]=>{op.refresh()?;println!("{}",op.status());},
            ["snapshot"]=>op.refresh()?,
            ["wait",ms]=>op.wait(ms.parse().map_err(|_|"wait milliseconds")?)?,
            ["grant"]=>op.mutate("grant",json!({"scope":args[4]}))?,
            ["input-release"]=>op.session.input_released(),
            ["renew"]|["release"]=>op.mutate(w[0],json!({}))?,
            ["set",input,parameter,value]=>{
                let v=if *parameter=="mute" {match *value{"true"=>json!(true),"false"=>json!(false),_=>return Err("mute requires true/false".into())}}else{json!(value.parse::<i64>().map_err(|_|"integer value")?)};
                op.mutate("set",json!({"targets":[{"target":{"input":input,"parameter":parameter},"value":v}]}))?;
            },
            ["send",input,monitor,value]=>op.mutate("set",json!({"targets":[{"target":{"input":input,"parameter":"send","monitor":monitor},"value":value.parse::<i64>().map_err(|_|"integer send")?}]}))?,
            ["mode",mode]=>op.stage("set_mode",json!({"mode":mode,"bounds":[]}))?,
            ["confirm"]=>op.confirm()?,
            ["cancel"]=>{if op.draft.take().is_some_and(|d|d.kind=="release_preview") && let Some(preview)=op.session.preview(op.now()).cloned(){op.mutate("cancel_preview",json!({"token":preview.token}))?;}},
            ["preview",input,parameter]=>{op.mutate("preview_release",json!({"targets":[{"input":input,"parameter":parameter}]}))?;let preview=op.session.preview(op.now()).ok_or("preview expired/unavailable")?.clone();println!("ENGINE PREVIEW destinations={:?} ramp={} token={}",preview.destinations,preview.ramp_frames,preview.token);op.stage("release_preview",json!({"token":preview.token}))?;},
            ["preview-send",input,monitor]=>{op.mutate("preview_release",json!({"targets":[{"input":input,"parameter":"send","monitor":monitor}]}))?;let preview=op.session.preview(op.now()).ok_or("preview expired/unavailable")?.clone();println!("ENGINE PREVIEW destinations={:?} ramp={} token={}",preview.destinations,preview.ramp_frames,preview.token);op.stage("release_preview",json!({"token":preview.token}))?;},
            ["context-reset"]=>{op.draft=None;op.session.context_changed();},
            ["json",kind,rest @ ..]=>{let body=audio::decode_command_body(rest.join(" ").as_bytes())?;if matches!(*kind,"set_mode"|"release_preview"){op.stage(kind,body)?;}else{op.mutate(kind,body)?;}},
            ["export",path]=>std::fs::write(path,format!("{}\n",op.status())).map_err(|e|e.to_string())?,
            _=>return Err(format!("unknown operator command: {line}")),
        }
    }
    println!("{}", op.status());
    Ok(())
}
