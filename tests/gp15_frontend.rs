//! Explicit bounded external provider driver. All mutations use Frontend actions.
//! Run only under a mutually acknowledged reservation; it never starts a host or PCM.
use shr_desk::{
    actions::Action,
    brain::Source,
    frontend::{Config, Event, Frontend},
};
use std::{
    fs, thread,
    time::{Duration, Instant},
};
fn wait(f: &mut Frontend, end: Instant, label: &str, p: impl Fn(&Frontend) -> bool) {
    loop {
        f.pump();
        if p(f) {
            return;
        }
        assert!(Instant::now() < end, "{label}: {} {:?}", f.message, f.state);
        thread::sleep(Duration::from_millis(5));
    }
}
fn tap(f: &mut Frontend, key: &str) {
    f.enqueue(Event::Key {
        key: key.into(),
        pressed: true,
    })
    .unwrap();
    f.pump();
    f.enqueue(Event::Key {
        key: key.into(),
        pressed: false,
    })
    .unwrap();
    f.pump();
}
fn review(f: &mut Frontend, end: Instant, action: Action) {
    eprintln!("GP15_ACTION {:?}", action);
    f.inject_controller(action).unwrap();
    f.pump();
    wait(f, end, "complete review", |f| {
        f.state.as_ref().is_some_and(|u| u.review.is_some())
    });
    for _ in 0..f.review_pages() {
        f.mark_presented();
        tap(f, "PageDown");
    }
    let prior = f
        .state
        .as_ref()
        .and_then(|u| u.brain_final.as_ref())
        .map(|r| r.context.request_id.clone());
    tap(f, "Enter");
    wait(f, end, "correlated applied final", |f| {
        f.state.as_ref().is_some_and(|u| {
            u.brain_fresh
                && u.brain_final.as_ref().is_some_and(|r| {
                    r.reason.is_none() && Some(r.context.request_id.clone()) != prior
                })
        })
    });
    let r = f.state.as_ref().unwrap().brain_final.as_ref().unwrap();
    eprintln!("GP15_APPLIED {}", serde_json::to_string(r).unwrap());
}
fn attach(scope: &str, remote: shr_desk::remote::Config, epoch: u64, end: Instant) -> Frontend {
    let mut f = Frontend::new(Config {
        wire_version: 2,
        remote: Some(remote),
        endpoint: "/unused-explicit-remote".into(),
        show: std::env::var("GP15_SHOW")
            .unwrap_or_else(|_| "11111111-1111-4111-8111-111111111111".into()),
        epoch,
        writer: format!("desk-gp15-{scope}"),
        scope: scope.into(),
    });
    f.enable_brain_audio().unwrap();
    wait(&mut f, end, "Brain read-only attachment", |f| {
        f.fresh() && f.state.as_ref().is_some_and(|u| u.brain_fresh)
    });
    assert!(!f.state.as_ref().unwrap().writer_granted());
    tap(&mut f, "G");
    wait(&mut f, end, "separate scope grant", |f| {
        f.state.as_ref().is_some_and(|u| u.writer_granted())
    });
    if matches!(
        scope,
        "local_operator_monitor" | "talkback_destinations" | "talkback_foh"
    ) {
        tap(&mut f, "F3");
    }
    f
}
fn confirm_structure(f: &mut Frontend, end: Instant) {
    wait(f, end, "structural complete review", |f| {
        f.state.as_ref().is_some_and(|u| u.review.is_some())
    });
    for _ in 0..f.review_pages() {
        f.mark_presented();
        tap(f, "PageDown");
    }
    let prior = f
        .state
        .as_ref()
        .and_then(|u| u.structural_final.as_ref())
        .map(|r| r.context.clone());
    tap(f, "Enter");
    wait(f, end, "structural source boundary", |f| {
        f.state
            .as_ref()
            .and_then(|u| u.structural_final.as_ref())
            .is_some_and(|r| r.reason.is_none() && Some(r.context.clone()) != prior)
    });
    eprintln!(
        "GP15_STRUCTURE {}",
        serde_json::to_string(f.state.as_ref().unwrap().structural_final.as_ref().unwrap())
            .unwrap()
    );
}
#[test]
#[ignore = "actual mTLS provider only; GP15_REMOTE_CONFIG + GP15_EPOCH; mutually ACKed reservation required"]
fn actual_mtls_frontend_monitor_routes_and_ptt_release() {
    let end = Instant::now() + Duration::from_secs(45);
    let path = std::env::var("GP15_REMOTE_CONFIG").expect("explicit remote config");
    let mut remote: shr_desk::remote::Config =
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    remote.bind.set_port(0);
    let epoch = std::env::var("GP15_EPOCH").unwrap().parse().unwrap();
    // Configure advertised output routes while the provider is globally quiesced.
    let mut routes = attach("output_routes", remote.clone(), epoch, end);
    wait(&mut routes, end, "actual routing inventory", |f| {
        f.state.as_ref().is_some_and(|u| u.structural_fresh)
    });
    let topology = routes
        .state
        .as_ref()
        .unwrap()
        .structural
        .as_ref()
        .unwrap()
        .topology
        .clone();
    let mut outputs = topology.outputs.clone();
    for (i, p) in outputs.iter_mut().enumerate() {
        p.source = if i < topology.pa_outputs {
            Some(shr_desk::topology::OutputSource::Pa { index: i })
        } else if i - topology.pa_outputs < topology.monitors {
            Some(shr_desk::topology::OutputSource::Monitor {
                index: i - topology.pa_outputs,
            })
        } else {
            None
        };
    }
    routes.inject_controller(Action::StructureEdit).unwrap();
    routes.pump();
    routes
        .inject_controller(Action::StructureImport(
            serde_json::json!({"outputs":outputs}).to_string(),
        ))
        .unwrap();
    routes.pump();
    routes.inject_controller(Action::StructureApply).unwrap();
    routes.pump();
    confirm_structure(&mut routes, end);
    drop(routes);
    let mut pa = attach("pa_configuration", remote.clone(), epoch, end);
    wait(&mut pa, end, "actual PA state", |f| {
        f.state.as_ref().is_some_and(|u| u.structural_fresh)
    });
    pa.inject_controller(Action::OutputRearm).unwrap();
    pa.pump();
    confirm_structure(&mut pa, end);
    let mut mix = attach("foh", remote.clone(), epoch, end);
    for index in [
        0,
        mix.state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .inputs
            .len()
            - 1,
    ] {
        mix.inject_controller(Action::Move(index as i32 - mix.selected as i32))
            .unwrap();
        mix.pump();
        let input = mix
            .state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .inputs[index]
            .clone();
        let muted = mix
            .state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .parameters
            .iter()
            .find(|p| p.target.input == input && p.target.parameter == "mute")
            .is_some_and(|p| p.target_value == true);
        if muted {
            mix.inject_controller(Action::Mute).unwrap();
            mix.pump();
            wait(&mut mix, end, "channel unmute review", |f| {
                f.state.as_ref().is_some_and(|u| u.review.is_some())
            });
            for _ in 0..mix.review_pages() {
                mix.mark_presented();
                tap(&mut mix, "PageDown");
            }
            tap(&mut mix, "Enter");
            wait(&mut mix, end, "channel unmuted", |f| {
                f.state
                    .as_ref()
                    .and_then(|u| u.snapshot.as_ref())
                    .is_some_and(|s| {
                        s.authority.parameters.iter().any(|p| {
                            p.target.input == input
                                && p.target.parameter == "mute"
                                && p.target_value == false
                        })
                    })
            });
        }
        let revision = mix
            .state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .revision
            .clone();
        mix.inject_controller(Action::Adjust(-1000)).unwrap();
        mix.pump();
        wait(&mut mix, end, "actual FOH fader edit", |f| {
            f.fresh()
                && f.state
                    .as_ref()
                    .and_then(|u| u.snapshot.as_ref())
                    .is_some_and(|s| s.authority.revision != revision)
        });
        eprintln!(
            "GP15_MIX {}",
            serde_json::to_string(mix.state.as_ref().unwrap().snapshot.as_ref().unwrap()).unwrap()
        );
    }
    let mut monitor = attach("local_operator_monitor", remote.clone(), epoch, end);
    wait(&mut monitor, end, "actual device configuration", |f| {
        f.state.as_ref().is_some_and(|u| u.device_fresh)
    });
    monitor.inject_controller(Action::DeviceEdit).unwrap();
    monitor.pump();
    let mut config = monitor
        .device_draft
        .clone()
        .expect("draft from actual readback");
    let left = config.monitor[0].slot;
    config.monitor[0].slot = config.monitor[1].slot;
    config.monitor[1].slot = left;
    monitor
        .inject_controller(Action::DeviceText(config.review().unwrap()))
        .unwrap();
    monitor.pump();
    monitor.inject_controller(Action::DeviceApply).unwrap();
    monitor.pump();
    wait(&mut monitor, end, "complete device mapping review", |f| {
        f.state.as_ref().is_some_and(|u| u.review.is_some())
    });
    for _ in 0..monitor.review_pages() {
        monitor.mark_presented();
        tap(&mut monitor, "PageDown");
    }
    tap(&mut monitor, "Enter");
    wait(
        &mut monitor,
        end,
        "device applied beyond accepted intent",
        |f| {
            f.state.as_ref().is_some_and(|u| {
                u.device_final.as_ref().is_some_and(|r| {
                    r.state == "applied_device"
                        && r.observation.as_ref().is_some_and(|o| o.config == config)
                })
            })
        },
    );
    eprintln!(
        "GP15_DEVICE_APPLIED {}",
        serde_json::to_string(
            monitor
                .state
                .as_ref()
                .unwrap()
                .device_final
                .as_ref()
                .unwrap()
        )
        .unwrap()
    );
    let count = monitor
        .state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .authority
        .inputs
        .len();
    for input in [15, 16, 31, 32, 47].into_iter().filter(|n| *n < count) {
        review(
            &mut monitor,
            end,
            Action::BrainSource(Source::Pfl { input }),
        );
        assert_eq!(
            monitor
                .state
                .as_ref()
                .unwrap()
                .brain
                .as_ref()
                .unwrap()
                .source,
            Source::Pfl { input }
        );
        assert!(
            !monitor
                .state
                .as_ref()
                .unwrap()
                .brain
                .as_ref()
                .unwrap()
                .monitor_armed
        );
        review(
            &mut monitor,
            end,
            Action::BrainSource(Source::Afl { input }),
        );
    }
    review(
        &mut monitor,
        end,
        Action::BrainSource(Source::Pfl { input: count - 1 }),
    );
    if monitor
        .state
        .as_ref()
        .unwrap()
        .brain
        .as_ref()
        .unwrap()
        .monitor_mute
    {
        review(&mut monitor, end, Action::BrainMute);
    }
    review(&mut monitor, end, Action::BrainArm);
    wait(&mut monitor, end, "high PFL actual ready", |f| {
        f.state
            .as_ref()
            .and_then(|u| u.brain.as_ref())
            .is_some_and(|b| b.monitor_path_ready)
    });
    let observe = Instant::now() + Duration::from_secs(1);
    while Instant::now() < observe {
        monitor.pump();
        thread::sleep(Duration::from_millis(5));
    }
    eprintln!(
        "GP15_HIGH_PFL {}",
        serde_json::to_string(monitor.state.as_ref().unwrap().brain.as_ref().unwrap()).unwrap()
    );
    review(&mut monitor, end, Action::BrainSource(Source::Main));
    review(&mut monitor, end, Action::BrainGain(-1800));
    if monitor
        .state
        .as_ref()
        .unwrap()
        .brain
        .as_ref()
        .unwrap()
        .monitor_mute
    {
        review(&mut monitor, end, Action::BrainMute);
    }
    review(&mut monitor, end, Action::BrainArm);
    wait(
        &mut monitor,
        end,
        "actual monitor prefill after requested arm",
        |f| {
            f.state
                .as_ref()
                .and_then(|u| u.brain.as_ref())
                .is_some_and(|b| b.monitor_path_ready)
        },
    );
    #[cfg(feature = "native")]
    for (w, h) in [(1920, 1080), (960, 540), (540, 960), (3840, 2160)] {
        monitor.enqueue(Event::Resize(w, h)).unwrap();
        monitor.pump();
        // Requires caller's inspected CPU lavapipe environment; no display window.
        eprintln!(
            "GP15_NATIVE {}",
            shr_desk::native::offscreen_at(&monitor.scene(), w, h).unwrap()
        );
    }
    let mut tb = attach("talkback_destinations", remote.clone(), epoch, end);
    let buses = tb
        .state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .authority
        .monitors
        .len();
    assert!(buses > 0);
    review(
        &mut tb,
        end,
        Action::TalkbackConfigure {
            monitors: vec![buses - 1],
            gain_cdb: -1200,
            mute: false,
        },
    );
    let mut foh = attach("talkback_foh", remote, epoch, end);
    review(&mut foh, end, Action::TalkbackFoh(true));
    let foh_revision = foh
        .state
        .as_ref()
        .unwrap()
        .brain_final
        .as_ref()
        .unwrap()
        .revision
        .clone();
    wait(&mut tb, end, "TB observes protected FOH revision", |f| {
        f.state.as_ref().is_some_and(|u| {
            u.brain_fresh
                && u.brain
                    .as_ref()
                    .is_some_and(|b| b.revision == foh_revision && b.talkback_foh)
        })
    });
    for mode in ["key-up", "focus-loss", "controller-removal"] {
        let old = tb
            .state
            .as_ref()
            .unwrap()
            .brain
            .as_ref()
            .unwrap()
            .hold_generation_counter
            .clone();
        tb.enqueue(Event::Key {
            key: "T".into(),
            pressed: true,
        })
        .unwrap();
        tb.pump();
        wait(&mut tb, end, "PTT applied", |f| {
            f.state
                .as_ref()
                .and_then(|u| u.brain.as_ref())
                .is_some_and(|b| b.held_generation.is_some() && b.hold_generation_counter != old)
        });
        if mode == "key-up" && std::env::var_os("GP15_FAULT_MODE").is_some() {
            eprintln!(
                "GP15_FAULT_READY owned-duplex STOP/CONT or restart may now be injected by coordinator"
            );
            wait(
                &mut tb,
                end,
                "provider fault closure while UI still requests hold",
                |f| {
                    f.state
                        .as_ref()
                        .and_then(|u| u.brain.as_ref())
                        .is_some_and(|b| b.held_generation.is_none())
                },
            );
            tb.talkback_release();
            let watch = Instant::now() + Duration::from_secs(2);
            while Instant::now() < watch {
                tb.pump();
                monitor.pump();
                foh.pump();
                assert!(
                    tb.state
                        .as_ref()
                        .and_then(|u| u.brain.as_ref())
                        .is_none_or(|b| b.held_generation.is_none()),
                    "fault/restart resurrected a hold"
                );
                thread::sleep(Duration::from_millis(5));
            }
            eprintln!(
                "GP15_FAULT_CLOSED {}",
                serde_json::to_string(&tb.state.as_ref().and_then(|u| u.brain.as_ref())).unwrap()
            );
            // No recovery intent is replayed. A new operator press/arm is a separate action.
            return;
        }
        let held_generation = tb
            .state
            .as_ref()
            .unwrap()
            .brain
            .as_ref()
            .unwrap()
            .held_generation
            .clone();
        let hold =
            Instant::now() + Duration::from_millis(if mode == "key-up" { 2100 } else { 220 });
        while Instant::now() < hold {
            tb.pump();
            monitor.pump();
            foh.pump();
            assert!(
                tb.state.as_ref().is_some_and(|u| u
                    .brain
                    .as_ref()
                    .is_some_and(|b| b.held_generation == held_generation)),
                "PTT did not remain held: {:?}",
                tb.state
            );
            thread::sleep(Duration::from_millis(5));
        }
        match mode {
            "key-up" => tb
                .enqueue(Event::Key {
                    key: "T".into(),
                    pressed: false,
                })
                .unwrap(),
            "focus-loss" => tb.enqueue(Event::Focus(false)).unwrap(),
            _ => tb.controller_removed(),
        }
        wait(&mut tb, end, "actual PTT release readback", |f| {
            f.state
                .as_ref()
                .and_then(|u| u.brain.as_ref())
                .is_some_and(|b| b.held_generation.is_none())
        });
        eprintln!(
            "GP15_RELEASE {mode} {}",
            serde_json::to_string(tb.state.as_ref().unwrap().brain.as_ref().unwrap()).unwrap()
        );
        tb.enqueue(Event::Focus(true)).unwrap();
        tb.enqueue(Event::Key {
            key: "T".into(),
            pressed: false,
        })
        .unwrap();
        tb.pump();
    }
    review(&mut foh, end, Action::TalkbackFoh(false));
    review(
        &mut tb,
        end,
        Action::TalkbackConfigure {
            monitors: vec![],
            gain_cdb: -1200,
            mute: true,
        },
    );
    // Monitor-only controls and source changes remain separate from shared mix controls.
    review(&mut monitor, end, Action::BrainDim);
    review(&mut monitor, end, Action::BrainMute);
    review(
        &mut monitor,
        end,
        Action::BrainSource(Source::Pfl { input: count - 1 }),
    );
    review(&mut monitor, end, Action::BrainArm);
    wait(&mut monitor, end, "changed source prefill after arm", |f| {
        f.state
            .as_ref()
            .and_then(|u| u.brain.as_ref())
            .is_some_and(|b| b.monitor_path_ready)
    });
    review(&mut monitor, end, Action::BrainSource(Source::None));
    tap(&mut tb, "F5");
    wait(&mut tb, end, "read-only reconnect no replay", |f| {
        f.fresh()
            && f.state.as_ref().is_some_and(|u| {
                !u.writer_granted()
                    && u.brain
                        .as_ref()
                        .is_some_and(|b| b.held_generation.is_none())
            })
    });
    pa.inject_controller(Action::OutputMute).unwrap();
    pa.pump();
    confirm_structure(&mut pa, end);
    eprintln!("GP15_DRIVER_COMPLETE sample/owner assertions belong to coordinator");
}
