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
fn assert_ui_ok(f: &Frontend) {
    // pump reports semantic action failures through message, not enqueue's Result.
    assert!(
        f.message.is_empty()
            || f.message == "Inventory changed: intents discarded; refreshing authority",
        "frontend action failed: {}; state={:?}",
        f.message,
        f.state.as_ref().map(|u| (
            &u.status,
            &u.brain_status,
            u.writer_lease_remaining_ms,
            u.received.elapsed(),
            u.fresh,
            u.brain_fresh,
            u.brain.as_ref().map(|b| (&b.revision, &b.held_generation))
        ))
    );
    if let Some(u) = &f.state {
        assert!(!u.status.starts_with("REFUSED/UNCERTAIN:"), "{}", u.status);
    }
}
fn action(f: &mut Frontend, action: Action) {
    eprintln!("GP15_ACTION {action:?}");
    f.message.clear();
    f.inject_controller(action).unwrap();
    f.pump();
    assert_ui_ok(f);
}
fn fresh_snapshot(f: &mut Frontend, end: Instant) {
    wait(f, end, "fresh actual raw snapshot", |f| {
        f.fresh() && f.state.as_ref().is_some_and(|u| u.snapshot.is_some())
    });
}
fn brain_revision(f: &Frontend) -> u64 {
    f.state
        .as_ref()
        .unwrap()
        .brain
        .as_ref()
        .unwrap()
        .revision
        .parse()
        .unwrap()
}
fn observe_revision(f: &mut Frontend, end: Instant, revision: u64) {
    // A different controller may still display a coherent but older revision.
    // Observe the preceding confirmed change before constructing the next intent.
    wait(f, end, "cross-controller raw/Brain revision barrier", |f| {
        f.fresh()
            && f.state.as_ref().is_some_and(|u| {
                u.brain_fresh
                    && u.snapshot
                        .as_ref()
                        .zip(u.brain.as_ref())
                        .is_some_and(|(raw, brain)| {
                            raw.authority.revision == brain.revision
                                && brain.revision.parse::<u64>().unwrap() >= revision
                        })
            })
    });
}
fn actual_device(f: &Frontend) -> bool {
    f.fresh()
        && f.state.as_ref().is_some_and(|u| {
            u.device_fresh
                && u.device
                    .as_ref()
                    .is_some_and(|d| d.connected && d.observation.is_some())
        })
}
fn tap(f: &mut Frontend, key: &str) {
    f.message.clear();
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
    assert_ui_ok(f);
}
// Presentation is accepted only while the displayed authority is fresh. A
// REVIEW READY update alone can be stale; never skip a page or recreate intent.
fn present_review(f: &mut Frontend, end: Instant) {
    let expected = f
        .state
        .as_ref()
        .and_then(|u| u.review.clone())
        .expect("review");
    f.synchronize_review();
    let pages = f.review_pages();
    for page in 0..pages {
        wait(f, end, "fresh unchanged review page", |f| {
            assert_ui_ok(f);
            assert_eq!(
                f.state.as_ref().and_then(|u| u.review.as_ref()),
                Some(&expected),
                "review changed during presentation"
            );
            f.fresh()
        });
        assert_eq!(f.review_pages(), pages);
        f.mark_presented();
        if page + 1 < pages {
            tap(f, "PageDown");
        }
    }
    wait(f, end, "fresh reviewed confirmation", |f| {
        assert_ui_ok(f);
        assert_eq!(
            f.state.as_ref().and_then(|u| u.review.as_ref()),
            Some(&expected),
            "review changed before confirmation"
        );
        f.fresh()
    });
}
fn review(f: &mut Frontend, end: Instant, requested: Action) {
    fresh_snapshot(f, end);
    wait(f, end, "fresh Brain before action", |f| {
        f.fresh() && f.state.as_ref().is_some_and(|u| u.brain_fresh)
    });
    action(f, requested);
    wait(f, end, "complete review", |f| {
        assert_ui_ok(f);
        f.state.as_ref().is_some_and(|u| u.review.is_some())
    });
    present_review(f, end);
    let prior = f
        .state
        .as_ref()
        .and_then(|u| u.brain_final.as_ref())
        .map(|r| r.context.request_id.clone());
    tap(f, "Enter");
    wait(f, end, "correlated applied final", |f| {
        assert_ui_ok(f);
        f.state.as_ref().is_some_and(|u| {
            f.fresh()
                && u.brain_fresh
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
        assert_ui_ok(f);
        f.fresh()
            && f.state
                .as_ref()
                .is_some_and(|u| u.writer_granted() && u.snapshot.is_some() && u.brain_fresh)
    });
    if matches!(
        scope,
        "local_operator_monitor" | "talkback_destinations" | "talkback_foh"
    ) {
        tap(&mut f, "F3");
    }
    fresh_snapshot(&mut f, end);
    f
}
fn confirm_structure(f: &mut Frontend, end: Instant) {
    wait(f, end, "structural complete review", |f| {
        assert_ui_ok(f);
        f.state.as_ref().is_some_and(|u| u.review.is_some())
    });
    present_review(f, end);
    let prior = f
        .state
        .as_ref()
        .and_then(|u| u.structural_final.as_ref())
        .map(|r| r.context.clone());
    tap(f, "Enter");
    wait(f, end, "structural source boundary", |f| {
        assert_ui_ok(f);
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
    action(&mut routes, Action::StructureEdit);
    for (i, output) in outputs.iter().enumerate() {
        let draft = routes
            .structural_draft
            .as_ref()
            .expect("detached output draft");
        assert_eq!(draft.fields[draft.selected], format!("/outputs/{i}/source"));
        action(
            &mut routes,
            Action::StructureText(serde_json::to_string(&output.source).unwrap()),
        );
        if i + 1 < outputs.len() {
            action(&mut routes, Action::StructureField(1));
        }
    }
    let intended = serde_json::json!({"outputs":outputs});
    assert_eq!(
        routes.structural_draft.as_ref().unwrap().body().unwrap(),
        intended
    );
    action(&mut routes, Action::StructureApply);
    confirm_structure(&mut routes, end);
    // A changed source map deliberately retires the old authenticated session
    // after its final reply. Explicitly reconnect read-only; never replay the
    // patch or restore its writer grant while verifying the new exact map.
    tap(&mut routes, "F5");
    wait(&mut routes, end, "fresh applied exact output patch", |f| {
        f.fresh()
            && f.state.as_ref().is_some_and(|u| {
                u.structural_fresh
                    && u.structural.as_ref().is_some_and(|s| {
                        serde_json::json!({"outputs":s.topology.outputs}) == intended
                    })
            })
    });
    assert!(!routes.state.as_ref().unwrap().writer_granted());
    drop(routes);
    let mut pa = attach("pa_configuration", remote.clone(), epoch, end);
    wait(&mut pa, end, "actual PA state", |f| {
        f.state.as_ref().is_some_and(|u| u.structural_fresh)
    });
    action(&mut pa, Action::OutputRearm);
    confirm_structure(&mut pa, end);
    wait(&mut pa, end, "fresh rearmed output state", |f| {
        f.fresh()
            && f.state.as_ref().is_some_and(|u| {
                u.structural_fresh && u.structural.as_ref().is_some_and(|s| !s.outputs_quiesced)
            })
    });
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
        let delta = index as i32 - mix.selected as i32;
        action(&mut mix, Action::Move(delta));
        fresh_snapshot(&mut mix, end);
        assert_eq!(mix.selected, index);
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
            action(&mut mix, Action::Mute);
            wait(&mut mix, end, "channel unmute review", |f| {
                assert_ui_ok(f);
                f.state.as_ref().is_some_and(|u| u.review.is_some())
            });
            present_review(&mut mix, end);
            tap(&mut mix, "Enter");
            wait(&mut mix, end, "channel unmuted", |f| {
                assert_ui_ok(f);
                f.fresh()
                    && f.state
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
        fresh_snapshot(&mut mix, end);
        let expected_fader = mix
            .state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .parameters
            .iter()
            .find(|p| {
                p.target.input == input
                    && p.target.parameter == "fader"
                    && p.target.monitor.is_none()
            })
            .expect("actual FOH fader")
            .target_value
            .as_i64()
            .unwrap()
            - 1000;
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
        action(&mut mix, Action::Adjust(-1000));
        wait(&mut mix, end, "actual FOH fader edit", |f| {
            assert_ui_ok(f);
            f.fresh()
                && f.state
                    .as_ref()
                    .and_then(|u| u.snapshot.as_ref())
                    .is_some_and(|s| {
                        s.authority.revision != revision
                            && s.authority.parameters.iter().any(|p| {
                                p.target.input == input
                                    && p.target.parameter == "fader"
                                    && p.target.monitor.is_none()
                                    && p.target_value == expected_fader
                            })
                    })
        });
        eprintln!(
            "GP15_MIX {}",
            serde_json::to_string(mix.state.as_ref().unwrap().snapshot.as_ref().unwrap()).unwrap()
        );
    }
    // The setup mix controller is finished; its stable intent remains. Keep
    // the PA controller for the explicit global safety mute at test completion.
    drop(mix);
    eprintln!("GP15_ROUTES_READY");
    let mut monitor = attach("local_operator_monitor", remote.clone(), epoch, end);
    wait(
        &mut monitor,
        end,
        "actual connected device configuration",
        actual_device,
    );
    action(&mut monitor, Action::DeviceEdit);
    let mut config = monitor
        .device_draft
        .clone()
        .expect("draft from actual readback");
    let original_config = config.clone();
    let left = config.monitor[0].slot;
    config.monitor[0].slot = config.monitor[1].slot;
    config.monitor[1].slot = left;
    assert_ne!(
        config, original_config,
        "device mapping must actually change"
    );
    action(&mut monitor, Action::DeviceText(config.review().unwrap()));
    assert_eq!(monitor.device_draft.as_ref(), Some(&config));
    action(&mut monitor, Action::DeviceApply);
    wait(&mut monitor, end, "complete device mapping review", |f| {
        assert_ui_ok(f);
        f.state.as_ref().is_some_and(|u| u.review.is_some())
    });
    present_review(&mut monitor, end);
    tap(&mut monitor, "Enter");
    wait(
        &mut monitor,
        end,
        "device applied beyond accepted intent",
        |f| {
            assert_ui_ok(f);
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
    wait(
        &mut monitor,
        end,
        "fresh applied actual device mapping",
        |f| {
            actual_device(f)
                && f.state
                    .as_ref()
                    .unwrap()
                    .device
                    .as_ref()
                    .unwrap()
                    .observation
                    .as_ref()
                    .unwrap()
                    .config
                    == config
        },
    );
    fresh_snapshot(&mut monitor, end);
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
    let main_observe = Instant::now() + Duration::from_secs(1);
    while Instant::now() < main_observe {
        monitor.pump();
        thread::sleep(Duration::from_millis(5));
    }
    eprintln!(
        "GP15_MAIN {}",
        serde_json::to_string(monitor.state.as_ref().unwrap().brain.as_ref().unwrap()).unwrap()
    );
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
    observe_revision(&mut tb, end, foh_revision.parse().unwrap());
    assert!(
        tb.state
            .as_ref()
            .unwrap()
            .brain
            .as_ref()
            .unwrap()
            .talkback_foh
    );
    observe_revision(&mut monitor, end, foh_revision.parse().unwrap());
    // Unity-gain pre-TB monitor return is the independent band-only witness.
    review(
        &mut monitor,
        end,
        Action::BrainSource(Source::Monitor { index: buses - 1 }),
    );
    review(&mut monitor, end, Action::BrainGain(0));
    review(&mut monitor, end, Action::BrainArm);
    wait(
        &mut monitor,
        end,
        "pre-TB performer monitor witness ready",
        |f| {
            f.state
                .as_ref()
                .and_then(|u| u.brain.as_ref())
                .is_some_and(|b| {
                    b.source == Source::Monitor { index: buses - 1 }
                        && b.monitor_armed
                        && b.monitor_path_ready
                        && b.monitor_gain_cdb == 0
                        && !b.monitor_mute
                        && !b.monitor_dim
                })
        },
    );
    let monitor_revision = monitor
        .state
        .as_ref()
        .unwrap()
        .brain
        .as_ref()
        .unwrap()
        .revision
        .clone();
    observe_revision(&mut tb, end, monitor_revision.parse().unwrap());
    eprintln!(
        "GP15_PRE_TB_MONITOR {}",
        serde_json::to_string(monitor.state.as_ref().unwrap().brain.as_ref().unwrap()).unwrap()
    );
    for mode in ["key-up", "focus-loss", "controller-removal"] {
        // Each new operator press needs a currently admitted UI state. The
        // closed observation interval alone is not a freshness/lease barrier.
        wait(
            &mut tb,
            end,
            "fresh released authority before new PTT",
            |f| {
                f.fresh()
                    && f.state.as_ref().is_some_and(|u| {
                        u.brain_fresh
                            && u.held_baseline_ready
                            && u.writer_granted()
                            && u.snapshot.as_ref().zip(u.brain.as_ref()).is_some_and(
                                |(raw, brain)| {
                                    raw.authority.revision == brain.revision
                                        && brain.held_generation.is_none()
                                },
                            )
                    })
            },
        );
        let old = tb
            .state
            .as_ref()
            .unwrap()
            .brain
            .as_ref()
            .unwrap()
            .hold_generation_counter
            .clone();
        tb.message.clear();
        tb.enqueue(Event::Key {
            key: "T".into(),
            pressed: true,
        })
        .unwrap();
        tb.pump();
        assert_ui_ok(&tb);
        wait(&mut tb, end, "PTT applied", |f| {
            f.state
                .as_ref()
                .and_then(|u| u.held_status.as_ref())
                .is_some_and(|b| {
                    b.fresh() && b.generation.is_some() && b.generation.as_ref() != Some(&old)
                })
        });
        if mode == "key-up" && std::env::var_os("GP15_FAULT_MODE").is_some() {
            let observe = Instant::now() + Duration::from_secs(1);
            while Instant::now() < observe {
                tb.pump();
                assert!(
                    tb.state
                        .as_ref()
                        .and_then(|u| u.held_status.as_ref())
                        .is_some_and(|b| b.fresh() && b.generation.is_some())
                );
                monitor.pump();
                foh.pump();
                thread::sleep(Duration::from_millis(5));
            }
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
                assert!(
                    tb.state
                        .as_ref()
                        .and_then(|u| u.brain.as_ref())
                        .is_none_or(|b| b.held_generation.is_none()),
                    "fault/restart resurrected a hold"
                );
                monitor.pump();
                foh.pump();
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
            .held_status
            .as_ref()
            .unwrap()
            .generation
            .clone();
        let hold =
            Instant::now() + Duration::from_millis(if mode == "key-up" { 2100 } else { 1100 });
        while Instant::now() < hold {
            tb.pump();
            let assertion_age = tb
                .state
                .as_ref()
                .and_then(|u| u.held_status.as_ref())
                .map(|b| b.observed.elapsed());
            assert!(
                tb.state.as_ref().is_some_and(|u| u
                    .held_status
                    .as_ref()
                    .is_some_and(|b| b.fresh()
                        && b.generation == held_generation
                        && b.media_authorized)),
                "PTT did not remain held: assertion_age={assertion_age:?}; TB={:?}; monitor={:?}; FOH={:?}",
                tb.state,
                monitor.state.as_ref().map(|u| (
                    &u.status,
                    &u.brain_status,
                    u.writer_lease_remaining_ms,
                    u.received.elapsed(),
                    u.fresh,
                    u.brain_fresh
                )),
                foh.state.as_ref().map(|u| (
                    &u.status,
                    &u.brain_status,
                    u.writer_lease_remaining_ms,
                    u.received.elapsed(),
                    u.fresh,
                    u.brain_fresh
                ))
            );
            monitor.pump();
            foh.pump();
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
        let released_observe = Instant::now() + Duration::from_millis(350);
        while Instant::now() < released_observe {
            tb.pump();
            monitor.pump();
            foh.pump();
            assert!(
                tb.state
                    .as_ref()
                    .and_then(|u| u.brain.as_ref())
                    .is_some_and(|b| b.held_generation.is_none())
            );
            thread::sleep(Duration::from_millis(5));
        }
        tb.enqueue(Event::Focus(true)).unwrap();
        tb.enqueue(Event::Key {
            key: "T".into(),
            pressed: false,
        })
        .unwrap();
        tb.pump();
    }
    observe_revision(&mut foh, end, brain_revision(&tb));
    review(&mut foh, end, Action::TalkbackFoh(false));
    observe_revision(&mut tb, end, brain_revision(&foh));
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
    observe_revision(&mut monitor, end, brain_revision(&tb));
    review(&mut monitor, end, Action::BrainDim);
    review(&mut monitor, end, Action::BrainMute);
    review(
        &mut monitor,
        end,
        Action::BrainSource(Source::Pfl { input: count - 1 }),
    );
    review(&mut monitor, end, Action::BrainArm);
    assert!(
        monitor
            .state
            .as_ref()
            .and_then(|u| u.brain.as_ref())
            .is_some_and(|b| b.monitor_mute)
    );
    // Source changes and arm preserve mute. Audible readiness requires an
    // explicit unmute, using the same reviewed action and original deadline.
    review(&mut monitor, end, Action::BrainMute);
    assert!(
        monitor
            .state
            .as_ref()
            .and_then(|u| u.brain.as_ref())
            .is_some_and(|b| !b.monitor_mute)
    );
    wait(&mut monitor, end, "changed source prefill after arm", |f| {
        f.state
            .as_ref()
            .and_then(|u| u.brain.as_ref())
            .is_some_and(|b| b.monitor_path_ready)
    });
    review(&mut monitor, end, Action::BrainSource(Source::None));
    let final_monitor_revision = brain_revision(&monitor);
    tap(&mut tb, "F5");
    wait(&mut tb, end, "read-only reconnect no replay", |f| {
        f.fresh()
            && f.state.as_ref().is_some_and(|u| {
                !u.writer_granted()
                    && u.brain_fresh
                    && u.brain
                        .as_ref()
                        .zip(u.snapshot.as_ref())
                        .is_some_and(|(b, raw)| {
                            b.held_generation.is_none()
                                && b.source == Source::None
                                && b.revision == raw.authority.revision
                                && b.revision.parse::<u64>().unwrap() >= final_monitor_revision
                        })
            })
    });
    observe_revision(&mut pa, end, final_monitor_revision);
    wait(&mut pa, end, "fresh live PA structural authority", |f| {
        f.fresh()
            && f.state.as_ref().is_some_and(|u| {
                u.writer_granted()
                    && u.brain_fresh
                    && u.structural_fresh
                    && u.structural
                        .as_ref()
                        .zip(u.snapshot.as_ref())
                        .is_some_and(|(s, raw)| {
                            s.revision == raw.authority.revision
                                && u.brain
                                    .as_ref()
                                    .is_some_and(|b| b.revision == raw.authority.revision)
                        })
            })
    });
    action(&mut pa, Action::OutputMute);
    confirm_structure(&mut pa, end);
    eprintln!("GP15_DRIVER_COMPLETE sample/owner assertions belong to coordinator");
}

#[test]
#[ignore = "actual restarted mTLS provider only; GP15_REMOTE_CONFIG + GP15_EPOCH; mutually ACKed reservation required"]
fn actual_mtls_frontend_readonly_restart_probe() {
    let end = Instant::now() + Duration::from_secs(10);
    let path = std::env::var("GP15_REMOTE_CONFIG").expect("explicit remote config");
    let mut remote: shr_desk::remote::Config =
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    remote.bind.set_port(0);
    let epoch = std::env::var("GP15_EPOCH").unwrap().parse().unwrap();
    let mut f = Frontend::new(Config {
        wire_version: 2,
        remote: Some(remote),
        endpoint: "/unused-explicit-remote".into(),
        show: std::env::var("GP15_SHOW")
            .unwrap_or_else(|_| "11111111-1111-4111-8111-111111111111".into()),
        epoch,
        writer: "desk-gp15-readonly-restart".into(),
        scope: "local_operator_monitor".into(),
    });
    f.enable_brain_audio().unwrap();
    wait(
        &mut f,
        end,
        "fresh read-only Brain/device restart observation",
        |f| {
            actual_device(f)
                && f.state
                    .as_ref()
                    .is_some_and(|u| u.brain_fresh && u.snapshot.is_some())
        },
    );
    let u = f.state.as_ref().unwrap();
    assert!(
        !u.writer_granted(),
        "read-only attachment must not acquire a grant"
    );
    assert!(u.writer_lease_remaining_ms.is_none());
    let brain = u.brain.as_ref().unwrap();
    let device = u.device.as_ref().unwrap();
    let observation = device.observation.as_ref().unwrap();
    assert!(
        brain.held_generation.is_none(),
        "restart retained a held gesture"
    );
    assert!(
        !brain.monitor_armed,
        "restart automatically armed monitoring"
    );
    assert_eq!(
        observation.status["armed"], false,
        "actual device must remain closed"
    );
    if let Ok(minimum) = std::env::var("GP15_EXPECT_BRAIN_EPOCH_MIN") {
        let minimum: u64 = minimum.parse().expect("integer minimum Brain epoch");
        assert!(
            observation.brain_epoch >= minimum,
            "Brain epoch did not reach required new minimum"
        );
    }
    eprintln!(
        "GP15_READONLY_RECOVERY {}",
        serde_json::json!({"source_epoch":epoch,"raw":u.snapshot,"brain":brain,"device":device,
            "writer_granted":u.writer_granted(),"writer_lease_remaining_ms":u.writer_lease_remaining_ms})
    );
}
