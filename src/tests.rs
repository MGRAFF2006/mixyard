use super::*;

fn example() -> (Config, Inventory) {
    (
        serde_json::from_str(include_str!("../examples/config.json")).unwrap(),
        serde_json::from_str(include_str!("../examples/inventory.json")).unwrap(),
    )
}
fn request(device: &str, action: MediaAction) -> MediaRequest {
    MediaRequest {
        schema_version: 2,
        device_uid: Some(format!("example:{device}")),
        action,
        origin_attribution: OriginAttribution::DeviceAttributed,
        dispatch_ownership: DispatchOwnership::Mixyard,
        remembered_playing_session_ids: Vec::new(),
        remembered_playing_context: None,
    }
}
fn send<'a>(plan: &'a Plan, source: &str, mix: &str) -> &'a PlannedSend {
    plan.sends
        .iter()
        .find(|send| send.source == source && send.mix == mix)
        .unwrap()
}
fn output<'a>(plan: &'a Plan, id: &str) -> &'a PlannedOutput {
    plan.outputs.iter().find(|output| output.id == id).unwrap()
}
fn actions(plan: &MediaPlan) -> Vec<(&str, MediaAction)> {
    plan.players
        .iter()
        .filter_map(|player| {
            player
                .planned_action
                .map(|action| (player.application_id.as_str(), action))
        })
        .collect()
}
fn policy(config: &mut Config, device: &str, mode: MediaPolicy) {
    config
        .media_controls
        .iter_mut()
        .find(|control| control.device_uid == format!("example:{device}"))
        .unwrap()
        .policy = mode;
}

#[test]
fn simultaneous_independent_mic_sends_and_output_fanout() {
    let (mut config, inventory) = example();
    let before = plan(&config, &inventory).unwrap();
    assert_eq!(send(&before, "mic", "call").effective_gain, 1.0);
    assert_eq!(send(&before, "mic", "record").effective_gain, 0.5);
    assert_eq!(send(&before, "mic", "monitor").effective_gain, 0.0);
    assert_eq!(output(&before, "speaker-a").effective_gain, 1.0);
    assert_eq!(output(&before, "speaker-c").effective_gain, 0.5);
    config
        .sends
        .iter_mut()
        .find(|send| send.source == "mic" && send.mix == "record")
        .unwrap()
        .gain = 0.25;
    config
        .outputs
        .iter_mut()
        .find(|output| output.id == "speaker-c")
        .unwrap()
        .mute = true;
    let after = plan(&config, &inventory).unwrap();
    assert_eq!(send(&after, "mic", "record").effective_gain, 0.25);
    assert_eq!(send(&after, "mic", "call"), send(&before, "mic", "call"));
    assert_eq!(
        send(&after, "mic", "monitor"),
        send(&before, "mic", "monitor")
    );
    assert_eq!(output(&after, "speaker-a"), output(&before, "speaker-a"));
    assert_eq!(output(&after, "speaker-c").effective_gain, 0.0);
    assert_eq!(output(&after, "speaker-c").peak_bound, 0.0);
}

#[test]
fn arbitrary_names_many_mixes_many_outputs_no_truncation() {
    let (mut config, mut inventory) = example();
    for i in 0..64 {
        let id = format!("custom-{i}");
        config.mixes.push(Mix {
            id: id.clone(),
            name: format!("Unusual / café {i}"),
            channels: 2,
            sample_rate_hz: 48_000,
            master: Gain::default(),
            exclude_groups: vec![],
        });
        config.sends.push(Send {
            source: "spotify".into(),
            mix: id.clone(),
            gain: 1.0,
            mute: false,
        });
        for j in 0..3 {
            let target = Target::Device {
                device_uid: format!("custom-output-{i}-{j}"),
            };
            config.outputs.push(OutputRoute {
                id: format!("fanout-{i}-{j}"),
                mix: id.clone(),
                target: target.clone(),
                gain: 0.5,
                mute: false,
                allow_mic_monitor: false,
            });
            inventory.outputs.push(OutputObservation {
                observed_id: format!("runtime-{i}-{j}"),
                target,
                format: None,
            });
        }
    }
    let result = plan(&config, &inventory).unwrap();
    assert_eq!(result.mixes.len(), 70);
    assert_eq!(result.outputs.len(), 199);
    assert!(result
        .outputs
        .iter()
        .all(|output| output.status == ResolutionStatus::Selected));
    assert!(result
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "capacity-unverified"));
}

#[test]
fn mic_consent_semantic_for_arbitrary_names_even_muted() {
    let (mut config, inventory) = example();
    config
        .mixes
        .iter_mut()
        .find(|mix| mix.id == "monitor")
        .unwrap()
        .name = "Harmless music label".into();
    config
        .outputs
        .iter_mut()
        .find(|output| output.id == "headphones")
        .unwrap()
        .allow_mic_monitor = false;
    assert!(plan(&config, &inventory)
        .unwrap_err()
        .contains("allowMicMonitor"));
    config
        .outputs
        .iter_mut()
        .find(|output| output.id == "headphones")
        .unwrap()
        .mute = true;
    assert!(config.validate().unwrap_err().contains("allowMicMonitor"));
    config
        .sends
        .retain(|send| !(send.source == "mic" && send.mix == "monitor"));
    let result = plan(&config, &inventory).unwrap();
    assert!(!result
        .sends
        .iter()
        .any(|send| send.source == "mic" && send.mix == "monitor"));
    // Safe default for a newly configured physical output is no mic consent.
    let route: OutputRoute = serde_json::from_str(
        r#"{"id":"new","mix":"desk-a","target":{"kind":"device","deviceUid":"unique"},"gain":1}"#,
    )
    .unwrap();
    assert!(!route.allow_mic_monitor);
}

#[test]
fn multiple_call_groups_exclusions_survive_rename() {
    let (mut config, inventory) = example();
    config.sources.push(Source {
        id: "return-two".into(),
        selector: Selector::CallReturn {
            application_id: "example:call-two".into(),
            group: "second-call".into(),
        },
    });
    config.mixes.push(Mix {
        id: "unusual".into(),
        name: "Not named Call".into(),
        channels: 1,
        sample_rate_hz: 48_000,
        master: Gain::default(),
        exclude_groups: vec!["second-call".into()],
    });
    config.sends.push(Send {
        source: "return".into(),
        mix: "unusual".into(),
        gain: 0.5,
        mute: false,
    });
    assert!(plan(&config, &inventory).is_ok());
    config.sends.push(Send {
        source: "return-two".into(),
        mix: "unusual".into(),
        gain: 0.0,
        mute: true,
    });
    assert!(config.validate().unwrap_err().contains("mix-minus"));
    config
        .mixes
        .iter_mut()
        .find(|mix| mix.id == "unusual")
        .unwrap()
        .name = "Renamed".into();
    assert!(config.validate().unwrap_err().contains("second-call"));
    config.sends.pop();
    config.sends.push(Send {
        source: "return".into(),
        mix: "call".into(),
        gain: 0.0,
        mute: true,
    });
    assert!(config.validate().unwrap_err().contains("primary-call"));
}

#[test]
fn missing_ambiguous_sources_and_outputs_preserve_other_paths() {
    let (config, inventory) = example();
    let before = plan(&config, &inventory).unwrap();
    for ambiguous in [false, true] {
        let mut changed = inventory.clone();
        if ambiguous {
            let mut mic = changed.sources[0].clone();
            mic.observed_id = "duplicate-uid".into();
            changed.sources.push(mic);
            let mut speaker = changed.outputs[3].clone();
            speaker.observed_id = "ambiguous-speaker".into();
            changed.outputs.push(speaker);
        } else {
            changed.sources[0].selector = Selector::Mic {
                device_uid: "different-default".into(),
            };
            changed.outputs[3].target = Target::Device {
                device_uid: "different-default-output".into(),
            };
        }
        let result = plan(&config, &changed).unwrap();
        for route in &result.sends {
            if route.source == "mic" {
                assert_eq!(route.effective_gain, 0.0);
                assert_eq!(route.observed_id, None);
            } else {
                assert_eq!(route, send(&before, &route.source, &route.mix));
            }
        }
        assert_eq!(output(&result, "speaker-a").effective_gain, 0.0);
        assert_eq!(output(&result, "speaker-c"), output(&before, "speaker-c"));
        assert_eq!(result.mixes, before.mixes);
    }
}

#[test]
fn deterministic_duplicate_observations_and_order() {
    let (mut config, mut inventory) = example();
    let expected = serde_json::to_string(&plan(&config, &inventory).unwrap()).unwrap();
    let expected_media = plan_media(
        &config,
        &inventory,
        &request("combined-output", MediaAction::Pause),
    )
    .unwrap();
    config.sources.reverse();
    config.mixes.reverse();
    config.sends.reverse();
    config.outputs.reverse();
    config.media_controls.reverse();
    inventory.sources.extend(inventory.sources.clone());
    inventory.sources.reverse();
    inventory.outputs.extend(inventory.outputs.clone());
    inventory.outputs.reverse();
    inventory
        .media_sessions
        .extend(inventory.media_sessions.clone());
    inventory.media_sessions.reverse();
    for session in &mut inventory.media_sessions {
        session.supported_actions.as_mut().unwrap().reverse();
    }
    assert_eq!(
        serde_json::to_string(&plan(&config, &inventory).unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        plan_media(
            &config,
            &inventory,
            &request("combined-output", MediaAction::Pause)
        )
        .unwrap(),
        expected_media
    );
}

#[test]
fn headroom_counts_absent_sources_and_output_gain_no_normalization() {
    let (mut config, mut inventory) = example();
    let before = plan(&config, &inventory).unwrap();
    assert_eq!(output(&before, "speaker-c").peak_bound, 0.5);
    config
        .sends
        .iter_mut()
        .find(|send| send.source == "spotify" && send.mix == "record")
        .unwrap()
        .gain = 0.5;
    inventory.sources.clear();
    assert!(plan(&config, &inventory)
        .unwrap_err()
        .contains("lower this mix"));
    config
        .mixes
        .iter_mut()
        .find(|mix| mix.id == "record")
        .unwrap()
        .master
        .gain = 0.8;
    let result = plan(&config, &inventory).unwrap();
    assert_eq!(output(&result, "record-input").peak_bound, 1.0);
    assert!(result.sends.iter().all(|send| send.effective_gain == 0.0));
    assert_eq!(output(&result, "speaker-c").peak_bound, 0.5);
    config
        .outputs
        .iter_mut()
        .find(|output| output.id == "speaker-c")
        .unwrap()
        .gain = 0.25;
    assert_eq!(
        output(&plan(&config, &inventory).unwrap(), "speaker-c").peak_bound,
        0.25
    );
}

#[test]
fn invalid_gains_all_layers_and_converging_outputs_rejected() {
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01, 1.01] {
        for layer in 0..3 {
            let (mut config, _) = example();
            match layer {
                0 => {
                    config.sends[0].gain = invalid;
                    config.sends[0].mute = true;
                }
                1 => {
                    config.mixes[0].master.gain = invalid;
                    config.mixes[0].master.mute = true;
                }
                _ => {
                    config.outputs[0].gain = invalid;
                    config.outputs[0].mute = true;
                }
            }
            assert!(config
                .validate()
                .unwrap_err()
                .contains("gain must be finite"));
        }
    }
    let (mut config, _) = example();
    let mut converging = config.outputs[3].clone();
    converging.id = "another-mix-same-device".into();
    converging.mix = "shared".into();
    config.outputs.push(converging);
    assert!(config.validate().unwrap_err().contains("converging"));
}

#[test]
fn capacity_rejects_whole_request_and_reports_unknowns() {
    let (mut config, mut inventory) = example();
    for limits in [
        BackendLimits {
            max_exported_mixes: Some(3),
            ..Default::default()
        },
        BackendLimits {
            max_output_routes: Some(2),
            ..Default::default()
        },
        BackendLimits {
            max_virtual_inputs: Some(1),
            ..Default::default()
        },
    ] {
        inventory.limits = limits;
        let error = plan(&config, &inventory).unwrap_err();
        assert!(error.contains("backend-capacity") && error.contains("no partial"));
    }
    inventory.limits = BackendLimits {
        max_exported_mixes: Some(6),
        max_output_routes: Some(7),
        max_virtual_inputs: Some(2),
    };
    assert_eq!(plan(&config, &inventory).unwrap().outputs.len(), 7);
    for i in 0..100 {
        config.mixes.push(Mix {
            id: format!("unused-{i}"),
            name: format!("Logical mix {i}"),
            channels: 2,
            sample_rate_hz: 48_000,
            master: Gain::default(),
            exclude_groups: vec![],
        });
    }
    let logical = plan(&config, &inventory).unwrap();
    assert_eq!(logical.mixes.len(), 106); // Export capacity does not cap logical mix count.
    assert_eq!(logical.outputs.len(), 7);
}

#[test]
fn format_mappings_and_unverified_rates() {
    let (config, mut inventory) = example();
    let result = plan(&config, &inventory).unwrap();
    assert_eq!(send(&result, "mic", "call").channel_mapping, "mono");
    assert_eq!(
        send(&result, "mic", "record").channel_mapping,
        "duplicate-mono"
    );
    assert_eq!(
        output(&result, "call-input").channel_mapping,
        "duplicate-mono"
    );
    inventory.sources[0].format = None;
    inventory.outputs[3].format.as_mut().unwrap().sample_rate_hz = 44_100;
    let result = plan(&config, &inventory).unwrap();
    assert_eq!(send(&result, "mic", "call").channel_mapping, "unverified");
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == "format-unverified"));
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == "rate-conversion-required"));
    inventory.outputs[3].format.as_mut().unwrap().channels = 6;
    let result = plan(&config, &inventory).unwrap();
    assert_eq!(
        output(&result, "speaker-a").status,
        ResolutionStatus::UnsupportedFormat
    );
    assert_eq!(output(&result, "speaker-a").effective_gain, 0.0);
    assert_eq!(output(&result, "speaker-a").channel_mapping, "disconnected");
    assert_eq!(output(&result, "speaker-a").rate_conversion_required, None);
    assert_eq!(output(&result, "speaker-b").effective_gain, 1.0);
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.subject == "output:speaker-a" && d.code == "unsupported-format"));
}

#[test]
fn cycles_self_recapture_and_return_alias_rejected() {
    assert!(serde_json::from_str::<Source>(
        r#"{"id":"feedback","selector":{"kind":"mix","mixId":"record"}}"#
    )
    .is_err());
    let (mut config, mut inventory) = example();
    config.sends.push(Send {
        source: "call".into(),
        mix: "record".into(),
        gain: 0.5,
        mute: false,
    });
    assert!(config
        .validate()
        .unwrap_err()
        .contains("unknown send source"));
    config.sends.pop();
    for origin in [Origin::MixyardOutput, Origin::MixyardProducer] {
        inventory.sources[0].origin = origin;
        assert!(plan(&config, &inventory).unwrap_err().contains("recapture"));
    }
    inventory.sources[0].origin = Origin::External;
    config.sources.push(Source {
        id: "leak".into(),
        selector: Selector::App {
            application_id: "example:call-app".into(),
        },
    });
    assert!(config
        .validate()
        .unwrap_err()
        .contains("ordinary app/media"));
}

#[test]
fn schema_versions_unknown_fields_duplicates_conflicts_rejected() {
    let (mut config, mut inventory) = example();
    config.schema_version = 1;
    assert!(config.validate().unwrap_err().contains("expected 2"));
    config.schema_version = 2;
    inventory.schema_version = 1;
    assert!(plan(&config, &inventory)
        .unwrap_err()
        .contains("inventory schemaVersion"));
    assert!(serde_json::from_str::<Selector>(
        r#"{"kind":"mic","deviceUid":"stable","observedId":"runtime"}"#
    )
    .is_err());
    inventory.schema_version = 2;
    let mut conflict = inventory.media_sessions[0].clone();
    conflict.playback_state = PlaybackState::Paused;
    inventory.media_sessions.push(conflict);
    assert!(plan(&config, &inventory)
        .unwrap_err()
        .contains("conflicting media session"));
    let (mut config, _) = example();
    config.sends.push(config.sends[0].clone());
    assert!(config.validate().unwrap_err().contains("duplicate"));
}

#[test]
fn spotify_device_a_pause_does_not_control_vlc_or_private_player() {
    let (config, inventory) = example();
    let result = plan_media(
        &config,
        &inventory,
        &request("device-a", MediaAction::Pause),
    )
    .unwrap();
    assert_eq!(result.status, "ready");
    assert_eq!(actions(&result), [("example:spotify", MediaAction::Pause)]);
    assert_eq!(result.scope, "global-media-session");
    assert!(result.players[0]
        .affected_device_uids
        .contains(&"example:device-c".into()));
    assert!(result
        .players
        .iter()
        .all(|player| player.dispatch_outcome == "not-dispatched"));
    assert!(result.device_only_quiet_hint.contains("output mute/gate"));
    assert_eq!(
        actions(
            &plan_media(
                &config,
                &inventory,
                &request("device-b", MediaAction::Pause)
            )
            .unwrap()
        ),
        [("example:vlc", MediaAction::Pause)]
    );
}

#[test]
fn mixed_device_primary_all_and_ambiguous_missing_primary_no_fallback() {
    let (mut config, mut inventory) = example();
    let req = request("combined-output", MediaAction::Pause);
    assert_eq!(
        actions(&plan_media(&config, &inventory, &req).unwrap()).len(),
        2
    );
    policy(
        &mut config,
        "combined-output",
        MediaPolicy::Primary {
            application_id: "example:spotify".into(),
            session_id: None,
        },
    );
    assert_eq!(
        actions(&plan_media(&config, &inventory, &req).unwrap()),
        [("example:spotify", MediaAction::Pause)]
    );
    let mut second = inventory.media_sessions[0].clone();
    second.session_id = "second-spotify".into();
    inventory.media_sessions.push(second);
    let result = plan_media(&config, &inventory, &req).unwrap();
    assert_eq!(result.status, "ambiguous-primary");
    assert!(actions(&result).is_empty());
    policy(
        &mut config,
        "combined-output",
        MediaPolicy::Primary {
            application_id: "example:spotify".into(),
            session_id: Some("simulated:session:spotify".into()),
        },
    );
    assert_eq!(
        actions(&plan_media(&config, &inventory, &req).unwrap()).len(),
        1
    );
    policy(
        &mut config,
        "combined-output",
        MediaPolicy::Primary {
            application_id: "example:private-player".into(),
            session_id: None,
        },
    );
    let result = plan_media(&config, &inventory, &req).unwrap();
    assert_eq!(result.status, "missing-primary");
    assert!(result.players.is_empty());
}

#[test]
fn duplicate_sessions_and_shared_player_fanout_dispatch_once_per_session() {
    let (config, mut inventory) = example();
    inventory
        .media_sessions
        .extend(inventory.media_sessions.clone());
    let result = plan_media(
        &config,
        &inventory,
        &request("combined-output", MediaAction::Pause),
    )
    .unwrap();
    assert_eq!(result.players.len(), 2);
    assert_eq!(actions(&result).len(), 2);
    assert_eq!(result.remember_on_confirmed_pause.len(), 2);
    assert!(result.remembered_playing_session_ids.is_empty()); // Not claimed applied.
}

#[test]
fn origin_and_dispatch_ownership_required_os_default_untouched() {
    let (config, inventory) = example();
    for device in [None, Some("unknown".into())] {
        let mut req = request("device-a", MediaAction::Pause);
        req.device_uid = device;
        assert!(plan_media(&config, &inventory, &req)
            .unwrap()
            .players
            .is_empty());
    }
    let mut req = request("device-a", MediaAction::Pause);
    req.origin_attribution = OriginAttribution::Unavailable;
    assert_eq!(
        plan_media(&config, &inventory, &req).unwrap().status,
        "unknown-origin"
    );
    req.origin_attribution = OriginAttribution::DeviceAttributed;
    for owner in [DispatchOwnership::Os, DispatchOwnership::Unknown] {
        req.dispatch_ownership = owner;
        let result = plan_media(&config, &inventory, &req).unwrap();
        assert_eq!(result.status, "dispatch-not-owned");
        assert!(result.players.is_empty());
    }
    assert_eq!(
        plan_media(
            &config,
            &inventory,
            &request("device-c", MediaAction::Pause)
        )
        .unwrap()
        .status,
        "os-default"
    );
    assert!(serde_json::from_str::<MediaRequest>(
        r#"{"schemaVersion":2,"deviceUid":"example:device-a","action":"pause"}"#
    )
    .is_err());
}

#[test]
fn group_playpause_pauses_playing_only_then_resumes_remembered_without_streams() {
    let (config, mut inventory) = example();
    inventory.media_sessions[1].playback_state = PlaybackState::Paused;
    let mut req = request("combined-output", MediaAction::PlayPause);
    let paused = plan_media(&config, &inventory, &req).unwrap();
    assert_eq!(actions(&paused), [("example:spotify", MediaAction::Pause)]);
    assert_eq!(
        paused.remember_on_confirmed_pause,
        ["simulated:session:spotify"]
    );
    assert!(paused.remembered_playing_session_ids.is_empty());
    // Model externally confirmed pause, independently of removed audible captures.
    inventory
        .sources
        .retain(|source| !matches!(source.selector, Selector::App { .. }));
    inventory.media_sessions[0].playback_state = PlaybackState::Paused;
    req.remembered_playing_session_ids = paused.remember_on_confirmed_pause;
    req.remembered_playing_context = paused.remember_on_confirmed_pause_context;
    inventory.revision += 1; // Observation changes do not invalidate desired routes.
    let resumed = plan_media(&config, &inventory, &req).unwrap();
    assert_eq!(actions(&resumed), [("example:spotify", MediaAction::Play)]);
    assert!(resumed
        .players
        .iter()
        .all(|player| player.planned_action != Some(MediaAction::PlayPause)));
    req.action = MediaAction::Play;
    assert_eq!(
        actions(&plan_media(&config, &inventory, &req).unwrap()).len(),
        2
    );
}

#[test]
fn resume_revalidates_routes_states_and_never_global_fallback() {
    let (mut config, mut inventory) = example();
    let mut req = request("combined-output", MediaAction::PlayPause);
    let pause = plan_media(&config, &inventory, &req).unwrap();
    inventory.media_sessions[0].playback_state = PlaybackState::Paused;
    inventory.media_sessions[1].playback_state = PlaybackState::Paused;
    assert_eq!(
        plan_media(&config, &inventory, &req).unwrap().status,
        "no-resume-history"
    );
    req.remembered_playing_context = pause.remember_on_confirmed_pause_context;
    req.remembered_playing_session_ids = vec![
        "simulated:session:spotify".into(),
        "simulated:session:private-player".into(),
    ];
    assert_eq!(
        actions(&plan_media(&config, &inventory, &req).unwrap()),
        [("example:spotify", MediaAction::Play)]
    );
    inventory.media_sessions[0].playback_state = PlaybackState::Stopped;
    let result = plan_media(&config, &inventory, &req).unwrap();
    assert_eq!(result.status, "resume-state-changed");
    assert!(actions(&result).is_empty());
    inventory.media_sessions[0].playback_state = PlaybackState::Paused;
    config
        .sends
        .iter_mut()
        .find(|send| send.source == "spotify" && send.mix == "shared")
        .unwrap()
        .mute = true;
    let result = plan_media(&config, &inventory, &req).unwrap();
    assert_eq!(result.status, "no-resume-history");
    assert!(actions(&result).is_empty());
}

#[test]
fn pause_history_cannot_cross_route_revisions_even_if_old_history_is_replayed() {
    for remove_all in [false, true] {
        let (mut config, mut inventory) = example();
        let mut req = request("combined-output", MediaAction::PlayPause);
        let paused = plan_media(&config, &inventory, &req).unwrap();
        assert!(paused.remembered_playing_context.is_none());
        req.remembered_playing_session_ids = paused.remember_on_confirmed_pause;
        req.remembered_playing_context = paused.remember_on_confirmed_pause_context;
        for session in &mut inventory.media_sessions {
            session.playback_state = PlaybackState::Paused;
        }
        let original_sends = config.sends.clone();
        config.revision = 2;
        config
            .sends
            .retain(|send| send.mix != "shared" || (!remove_all && send.source != "spotify"));
        let removed = plan_media(&config, &inventory, &req).unwrap();
        assert_eq!(
            removed.status,
            if remove_all {
                "no-routed-players"
            } else {
                "no-resume-history"
            }
        );
        assert!(actions(&removed).is_empty());
        assert!(removed.remembered_playing_session_ids.is_empty());
        assert!(removed.remembered_playing_context.is_none());
        assert_eq!(
            removed.unavailable_remembered_session_ids,
            req.remembered_playing_session_ids
        );
        config.sends = original_sends;
        config.revision = 3;
        // Deliberately replay original revision-1 IDs/context, without using the
        // removal result: current membership alone must not imply continuity.
        let restored = plan_media(&config, &inventory, &req).unwrap();
        assert_eq!(restored.status, "no-resume-history");
        assert!(actions(&restored).is_empty());
        assert_eq!(
            restored.unavailable_remembered_session_ids,
            req.remembered_playing_session_ids
        );
    }
}

#[test]
fn legacy_wrong_version_device_and_policy_history_never_authorize_resume() {
    let (config, mut inventory) = example();
    let mut req = request("combined-output", MediaAction::PlayPause);
    let paused = plan_media(&config, &inventory, &req).unwrap();
    req.remembered_playing_session_ids = paused.remember_on_confirmed_pause;
    let context = paused.remember_on_confirmed_pause_context.unwrap();
    for session in &mut inventory.media_sessions {
        session.playback_state = PlaybackState::Paused;
    }
    assert_eq!(
        plan_media(&config, &inventory, &req).unwrap().status,
        "no-resume-history"
    );
    for change in ["version", "device", "policy", "membership"] {
        let mut context = context.clone();
        match change {
            "version" => context.schema_version = 0,
            "device" => context.device_uid = "example:device-a".into(),
            "policy" => {
                context.policy = MediaPolicy::Primary {
                    application_id: "example:spotify".into(),
                    session_id: None,
                }
            }
            "membership" => context.routed_application_ids.clear(),
            _ => unreachable!(),
        }
        req.remembered_playing_context = Some(context);
        let result = plan_media(&config, &inventory, &req).unwrap();
        assert_eq!(result.status, "no-resume-history", "{change}");
        assert!(actions(&result).is_empty());
        assert_eq!(
            result.unavailable_remembered_session_ids,
            req.remembered_playing_session_ids
        );
    }
    req.remembered_playing_context = Some(context.clone());
    let mut changed_policy = config.clone();
    policy(
        &mut changed_policy,
        "combined-output",
        MediaPolicy::Primary {
            application_id: "example:spotify".into(),
            session_id: None,
        },
    );
    assert_eq!(
        plan_media(&changed_policy, &inventory, &req)
            .unwrap()
            .status,
        "no-resume-history"
    );
    req.device_uid = Some("example:device-a".into());
    assert_eq!(
        plan_media(&config, &inventory, &req).unwrap().status,
        "no-resume-history"
    );
}

#[test]
fn unavailable_history_is_reported_before_device_primary_and_ownership_returns() {
    let (config, inventory) = example();
    let mut req = request("device-a", MediaAction::PlayPause);
    let paused = plan_media(&config, &inventory, &req).unwrap();
    req.remembered_playing_session_ids = paused.remember_on_confirmed_pause;
    req.remembered_playing_context = paused.remember_on_confirmed_pause_context;
    for change in [
        "no-output",
        "no-session",
        "os-policy",
        "no-origin",
        "no-ownership",
    ] {
        let mut config = config.clone();
        let mut inventory = inventory.clone();
        let mut req = req.clone();
        let expected = match change {
            "no-output" => {
                config.outputs.retain(|output| output.id != "speaker-a");
                "unknown-device"
            }
            "no-session" => {
                inventory.media_sessions.clear();
                "missing-primary"
            }
            "os-policy" => {
                policy(&mut config, "device-a", MediaPolicy::OsDefault);
                "os-default"
            }
            "no-origin" => {
                req.origin_attribution = OriginAttribution::Unavailable;
                "unknown-origin"
            }
            "no-ownership" => {
                req.dispatch_ownership = DispatchOwnership::Os;
                "dispatch-not-owned"
            }
            _ => unreachable!(),
        };
        let result = plan_media(&config, &inventory, &req).unwrap();
        assert_eq!(result.status, expected);
        assert!(result.remembered_playing_session_ids.is_empty());
        assert!(result.remembered_playing_context.is_none());
        assert_eq!(
            result.unavailable_remembered_session_ids,
            req.remembered_playing_session_ids
        );
        assert!(actions(&result).is_empty());
    }
}

#[test]
fn unsupported_observed_output_preserves_siblings_and_unrelated_media() {
    let (config, inventory) = example();
    for format in [
        AudioFormat {
            channels: 6,
            sample_rate_hz: 48_000,
        },
        AudioFormat {
            channels: 2,
            sample_rate_hz: 0,
        },
    ] {
        let mut inventory = inventory.clone();
        inventory.outputs[3].format = Some(format);
        let result = plan(&config, &inventory).unwrap();
        assert_eq!(
            output(&result, "speaker-a").status,
            ResolutionStatus::UnsupportedFormat
        );
        assert_eq!(output(&result, "speaker-a").effective_gain, 0.0);
        assert_eq!(
            output(&result, "speaker-b").status,
            ResolutionStatus::Selected
        );
        assert_eq!(output(&result, "speaker-b").effective_gain, 1.0);
        let affected = plan_media(
            &config,
            &inventory,
            &request("device-a", MediaAction::Pause),
        )
        .unwrap();
        assert_eq!(affected.status, "device-disconnected");
        assert!(actions(&affected).is_empty());
        let healthy = plan_media(
            &config,
            &inventory,
            &request("device-b", MediaAction::Pause),
        )
        .unwrap();
        assert_eq!(healthy.status, "ready");
        assert_eq!(actions(&healthy), [("example:vlc", MediaAction::Pause)]);
        assert!(healthy
            .players
            .iter()
            .all(|player| player.dispatch_outcome == "not-dispatched"));
        let mut unsafe_config = config.clone();
        unsafe_config.mixes[0].channels = 6;
        assert!(plan(&unsafe_config, &inventory).is_err());
        inventory.limits.max_output_routes = Some(1);
        assert!(plan(&config, &inventory)
            .unwrap_err()
            .contains("backend-capacity"));
    }
}

#[test]
fn unknown_state_capability_and_nonidempotent_actions_are_explicit() {
    let (config, mut inventory) = example();
    let req = request("combined-output", MediaAction::PlayPause);
    inventory.media_sessions[1].playback_state = PlaybackState::Unknown;
    let result = plan_media(&config, &inventory, &req).unwrap();
    assert_eq!(result.status, "unknown-playback-state");
    assert!(actions(&result).is_empty());
    assert!(result
        .players
        .iter()
        .any(|player| player.decision == "unknown-playback-state"));
    inventory.media_sessions[1].playback_state = PlaybackState::Playing;
    inventory.media_sessions[1].supported_actions = None;
    let result = plan_media(&config, &inventory, &req).unwrap();
    assert_eq!(result.status, "capability-blocked");
    assert!(actions(&result).is_empty());
    assert!(result
        .players
        .iter()
        .any(|player| player.decision == "unknown-capability"));
    inventory.media_sessions[1].supported_actions = Some(vec![MediaAction::PlayPause]);
    let result = plan_media(&config, &inventory, &req).unwrap();
    assert!(actions(&result).is_empty()); // Individual toggle is not a group substitute.
    inventory.media_sessions[1].supported_actions =
        Some(vec![MediaAction::Next, MediaAction::Previous]);
    for action in [MediaAction::Next, MediaAction::Previous] {
        let result = plan_media(&config, &inventory, &request("combined-output", action)).unwrap();
        assert_eq!(result.retry_policy, "never-automatically-retry");
        assert_eq!(actions(&result).len(), 2);
        assert!(result
            .players
            .iter()
            .all(|player| player.dispatch_outcome == "not-dispatched"));
    }
}

#[test]
fn disabled_or_ambiguous_output_never_targets_player() {
    let (mut config, mut inventory) = example();
    config
        .outputs
        .iter_mut()
        .find(|output| output.id == "speaker-a")
        .unwrap()
        .mute = true;
    assert_eq!(
        plan_media(&config, &inventory, &request("device-a", MediaAction::Play))
            .unwrap()
            .status,
        "missing-primary"
    );
    config
        .outputs
        .iter_mut()
        .find(|output| output.id == "speaker-a")
        .unwrap()
        .mute = false;
    let mut duplicate = inventory.outputs[3].clone();
    duplicate.observed_id = "ambiguous".into();
    inventory.outputs.push(duplicate);
    let result = plan_media(&config, &inventory, &request("device-a", MediaAction::Play)).unwrap();
    assert_eq!(result.status, "device-disconnected");
    assert!(result.players.is_empty());
}

#[test]
fn native_contract_and_simulated_evidence_are_not_live_state() {
    let native =
        r#"{"schemaVersion":1,"platform":"linux","capabilities":{},"devices":[],"diagnostics":[]}"#;
    assert!(serde_json::from_str::<Inventory>(native).is_err());
    let (config, inventory) = example();
    let result = plan(&config, &inventory).unwrap();
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(!encoded.contains("private"));
    assert_eq!(result.inventory_evidence, Evidence::Simulated);
    assert_eq!(result.mode, "dry-run");
    let result = plan_media(
        &config,
        &inventory,
        &request("device-a", MediaAction::Pause),
    )
    .unwrap();
    assert_eq!(result.mode, "dry-run-media-control");
    assert_eq!(result.inventory_evidence, Evidence::Simulated);
}
