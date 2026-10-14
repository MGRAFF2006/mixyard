//! Read-only projections of configured intent and the authoritative dry-run planner.
//! None of these observations establish permission, consumers, or applied audio.
use crate::*;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    PreflightFailed,
    ConfigValidationFailed,
    InventorySchemaUnsupported,
    CapacityExceeded,
    CapacityUnknown,
    SourceMissing,
    SourceAmbiguous,
    OutputMissing,
    OutputAmbiguous,
    UnsupportedObservedFormat,
    FormatUnknown,
    ConflictingObservation,
    RecaptureForbidden,
    CallReturnAliasForbidden,
    ExcludedCallReturn,
    MicMonitorConsentRequired,
    SendNotConfigured,
    SendMuted,
    SendZero,
    MixMasterMuted,
    MixMasterZero,
    OutputMuted,
    OutputZero,
    UnknownSourceReference,
    UnknownMixReference,
    PermissionUnknown,
    ConsumerUnknown,
    NativeStateUnknown,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Unknown {
    Unknown,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub subject: String,
    pub code: Reason,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityInspection {
    pub resource: &'static str,
    pub requested: usize,
    pub maximum: Option<usize>,
    pub reason: Option<Reason>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInspection<'a> {
    pub configured: &'a Source,
    /// Exact selector candidates, not proof that capture is active/permitted.
    pub candidates: Vec<&'a Observation>,
    pub planned_resolution: Option<ResolutionStatus>,
    pub permission: Unknown,
    pub capture_state: Unknown,
    pub reasons: Vec<Reason>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathInspection<'a> {
    pub source: &'a str,
    pub configured_send: Option<&'a Send>,
    /// Includes configured muted/zero sends. It is not a claim of live sharing.
    pub intended_potential_sharing: bool,
    pub excluded: bool,
    /// Product of the validated planner's send (including master) and output
    /// coefficients. Not a measurement. None means no accepted dry-run plan.
    pub planned_effective_coefficient: Option<f64>,
    pub applied_sharing: Unknown,
    pub reasons: Vec<Reason>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputInspection<'a> {
    pub configured: &'a OutputRoute,
    pub mix: Option<&'a Mix>,
    pub candidates: Vec<&'a OutputObservation>,
    pub planned_resolution: Option<ResolutionStatus>,
    pub consumer: Unknown,
    pub native_state: Unknown,
    pub reasons: Vec<Reason>,
    pub paths: Vec<PathInspection<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection<'a> {
    /// Inspection schema is independent of config/inventory schema 2.
    pub schema_version: u32,
    pub mode: &'static str,
    pub config_revision: u64,
    pub inventory_revision: u64,
    pub inventory_evidence: Evidence,
    pub platform: Platform,
    pub submixes: &'static str,
    pub native_state: Unknown,
    pub live_levels: Option<f64>,
    pub player_pause_state: Unknown,
    pub capture_state: Unknown,
    pub plan_available: bool,
    pub preflight_error: Option<String>,
    pub findings: Vec<Finding>,
    pub capacity: Vec<CapacityInspection>,
    pub sources: Vec<SourceInspection<'a>>,
    /// Full configured identities, including unused mixes and orphan sends.
    pub mixes: Vec<&'a Mix>,
    pub sends: Vec<&'a Send>,
    pub outputs: Vec<OutputInspection<'a>>,
    pub plan: Option<Plan>,
}

fn level_reasons(level: Gain, muted: Reason, zero: Reason, reasons: &mut Vec<Reason>) {
    if level.mute {
        reasons.push(muted);
    }
    if level.gain == 0.0 {
        reasons.push(zero);
    }
}

/// Inspect never repairs policy or creates an alternative applicable plan.
/// Only `plan` can populate `Inspection::plan`; rejection still yields useful
/// configured-path facts, with no effective coefficients.
pub fn inspect<'a>(config: &'a Config, inventory: &'a Inventory) -> Inspection<'a> {
    let (plan, preflight_error) = match plan(config, inventory) {
        Ok(plan) => (Some(plan), None),
        Err(error) => (None, Some(error)),
    };
    let mut findings = Vec::new();
    let mut finding = |subject: &str, code| {
        findings.push(Finding {
            subject: subject.into(),
            code,
        })
    };
    if preflight_error.is_some() {
        finding("preflight", Reason::PreflightFailed);
    }
    if config.validate().is_err() {
        finding("config", Reason::ConfigValidationFailed);
    }
    if inventory.schema_version != 2 {
        finding("inventory", Reason::InventorySchemaUnsupported);
    }
    finding("permission", Reason::PermissionUnknown);
    finding("consumer", Reason::ConsumerUnknown);
    finding("native", Reason::NativeStateUnknown);

    let capacity: Vec<_> = [
        (
            "exported-mixes",
            config
                .outputs
                .iter()
                .map(|o| &o.mix)
                .collect::<BTreeSet<_>>()
                .len(),
            inventory.limits.max_exported_mixes,
        ),
        (
            "output-routes",
            config.outputs.len(),
            inventory.limits.max_output_routes,
        ),
        (
            "virtual-inputs",
            config
                .outputs
                .iter()
                .filter(|o| matches!(o.target, Target::VirtualInput { .. }))
                .count(),
            inventory.limits.max_virtual_inputs,
        ),
    ]
    .into_iter()
    .map(|(resource, requested, maximum)| {
        let reason = match maximum {
            None => Some(Reason::CapacityUnknown),
            Some(maximum) if requested > maximum => Some(Reason::CapacityExceeded),
            _ => None,
        };
        if let Some(reason) = reason {
            finding(resource, reason);
        }
        CapacityInspection {
            resource,
            requested,
            maximum,
            reason,
        }
    })
    .collect();
    // These are diagnostic facts only. All admission and effective routing above
    // remains exclusively the existing validator/planner's decision.
    let mut configured_sources: Vec<_> = config.sources.iter().collect();
    configured_sources.sort_by(|a, b| a.id.cmp(&b.id));
    let sources: Vec<_> = configured_sources.into_iter().map(|source| {
        let mut candidates: Vec<_> = inventory.sources.iter().filter(|o| o.selector == source.selector).collect();
        candidates.sort_by(|a,b| a.observed_id.cmp(&b.observed_id));
        candidates.dedup();
        let mut reasons = Vec::new();
        let unique_ids = candidates.iter().map(|o| &o.observed_id).collect::<BTreeSet<_>>();
        match unique_ids.len() {
            0 => reasons.push(Reason::SourceMissing),
            1 => {},
            _ => reasons.push(Reason::SourceAmbiguous),
        }
        if candidates.iter().any(|o| inventory.sources.iter().any(|other| other.observed_id == o.observed_id && other != *o)) {
            reasons.push(Reason::ConflictingObservation);
        }
        if candidates.iter().any(|o| o.origin != Origin::External) { reasons.push(Reason::RecaptureForbidden); }
        if candidates.iter().any(|o| o.format.is_some_and(|f| f.validate().is_err())) { reasons.push(Reason::UnsupportedObservedFormat); }
        if candidates.iter().any(|o| o.format.is_none()) { reasons.push(Reason::FormatUnknown); }
        if let Selector::App { application_id } = &source.selector {
            if config.sources.iter().any(|s| matches!(&s.selector, Selector::CallReturn { application_id: app, .. } if app == application_id))
                || inventory.sources.iter().any(|s| matches!(&s.selector, Selector::CallReturn { application_id: app, .. } if app == application_id)) {
                reasons.push(Reason::CallReturnAliasForbidden);
            }
        }
        for reason in &reasons { finding(&format!("source:{}", source.id), *reason); }
        SourceInspection {
            configured: source,
            candidates,
            planned_resolution: plan.as_ref().and_then(|p| p.sources.iter().find(|s| s.source == source.id)).map(|s| s.status),
            permission: Unknown::Unknown,
            capture_state: Unknown::Unknown,
            reasons,
        }
    }).collect();
    let mut mixes: Vec<_> = config.mixes.iter().collect();
    mixes.sort_by(|a, b| a.id.cmp(&b.id));
    let mut sends: Vec<_> = config.sends.iter().collect();
    sends.sort_by(|a, b| (&a.source, &a.mix).cmp(&(&b.source, &b.mix)));
    for send in &sends {
        let source = config.sources.iter().find(|s| s.id == send.source);
        let mix = mixes.iter().find(|m| m.id == send.mix);
        if let (Some(source), Some(mix)) = (source, mix) {
            if matches!(&source.selector, Selector::CallReturn { group, .. } if mix.exclude_groups.contains(group))
            {
                finding(
                    &format!("send:{}->{}", send.source, send.mix),
                    Reason::ExcludedCallReturn,
                );
            }
        }
        if !config.sources.iter().any(|s| s.id == send.source) {
            finding(
                &format!("send:{}->{}", send.source, send.mix),
                Reason::UnknownSourceReference,
            );
        }
        if !mixes.iter().any(|m| m.id == send.mix) {
            finding(
                &format!("send:{}->{}", send.source, send.mix),
                Reason::UnknownMixReference,
            );
        }
    }
    let mut configured_outputs: Vec<_> = config.outputs.iter().collect();
    configured_outputs.sort_by(|a, b| a.id.cmp(&b.id));
    let outputs = configured_outputs.into_iter().map(|output| {
        let mix = mixes.iter().copied().find(|m| m.id == output.mix);
        let planned_output = plan.as_ref().and_then(|p| p.outputs.iter().find(|o| o.id == output.id));
        let mut candidates: Vec<_> = inventory.outputs.iter().filter(|o| o.target == output.target).collect();
        candidates.sort_by(|a,b| a.observed_id.cmp(&b.observed_id));
        candidates.dedup();
        let mut reasons = Vec::new();
        match candidates.iter().map(|o| &o.observed_id).collect::<BTreeSet<_>>().len() {
            0 => reasons.push(Reason::OutputMissing),
            1 => {},
            _ => reasons.push(Reason::OutputAmbiguous),
        }
        if candidates.iter().any(|o| inventory.outputs.iter().any(|other| other.observed_id == o.observed_id && other != *o)) { reasons.push(Reason::ConflictingObservation); }
        if candidates.iter().any(|o| o.format.is_some_and(|f| f.validate().is_err())) { reasons.push(Reason::UnsupportedObservedFormat); }
        if candidates.iter().any(|o| o.format.is_none()) { reasons.push(Reason::FormatUnknown); }
        if mix.is_none() { reasons.push(Reason::UnknownMixReference); }
        level_reasons(output.level(), Reason::OutputMuted, Reason::OutputZero, &mut reasons);
        let paths = sources.iter().map(|source| {
            let send = sends.iter().copied().find(|s| s.source == source.configured.id && s.mix == output.mix);
            let excluded = mix.is_some_and(|m| matches!(&source.configured.selector, Selector::CallReturn { group, .. } if m.exclude_groups.contains(group)));
            let mut path_reasons = source.reasons.clone();
            if excluded { path_reasons.push(Reason::ExcludedCallReturn); }
            match send {
                None => path_reasons.push(Reason::SendNotConfigured),
                Some(send) => {
                    level_reasons(send.level(), Reason::SendMuted, Reason::SendZero, &mut path_reasons);
                    if matches!(source.configured.selector, Selector::Mic { .. }) && matches!(output.target, Target::Device { .. }) && !output.allow_mic_monitor {
                        path_reasons.push(Reason::MicMonitorConsentRequired);
                    }
                }
            }
            if let Some(mix) = mix { level_reasons(mix.master, Reason::MixMasterMuted, Reason::MixMasterZero, &mut path_reasons); }
            path_reasons.extend(&reasons);
            let coefficient = plan.as_ref().and_then(|p| planned_output.map(|o| {
                p.sends.iter().find(|s| s.source == source.configured.id && s.mix == output.mix).map_or(0.0, |s| s.effective_gain * o.effective_gain)
            }));
            for reason in &path_reasons { finding(&format!("path:{}->{}->{}", source.configured.id, output.mix, output.id), *reason); }
            PathInspection {
                source: &source.configured.id,
                configured_send: send,
                intended_potential_sharing: send.is_some(),
                excluded,
                planned_effective_coefficient: coefficient,
                applied_sharing: Unknown::Unknown,
                reasons: path_reasons,
            }
        }).collect();
        OutputInspection { configured: output, mix, candidates, planned_resolution: planned_output.map(|o| o.status), consumer: Unknown::Unknown, native_state: Unknown::Unknown, reasons, paths }
    }).collect();
    Inspection {
        schema_version: 1,
        mode: "routing-inspection",
        config_revision: config.revision,
        inventory_revision: inventory.revision,
        inventory_evidence: inventory.evidence,
        platform: inventory.platform,
        submixes: "not-implemented",
        native_state: Unknown::Unknown,
        live_levels: None,
        player_pause_state: Unknown::Unknown,
        capture_state: Unknown::Unknown,
        plan_available: plan.is_some(),
        preflight_error,
        findings,
        capacity,
        sources,
        mixes,
        sends,
        outputs,
        plan,
    }
}
