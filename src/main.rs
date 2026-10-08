use shr_desk::local_audio;
use shr_desk::{
    model::actions::{self, Action, Inputs},
    model::{Desk, Edit, Mode, Page, Simulator},
    render,
};
use std::{
    error::Error,
    io::{self, BufRead, Read, Write},
    path::Path,
};

const HELP: &str = "shr-desk: offline surface prototype (no hardware or network)\n\n  shr-desk gallery DIRECTORY   Render three 1920x1080 SVG screens + index\n  shr-desk simulate            Line-oriented interactive control simulator\n\n  shr-desk --provider SHOW_UUID EPOCH --snapshot-file FILE [--snapshot-file FILE ...] [--select INPUT_ID] [--render FILE.svg]\n    Explicit headless read-only GP02 snapshot bodies; no connection or devices\n\nReal local offline provider (explicit opt-in):\n  shr-desk --audio-local ENDPOINT SHOW_UUID EPOCH WRITER foh|monitor1|monitor2 [--script FILE|-]\n  Script: snapshot, grant, input-release, set INPUT PARAM INTEGER|BOOL, send INPUT MONITOR MDB, mode manual|assist, json KIND BODY, status, export FILE, release\n  Measurement: --measurement-local ENDPOINT SHOW EPOCH WRITER pa_configuration --script FILE\n  measurement probe|capture|cancel|result|propose|apply; exact editor fields documented in docs/PA_MEASUREMENT.md.\n  Private owned Unix endpoint only; no hardware acceptance.\n\nSimulation commands:\n  select ID | level DB | pan -100..100 | mute | hold | release\n  mode auto|assist|manual | propose ID DB | page mix|channel|analysis\n  midi HEX HEX HEX   Inject one complete message using the fixture profile\n  confirm | cancel | back | bank -1|1 | key KEY | keydown KEY | keyup KEY\n  focus text|surface|lost | disconnect | reconnect | render FILE.svg | status | help | quit\n\nHold/mute/release/mode open previews; confirm commits, cancel discards.\nDraft pads: P1 Confirm, P2 Cancel, P8 Back; mode picker P1/P2/P3 Auto/Assist/Manual.\nKeys: F1/F2/F6 pages, arrows/Tab selection, PageUp/PageDown banks, +/- gain, [/ ] pan,\nM/H/R/A previews, F10 menu, 1/2/3 mode choice, Enter confirm, Esc back/cancel.\nSimulator state is synthetic and discarded on exit. No device is opened.\n\nRead-only module health: --modules-status ENDPOINT SHOW_UUID EPOCH\n\nReal frontend: --headless ENDPOINT SHOW EPOCH WRITER SCOPE OUTPUT.ppm [--role-provider EXEC REGISTRY ACQUIRE_JSON]\nOptional native feature: --native ENDPOINT SHOW EPOCH WRITER SCOPE [--role-provider EXEC REGISTRY ACQUIRE_JSON]\nCPU GPU check: --offscreen ENDPOINT SHOW EPOCH WRITER SCOPE OUTPUT.ppm\nAdd --brain-audio for explicit GP15 monitoring/PTT on C-AUDIO2. F3 Brain page; T hold talkback (release closes); source/arm and destinations require reviewed separate grants. Add --dynamic to explicitly select C-AUDIO2/rendered2 and GP07-processing4 (no silent downgrade). Add --processing to a real frontend invocation to probe GP07 on its shared connection. Channel: E edit, U/I field, J/K fine, N/P coarse, F4 Apply, Enter confirm, Esc cancel. F12 sends: F1 overview/F2 channel, U/I monitor, O/F/V scope reattach then NEW G grant; E tap draft, S level entry, F4 review. PA: F11 muted Master EQ; L probe live EQ then E edit; B/C section/channel.\nNative opens a window only when explicitly invoked; do not launch during software-only work.";

fn main() -> Result<(), Box<dyn Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let remote = if let Some(index) = args.iter().position(|a| a == "--remote-config") {
        if index + 1 >= args.len() {
            return Err("--remote-config requires explicit JSON file".into());
        }
        let path = args.remove(index + 1);
        args.remove(index);
        let bytes = std::fs::read(&path)?;
        if bytes.len() > 65536 {
            return Err("remote config capacity".into());
        }
        Some(serde_json::from_slice::<shr_desk::remote::Config>(&bytes)?)
    } else {
        None
    };
    let brain_audio = args.iter().any(|a| a == "--brain-audio");
    let dynamic = brain_audio || remote.is_some() || args.iter().any(|a| a == "--dynamic");
    let processing = args.iter().any(|a| a == "--processing");
    if (processing || dynamic)
        && !args
            .first()
            .is_some_and(|a| matches!(a.as_str(), "--headless" | "--native" | "--offscreen"))
    {
        return Err("--processing is an explicit real frontend capability probe".into());
    }
    args.retain(|a| a != "--processing" && a != "--dynamic" && a != "--brain-audio");
    match args.as_slice() {
        [] => println!("{HELP}"),
        [a] if a == "--help" || a == "help" => println!("{HELP}"),
        [a] if a == "simulate" => simulate()?,
        [a, path] if a == "gallery" => gallery(Path::new(path))?,
        [a, show, epoch, files @ ..] if a == "--provider" => provider_files(show, epoch, files)?,
        [a, rest @ ..] if a == "--audio-local" => local_audio::run(rest)?,
        [a, rest @ ..] if a == "--measurement-local" => local_audio::run_measurement(rest)?,
        [a, endpoint, show, epoch] if a == "--modules-status" => {
            let config = shr_desk::frontend::Config {
                wire_version: 1,
                remote: None,
                endpoint: endpoint.into(),
                show: show.clone(),
                epoch: epoch.parse().map_err(|_| "module epoch")?,
                writer: "modules-readonly".into(),
                scope: "foh".into(),
            };
            println!(
                "{}",
                serde_json::to_string(&shr_desk::modules::query(&config)?)?
            );
        }
        [a, rest @ ..] if a == "--headless" => {
            frontend_headless(rest, processing, dynamic, remote.clone(), brain_audio)?
        }
        #[cfg(feature = "native")]
        [a, rest @ ..] if a == "--native" => {
            let (mut config, role) = native_args(rest)?;
            config.wire_version = if dynamic { 2 } else { 1 };
            config.remote = remote.clone();
            shr_desk::native::run_with_capabilities(config, role, processing, brain_audio)?;
        }
        #[cfg(feature = "native")]
        [a, rest @ ..] if a == "--offscreen" => {
            let (mut config, output, role) = headless_args(rest)?;
            config.wire_version = if dynamic { 2 } else { 1 };
            config.remote = remote.clone();
            let mut front = shr_desk::frontend::Frontend::new(config);
            if let Some(role) = role {
                front.attach_role(role);
            }
            if processing {
                front.enable_processing()?;
            }
            if brain_audio {
                front.enable_brain_audio()?;
            }
            wait_front(&mut front)?;
            println!("{}", shr_desk::native::offscreen(&front.scene())?);
            shr_desk::raster::ppm(&front.scene(), Path::new(output))?;
        }
        _ => return Err("invalid arguments; use --help".into()),
    }
    Ok(())
}

fn gallery(path: &Path) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(path)?;
    let sim = Simulator::default();
    let mut desk = Desk::new(sim.state.clone());
    desk.select(11);
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>SHR Desk screen drafts</title><style>body{background:#10151d;color:#e4e8e9;font:18px monospace;margin:24px}a{color:#66dfd3}img{width:100%;height:auto;border:1px solid #3c4f63}section{margin:40px 0}</style><h1>SHR Desk / Full-HD screen drafts</h1><p>Offline simulation. Terminus 12x24. No live audio, MIDI, network or engine.</p><nav><a href=\"#mix\">Mix</a> | <a href=\"#channel\">Channel</a> | <a href=\"#analysis\">Analysis</a></nav>",
    );
    for (page, name) in [
        (Page::Mix, "mix"),
        (Page::Channel, "channel"),
        (Page::Analysis, "analysis"),
    ] {
        desk.page = page;
        let scene = render::scene(&desk);
        if !scene.in_bounds() {
            return Err("scene outside full-HD viewport".into());
        }
        std::fs::write(path.join(format!("{name}.svg")), render::svg(&scene))?;
        html.push_str(&format!("<section id=\"{name}\"><h2>{name}</h2><a href=\"{name}.svg\">Open at 1920x1080</a><img src=\"{name}.svg\" alt=\"{name} simulation screen\"></section>"));
    }
    html.push_str("</html>");
    std::fs::write(path.join("index.html"), html)?;
    println!("{}", path.join("index.html").display());
    Ok(())
}

fn dispatch(d: &mut Desk, s: &mut Simulator, c: shr_desk::model::Command) -> Result<(), String> {
    let ack = s.apply(&c);
    if !d.resolve(&ack, s.state.clone()) {
        return Err("uncorrelated response".into());
    }
    ack.result.map(|_| ()).map_err(|e| format!("{e:?}"))
}
fn mdb(s: &str) -> Result<i32, String> {
    let v = s.parse::<f64>().map_err(|_| "invalid dB value")?;
    if !v.is_finite() || !(-90.0..=12.0).contains(&v) {
        return Err("dB must be finite and within -90..12".into());
    }
    Ok((v * 1000.0).round() as i32)
}
fn status(d: &Desk) {
    println!(
        "SIMULATION | {:?} | {:?} | link={} | revision={} | {}",
        d.page, d.confirmed.mode, d.connected, d.confirmed.revision, d.last_result
    );
    if let Some(c) = d.selected() {
        println!(
            "CH {:02} {}: {:+.1} dB / pan {:+} / mute {} / hold {} / target {:+.1} dB",
            c.id,
            c.name,
            f64::from(c.gain_mdb) / 1000.0,
            c.pan,
            c.muted,
            c.held,
            f64::from(c.proposed_mdb) / 1000.0
        );
    }
}

fn simulate() -> Result<(), Box<dyn Error>> {
    let mut sim = Simulator::default();
    let mut desk = Desk::new(sim.state.clone());
    let mut inputs = Inputs::default();
    println!("{HELP}");
    status(&desk);
    for line in io::stdin().lock().lines() {
        let line = line?;
        let words: Vec<_> = line.split_whitespace().collect();
        if words.as_slice() == ["quit"] {
            break;
        }
        if let Err(error) = command(&words, &mut desk, &mut sim, &mut inputs) {
            println!("REFUSED: {error}");
        }
        status(&desk);
        io::stdout().flush()?;
    }
    Ok(())
}

fn command(w: &[&str], d: &mut Desk, s: &mut Simulator, inputs: &mut Inputs) -> Result<(), String> {
    let channel = d.selected;
    let action = match w {
        [] | ["status"] => return Ok(()),
        ["help"] => {
            println!("{HELP}");
            return Ok(());
        }
        ["select", id] => Action::Select(id.parse().map_err(|_| "invalid channel")?),
        ["bank", delta] => Action::Bank(delta.parse().map_err(|_| "invalid bank delta")?),
        ["level", db] => {
            actions::apply(
                d,
                Action::Edit(Edit::Gain {
                    channel,
                    mdb: mdb(db)?,
                }),
            )?;
            Action::Confirm
        }
        ["pan", value] => {
            actions::apply(
                d,
                Action::Edit(Edit::Pan {
                    channel,
                    value: value.parse().map_err(|_| "invalid pan")?,
                }),
            )?;
            Action::Confirm
        }
        ["mute"] => Action::Mute,
        ["hold"] => Action::Hold,
        ["release"] => Action::Release,
        ["menu"] => Action::Menu,
        ["mode"] => Action::ModePicker,
        ["mode", mode] => {
            if d.draft.is_none() {
                actions::apply(d, Action::ModePicker)?;
            }
            Action::ChooseMode(match *mode {
                "auto" => Mode::Auto,
                "assist" => Mode::Assist,
                "manual" => Mode::Manual,
                _ => return Err("unknown mode".into()),
            })
        }
        ["confirm"] => Action::Confirm,
        ["cancel"] => Action::Cancel,
        ["back"] => Action::Back,
        ["page", page] => Action::Page(match *page {
            "mix" => Page::Mix,
            "channel" => Page::Channel,
            "analysis" => Page::Analysis,
            _ => return Err("unknown page".into()),
        }),
        ["key", key] => {
            let result = inputs.key(d, key, true);
            inputs.key(d, key, false)?;
            let c = result?;
            if let Some(c) = c {
                dispatch(d, s, c)?;
            }
            return Ok(());
        }
        ["keydown", key] | ["keyup", key] => {
            if let Some(c) = inputs.key(d, key, w[0] == "keydown")? {
                dispatch(d, s, c)?;
            }
            return Ok(());
        }
        ["focus", "text"] => {
            inputs.context_loss(d);
            inputs.text_focus = true;
            return Ok(());
        }
        ["focus", "surface"] | ["focus", "lost"] => {
            inputs.context_loss(d);
            return Ok(());
        }
        ["propose", id, db] => {
            s.propose(id.parse().map_err(|_| "invalid channel")?, mdb(db)?)
                .map_err(|e| format!("{e:?}"))?;
            if d.connected {
                d.reconnect(s.state.clone());
            }
            return Ok(());
        }
        ["disconnect"] => {
            d.disconnect();
            inputs.context_loss(d);
            return Ok(());
        }
        ["reconnect"] => {
            d.reconnect(s.state.clone());
            inputs.context_loss(d);
            return Ok(());
        }
        ["render", path] => {
            std::fs::write(path, render::svg(&render::scene(d))).map_err(|e| e.to_string())?;
            return Ok(());
        }
        ["midi", a, b, c] => {
            let bytes =
                [a, b, c].map(|v| u8::from_str_radix(v, 16).map_err(|_| "invalid hex byte"));
            if let Some(c) = inputs.midi(d, &[bytes[0]?, bytes[1]?, bytes[2]?])? {
                dispatch(d, s, c)?;
            }
            return Ok(());
        }
        _ => return Err("unknown command; use help".into()),
    };
    if let Some(c) = actions::apply(d, action)? {
        dispatch(d, s, c)?;
    }
    Ok(())
}

fn provider_files(show: &str, epoch: &str, args: &[String]) -> Result<(), Box<dyn Error>> {
    let parsed_epoch = epoch.parse::<u64>()?;
    if parsed_epoch.to_string() != epoch {
        return Err("noncanonical epoch".into());
    }
    let epoch = parsed_epoch;
    let mut client = shr_desk::provider::Client::new(show, epoch)?;
    let started = std::time::Instant::now();
    let mut files = 0usize;
    let mut select = None;
    let mut render_path = None;
    let mut pairs = args.chunks_exact(2);
    for pair in &mut pairs {
        match pair[0].as_str() {
            "--snapshot-file" => {
                files += 1;
                if files > 16 {
                    return Err("at most16 input messages".into());
                }
                let mut bytes = Vec::new();
                std::fs::File::open(&pair[1])?
                    .take(shr_desk::provider::MAX_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)?;
                client.ingest(&bytes, started.elapsed().as_millis() as u64)?;
            }
            "--select" if select.is_none() => select = Some(pair[1].clone()),
            "--render" if render_path.is_none() => render_path = Some(pair[1].clone()),
            _ => return Err("invalid provider option".into()),
        }
    }
    if !pairs.remainder().is_empty() || files == 0 {
        return Err("explicit snapshot files required".into());
    }
    if client.snapshot().is_none() || client.is_collecting() {
        return Err("incomplete snapshot".into());
    }
    if let Some(id) = select {
        client.select(&id)?;
    }
    let now = started.elapsed().as_millis() as u64;
    for line in client.lines(now) {
        println!("{line}");
    }
    if let Some(path) = render_path {
        std::fs::write(path, render::svg(&render::provider_scene(&client, now)))?;
    }
    Ok(())
}

fn frontend_config(args: &[String]) -> Result<shr_desk::frontend::Config, String> {
    if args.len() != 5 {
        return Err("usage: --native ENDPOINT SHOW_UUID EPOCH WRITER foh|monitor1|monitor2".into());
    }
    let config = shr_desk::frontend::Config {
        wire_version: 1,
        remote: None,
        endpoint: args[0].clone().into(),
        show: args[1].clone(),
        epoch: args[2].parse().map_err(|_| "epoch")?,
        writer: args[3].clone(),
        scope: args[4].clone(),
    };
    shr_desk::audio::Session::new_version(
        &config.show,
        config.epoch,
        &config.writer,
        &config.scope,
        2,
    )?;
    Ok(config)
}
fn headless_args(
    args: &[String],
) -> Result<
    (
        shr_desk::frontend::Config,
        &str,
        Option<shr_desk::roles::Config>,
    ),
    String,
> {
    if args.len() != 6 && args.len() != 10 {
        return Err(
            "usage: --headless|--offscreen ENDPOINT SHOW_UUID EPOCH WRITER SCOPE OUTPUT.ppm".into(),
        );
    }
    Ok((
        frontend_config(&args[..5])?,
        &args[5],
        role_args(&args[6..])?,
    ))
}
fn wait_front(front: &mut shr_desk::frontend::Frontend) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        front.pump();
        if front.state.is_some() {
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            return Err("frontend initial state deadline".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
fn frontend_headless(
    args: &[String],
    processing: bool,
    dynamic: bool,
    remote: Option<shr_desk::remote::Config>,
    brain_audio: bool,
) -> Result<(), String> {
    let (mut config, output, role) = headless_args(args)?;
    config.wire_version = if dynamic { 2 } else { 1 };
    config.remote = remote;
    let mut front = shr_desk::frontend::Frontend::new(config);
    if let Some(role) = role {
        front.attach_role(role);
    }
    if processing {
        front.enable_processing()?;
    }
    if brain_audio {
        front.enable_brain_audio()?;
    }
    wait_front(&mut front)?;
    shr_desk::raster::ppm(&front.scene(), Path::new(output))?;
    println!(
        "real-provider headless scene; fresh={} status={}",
        front.fresh(),
        front
            .state
            .as_ref()
            .map_or("unavailable", |s| s.status.as_str())
    );
    Ok(())
}

fn role_args(args: &[String]) -> Result<Option<shr_desk::roles::Config>, String> {
    if args.is_empty() {
        return Ok(None);
    }
    if args.len() != 4 || args[0] != "--role-provider" {
        return Err(
            "expected --role-provider ABSOLUTE_EXECUTABLE ABSOLUTE_PRIVATE_DIRECTORY ACQUIRE_JSON"
                .into(),
        );
    }
    if !std::fs::symlink_metadata(&args[3])
        .map_err(|e| e.to_string())?
        .file_type()
        .is_file()
    {
        return Err("role acquire must be a regular nonsymlink JSON file".into());
    }
    let mut text = String::new();
    std::fs::File::open(&args[3])
        .map_err(|e| e.to_string())?
        .take(65537)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() > 65536 {
        return Err("role acquire capacity".into());
    }
    let acquire = shr_desk::roles::decode_acquire(text.as_bytes())?;
    Ok(Some(shr_desk::roles::Config {
        executable: args[1].clone().into(),
        directory: args[2].clone().into(),
        acquire,
    }))
}
#[cfg(feature = "native")]
fn native_args(
    args: &[String],
) -> Result<(shr_desk::frontend::Config, Option<shr_desk::roles::Config>), String> {
    if args.len() != 5 && args.len() != 9 {
        return Err("--native ENDPOINT SHOW_UUID EPOCH WRITER SCOPE [--role-provider EXECUTABLE PRIVATE_DIRECTORY ACQUIRE_JSON]".into());
    }
    Ok((frontend_config(&args[..5])?, role_args(&args[5..])?))
}
