use super::*;
use crate::{plan, profiles::Profile, Config, Inventory, MediaPolicy, Plan, Selector, Target};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub token: StateToken,
    pub candidate: Option<Config>,
    pub routing: Option<Plan>,
    pub audio_applied: bool,
}

fn bound(len: usize) -> Result<(), Error> {
    if len > MAX_ITEMS {
        return Err(Error::new(
            "resource-limit",
            "proposal collection exceeds 256 items",
        ));
    }
    Ok(())
}
fn selector(s: &Selector) -> Result<(), Error> {
    use super::validate::text;
    match s {
        Selector::Mic { device_uid } => text(device_uid),
        Selector::App { application_id } => text(application_id),
        Selector::CallReturn {
            application_id,
            group,
        } => {
            text(application_id)?;
            text(group)
        }
    }
}
fn target(t: &Target) -> Result<(), Error> {
    match t {
        Target::Device { device_uid } => super::validate::text(device_uid),
        Target::VirtualInput { endpoint_id } => super::validate::text(endpoint_id),
    }
}
fn admit_config(c: &Config) -> Result<(), Error> {
    use super::validate::text;
    for len in [
        c.sources.len(),
        c.mixes.len(),
        c.sends.len(),
        c.outputs.len(),
        c.media_controls.len(),
    ] {
        bound(len)?;
    }
    let mut work =
        c.sources.len() + c.mixes.len() + c.sends.len() + c.outputs.len() + c.media_controls.len();
    for s in &c.sources {
        text(&s.id)?;
        selector(&s.selector)?;
    }
    for m in &c.mixes {
        text(&m.id)?;
        text(&m.name)?;
        bound(m.exclude_groups.len())?;
        work += m.exclude_groups.len();
        if work > MAX_WORK {
            return Err(Error::new("resource-limit", "proposal exceeds 4096 items"));
        }
        for group in &m.exclude_groups {
            text(group)?;
        }
    }
    for s in &c.sends {
        text(&s.source)?;
        text(&s.mix)?;
    }
    for o in &c.outputs {
        text(&o.id)?;
        text(&o.mix)?;
        target(&o.target)?;
    }
    for m in &c.media_controls {
        text(&m.device_uid)?;
        if let MediaPolicy::Primary {
            application_id,
            session_id,
        } = &m.policy
        {
            text(application_id)?;
            if let Some(s) = session_id {
                text(s)?;
            }
        }
    }
    Ok(())
}
fn admit_inventory(i: &Inventory) -> Result<(), Error> {
    use super::validate::text;
    for len in [i.sources.len(), i.outputs.len(), i.media_sessions.len()] {
        bound(len)?;
    }
    for s in &i.sources {
        text(&s.observed_id)?;
        selector(&s.selector)?;
    }
    for o in &i.outputs {
        text(&o.observed_id)?;
        target(&o.target)?;
    }
    let mut work = i.sources.len() + i.outputs.len() + i.media_sessions.len();
    for s in &i.media_sessions {
        text(&s.session_id)?;
        text(&s.application_id)?;
        if let Some(actions) = &s.supported_actions {
            bound(actions.len())?;
            work += actions.len();
            if work > MAX_WORK {
                return Err(Error::new("resource-limit", "inventory exceeds 4096 items"));
            }
        }
    }
    Ok(())
}

fn attenuate(
    mute: &mut bool,
    gain: &mut f64,
    next_mute: Option<bool>,
    next_gain: Option<f64>,
) -> Result<(), Error> {
    if let Some(next) = next_gain {
        if next > *gain {
            return Err(Error::new(
                "invalid-override",
                "automation overrides may only attenuate the selected configuration",
            ));
        }
        *gain = next;
    }
    if next_mute == Some(true) {
        *mute = true;
    }
    Ok(())
}

/// Separate, read-only materialization. No store writes, capture authority or native activation.
/// `expected` must come from the owner's CURRENT state, never copied blindly from an old result.
/// The adapter must still call profiles::preview and use the store's expected-revision write protocol.
pub fn preview_proposal(
    evaluation: &Evaluation,
    expected: &StateToken,
    current: &Config,
    profiles: &[Profile],
    inventory: &Inventory,
) -> Result<Proposal, Error> {
    // Admit all data that this helper will scan/clone before invoking existing validators/planner.
    super::validate::admit_action(&evaluation.action)?;
    for token in [&evaluation.token, expected] {
        super::validate::text(&token.graph_id)?;
        super::validate::text(&token.epoch)?;
    }
    admit_config(current)?;
    admit_inventory(inventory)?;
    bound(profiles.len())?;
    for p in profiles {
        super::validate::text(&p.id)?;
        super::validate::text(&p.name)?;
    }
    if let Action::Select {
        profile_id: Some(id),
        ..
    } = &evaluation.action
    {
        if let Some(p) = profiles.iter().find(|p| &p.id == id) {
            admit_config(&p.config)?;
        }
    }
    if &evaluation.token != expected
        || current.revision != expected.context.config_revision
        || inventory.revision != expected.context.inventory_revision
    {
        return Err(Error::new(
            "stale-decision",
            "decision/state tokens or configuration/inventory revisions changed; reevaluate",
        ));
    }
    if evaluation.schema_version != 1 || evaluation.audio_applied {
        return Err(Error::new(
            "invalid-automation",
            "not an unapplied format-v1 evaluation",
        ));
    }
    super::validate::action(&evaluation.action)?;
    let Action::Select {
        profile_id,
        overrides,
    } = &evaluation.action
    else {
        return Ok(Proposal {
            token: expected.clone(),
            candidate: None,
            routing: None,
            audio_applied: false,
        });
    };
    current
        .validate()
        .map_err(|e| Error::new("invalid-policy", e))?;
    let mut identities = BTreeSet::new();
    for profile in profiles {
        super::validate::text(&profile.id)?;
        if !identities.insert(&profile.id) {
            return Err(Error::new("invalid-policy", "duplicate profile identity"));
        }
    }
    let mut candidate = match profile_id {
        Some(id) => profiles
            .iter()
            .find(|p| &p.id == id)
            .ok_or_else(|| {
                Error::new(
                    "profile-not-found",
                    "selected exact profile ID no longer exists",
                )
            })?
            .config
            .clone(),
        None => current.clone(),
    };
    // A mute cannot rescue an invalid profile's missing consent or excluded call return.
    candidate
        .validate()
        .map_err(|e| Error::new("invalid-policy", e))?;
    for change in overrides {
        match change {
            Override::Send {
                source,
                mix,
                mute,
                gain,
            } => {
                let route = candidate
                    .sends
                    .iter_mut()
                    .find(|s| &s.source == source && &s.mix == mix)
                    .ok_or_else(|| {
                        Error::new("override-target-missing", "exact source/mix send is absent")
                    })?;
                attenuate(&mut route.mute, &mut route.gain, *mute, *gain)?;
            }
            Override::Output { output, mute, gain } => {
                let route = candidate
                    .outputs
                    .iter_mut()
                    .find(|o| &o.id == output)
                    .ok_or_else(|| {
                        Error::new("override-target-missing", "exact output ID is absent")
                    })?;
                attenuate(&mut route.mute, &mut route.gain, *mute, *gain)?;
            }
            Override::Mix { mix, mute, gain } => {
                let route = candidate
                    .mixes
                    .iter_mut()
                    .find(|m| &m.id == mix)
                    .ok_or_else(|| {
                        Error::new("override-target-missing", "exact mix ID is absent")
                    })?;
                attenuate(&mut route.master.mute, &mut route.master.gain, *mute, *gain)?;
            }
        }
    }
    candidate.revision = current
        .revision
        .max(candidate.revision)
        .checked_add(1)
        .ok_or_else(|| {
            Error::new(
                "revision-exhausted",
                "config revision exhausted; no proposal",
            )
        })?;
    let routing = plan(&candidate, inventory).map_err(|e| Error::new("invalid-policy", e))?;
    Ok(Proposal {
        token: expected.clone(),
        candidate: Some(candidate),
        routing: Some(routing),
        audio_applied: false,
    })
}
