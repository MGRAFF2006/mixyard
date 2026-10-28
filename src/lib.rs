//! Frozen headless policy v2: examples/config.json, inventory.json, media-request.json.
//! Native discovery envelope v1 remains separate; adapters must map stable identity,
//! source provenance, output formats, persistent media sessions and capacity.
//!
//! Config: sources -> named mixes -> physical/virtual outputs. No special names,
//! mix-count cap or chaining; layers are acyclic. Gain/mute per send/output, master
//! per mix, mono/stereo format and explicit excludeGroups. Excluded call-return
//! sends are rejected even muted. Mixyard producer/output recapture and app aliases
//! of call returns are rejected. Every physical output carrying any configured mic
//! send needs allowMicMonitor consent, including muted/zero sends. Omitted sends OFF.
//! Outputs use the complete target channel layout; converging routes are rejected.
//! Renaming changes presentation only. Linear gains finite [0,1]; conservative peak
//! = master * sum(unmuted sends), including absent sources, then * output gain.
//! Never normalize. No limiter/DSP. Exact missing/ambiguous targets stay disconnected.
//! Duplicate observations deduplicate; conflicts fail. Known backend capacities
//! reject the whole plan, never truncate. Unknown limits/formats are unverified.
//! Native fanout needs independent clock adaptation and bounded output lifecycles;
//! a slow/lost output must not stall working outputs. No such native claim here.
//!
//! Media association uses enabled configured app->mix->device routes plus observed
//! session applicationId, not capture streams (pause can remove a stream, not Play).
//! Actual session IDs deduplicate; private/unselected players and call returns excluded.
//! Primary missing/ambiguous never falls back. OS ownership is the default. Planning
//! actions requires caller-supplied device attribution AND exclusive dispatch
//! ownership; native attribution/ownership qualification is still pending.
//! Group PlayPause becomes explicit Pause if any selected player plays, otherwise
//! Play only for remembered sessions still routed and paused in the same device,
//! config revision and media policy context. Config revisions must advance on
//! every desired edit; raw IDs without versioned context cannot authorize resume.
//! Unknown state/capability blocks group actions. Pause-memory candidates require confirmed native outcomes;
//! every player outcome here is not-dispatched. Future native dispatch must report
//! partial/unknown outcomes per player; never blindly retry Next/Previous.
//! Scope is global-media-session: one player shared by A+B pauses both. Quieting
//! only device A is an output mute/gate, not device-scoped player pause.
//!
//! validate/plan/plan-media are pure, display-free JSON computations. No daemon, IPC,
//! native capture or OS writes. Examples are simulated. Results have config/inventory
//! revisions and schemaVersion 2. Exit 0 completed (including degraded/unavailable),
//! 2 usage/unsupported, 3 invalid input/policy, 4 I/O. Future GUI/tray/CLI must share
//! an independent native runtime surviving UI closure; this crate has no UI deps.

pub mod audio;
pub mod automation;
pub mod context;
pub mod diagnostics;
pub mod dsp;
pub mod latency;
pub mod profiles;
mod store_fs;

use serde::{Deserialize, Serialize};
pub mod runtime;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Selector {
    Mic {
        #[serde(rename = "deviceUid")]
        device_uid: String,
    },
    App {
        #[serde(rename = "applicationId")]
        application_id: String,
    },
    CallReturn {
        #[serde(rename = "applicationId")]
        application_id: String,
        group: String,
    },
}
impl Selector {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Mic { device_uid } => nonempty("deviceUid", device_uid),
            Self::App { application_id } => nonempty("applicationId", application_id),
            Self::CallReturn {
                application_id,
                group,
            } => {
                nonempty("applicationId", application_id)?;
                nonempty("call-return group", group)
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub selector: Selector,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Gain {
    pub gain: f64,
    #[serde(default)]
    pub mute: bool,
}
impl Default for Gain {
    fn default() -> Self {
        Self {
            gain: 1.0,
            mute: false,
        }
    }
}
impl Gain {
    fn amplitude(self) -> f64 {
        if self.mute {
            0.0
        } else {
            self.gain
        }
    }
    fn validate(self) -> Result<(), String> {
        if !self.gain.is_finite() || !(0.0..=1.0).contains(&self.gain) {
            return Err("gain must be finite and in [0,1]".into());
        }
        Ok(())
    }
}

fn preferred_rate() -> u32 {
    48_000
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mix {
    pub id: String,
    pub name: String,
    pub channels: u16,
    #[serde(default = "preferred_rate")]
    pub sample_rate_hz: u32,
    #[serde(default)]
    pub master: Gain,
    #[serde(default)]
    pub exclude_groups: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub source: String,
    pub mix: String,
    pub gain: f64,
    #[serde(default)]
    pub mute: bool,
}
impl Send {
    fn level(&self) -> Gain {
        Gain {
            gain: self.gain,
            mute: self.mute,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Target {
    Device {
        #[serde(rename = "deviceUid")]
        device_uid: String,
    },
    VirtualInput {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
}
impl Target {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Device { device_uid } => nonempty("output deviceUid", device_uid),
            Self::VirtualInput { endpoint_id } => nonempty("endpointId", endpoint_id),
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputRoute {
    pub id: String,
    pub mix: String,
    pub target: Target,
    pub gain: f64,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub allow_mic_monitor: bool,
}
impl OutputRoute {
    fn level(&self) -> Gain {
        Gain {
            gain: self.gain,
            mute: self.mute,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MediaPolicy {
    OsDefault,
    Primary {
        #[serde(rename = "applicationId")]
        application_id: String,
        #[serde(default, rename = "sessionId")]
        session_id: Option<String>,
    },
    AllRoutedPlayers,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceMediaPolicy {
    pub device_uid: String,
    pub policy: MediaPolicy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub revision: u64,
    pub sources: Vec<Source>,
    pub mixes: Vec<Mix>,
    pub sends: Vec<Send>,
    pub outputs: Vec<OutputRoute>,
    #[serde(default)]
    pub media_controls: Vec<DeviceMediaPolicy>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Evidence {
    Simulated,
    NativeDiscovery,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Windows,
    Macos,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    External,
    MixyardOutput,
    MixyardProducer,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioFormat {
    pub channels: u16,
    pub sample_rate_hz: u32,
}
impl AudioFormat {
    fn validate(self) -> Result<(), String> {
        if !matches!(self.channels, 1 | 2) || self.sample_rate_hz == 0 {
            return Err(format!("unsupported-format ({} channels, {} Hz); select supported mono/stereo with a positive sample rate", self.channels, self.sample_rate_hz));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub observed_id: String,
    pub selector: Selector,
    pub origin: Origin,
    #[serde(default)]
    pub format: Option<AudioFormat>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputObservation {
    pub observed_id: String,
    pub target: Target,
    #[serde(default)]
    pub format: Option<AudioFormat>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum MediaAction {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
    #[default]
    Unknown,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaSession {
    pub session_id: String,
    pub application_id: String,
    #[serde(default)]
    pub playback_state: PlaybackState,
    /// None means unknown; [] explicitly means no actions supported.
    #[serde(default)]
    pub supported_actions: Option<Vec<MediaAction>>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendLimits {
    pub max_exported_mixes: Option<usize>,
    pub max_output_routes: Option<usize>,
    pub max_virtual_inputs: Option<usize>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inventory {
    pub schema_version: u32,
    pub revision: u64,
    pub platform: Platform,
    pub evidence: Evidence,
    pub sources: Vec<Observation>,
    pub outputs: Vec<OutputObservation>,
    #[serde(default)]
    pub media_sessions: Vec<MediaSession>,
    #[serde(default)]
    pub limits: BackendLimits,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionStatus {
    Selected,
    Missing,
    Ambiguous,
    UnsupportedFormat,
}
fn status(count: usize) -> ResolutionStatus {
    match count {
        0 => ResolutionStatus::Missing,
        1 => ResolutionStatus::Selected,
        _ => ResolutionStatus::Ambiguous,
    }
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Diagnostic {
    pub subject: String,
    pub code: &'static str,
    pub message: String,
}
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub source: String,
    pub selector: Selector,
    pub status: ResolutionStatus,
    pub observed_ids: Vec<String>,
    pub format: Option<AudioFormat>,
}
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MixPlan {
    pub id: String,
    pub name: String,
    pub channels: u16,
    pub sample_rate_hz: u32,
    pub master: Gain,
    pub exclude_groups: Vec<String>,
    pub peak_bound: f64,
}
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlannedSend {
    pub source: String,
    pub mix: String,
    pub gain: f64,
    pub mute: bool,
    pub observed_id: Option<String>,
    pub effective_gain: f64,
    pub channel_mapping: &'static str,
    pub rate_conversion_required: Option<bool>,
}
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlannedOutput {
    pub id: String,
    pub mix: String,
    pub target: Target,
    pub gain: f64,
    pub mute: bool,
    pub allow_mic_monitor: bool,
    pub status: ResolutionStatus,
    pub observed_ids: Vec<String>,
    /// Coefficient relative to an already-mastered mix, zero if unresolved.
    pub effective_gain: f64,
    pub peak_bound: f64,
    pub channel_mapping: &'static str,
    pub rate_conversion_required: Option<bool>,
}
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub schema_version: u32,
    pub mode: &'static str,
    pub config_revision: u64,
    pub inventory_revision: u64,
    pub inventory_evidence: Evidence,
    pub platform: Platform,
    pub mixes: Vec<MixPlan>,
    pub sources: Vec<Resolution>,
    pub sends: Vec<PlannedSend>,
    pub outputs: Vec<PlannedOutput>,
    pub diagnostics: Vec<Diagnostic>,
}

fn nonempty(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} must not be blank"));
    }
    Ok(())
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 2 {
            return Err(
                "unsupported config schemaVersion (expected 2); migrate fixed-bus v1 explicitly"
                    .into(),
            );
        }
        let mut sources = BTreeMap::new();
        let mut selectors = BTreeSet::new();
        let mut apps = BTreeSet::new();
        let mut returns = BTreeSet::new();
        for source in &self.sources {
            nonempty("source id", &source.id)?;
            source.selector.validate()?;
            if sources.insert(&source.id, &source.selector).is_some() {
                return Err(format!("duplicate source id: {}", source.id));
            }
            if !selectors.insert(&source.selector) {
                return Err(format!("duplicate source selector: {}", source.id));
            }
            match &source.selector {
                Selector::App { application_id } => {
                    apps.insert(application_id);
                }
                Selector::CallReturn { application_id, .. } => {
                    returns.insert(application_id);
                }
                _ => {}
            }
        }
        if !apps.is_disjoint(&returns) {
            return Err("call-return application cannot also be ordinary app/media".into());
        }
        let mut mixes = BTreeMap::new();
        for mix in &self.mixes {
            nonempty("mix id", &mix.id)?;
            nonempty("mix name", &mix.name)?;
            AudioFormat {
                channels: mix.channels,
                sample_rate_hz: mix.sample_rate_hz,
            }
            .validate()?;
            mix.master.validate()?;
            if mixes.insert(&mix.id, mix).is_some() {
                return Err(format!("duplicate mix id: {}", mix.id));
            }
            let mut groups = BTreeSet::new();
            for group in &mix.exclude_groups {
                nonempty("excludeGroups entry", group)?;
                if !groups.insert(group) {
                    return Err(format!("{}: duplicate exclusion group", mix.id));
                }
            }
        }
        let mut sends = BTreeSet::new();
        for send in &self.sends {
            let source = sources
                .get(&send.source)
                .ok_or_else(|| format!("unknown send source: {}", send.source))?;
            let mix = mixes
                .get(&send.mix)
                .ok_or_else(|| format!("unknown send mix: {}", send.mix))?;
            send.level().validate()?;
            if !sends.insert((&send.source, &send.mix)) {
                return Err("duplicate source->mix send".into());
            }
            if let Selector::CallReturn { group, .. } = source {
                if mix.exclude_groups.contains(group) {
                    return Err(format!(
                        "{}: excluded call-return group {group} cannot feed this mix (mix-minus)",
                        mix.id
                    ));
                }
            }
        }
        for mix in &self.mixes {
            let peak = self.peak_bound(mix);
            if peak > 1.0 {
                return Err(format!("mix {} peak bound {peak} exceeds headroom 1; lower this mix's sends or master, or mute a send", mix.id));
            }
        }
        let mut output_ids = BTreeSet::new();
        let mut targets = BTreeSet::new();
        for output in &self.outputs {
            nonempty("output id", &output.id)?;
            output.target.validate()?;
            output.level().validate()?;
            let mix = mixes
                .get(&output.mix)
                .ok_or_else(|| format!("unknown output mix: {}", output.mix))?;
            if !output_ids.insert(&output.id) {
                return Err("duplicate output id".into());
            }
            if !targets.insert(&output.target) {
                return Err("converging output routes unsupported; select one mix per physical device/virtual input (complete channel layout)".into());
            }
            if matches!(output.target, Target::Device { .. })
                && !output.allow_mic_monitor
                && self.sends.iter().any(|send| {
                    send.mix == mix.id && matches!(sources[&send.source], Selector::Mic { .. })
                })
            {
                return Err(format!("output {} needs explicit allowMicMonitor consent for microphone monitoring, regardless of mix name or current mute", output.id));
            }
            let peak = self.peak_bound(mix) * output.level().amplitude();
            if peak > 1.0 {
                return Err(format!(
                    "output {} peak bound {peak} exceeds headroom 1; lower this output gain",
                    output.id
                ));
            }
        }
        let mut policies = BTreeSet::new();
        for control in &self.media_controls {
            nonempty("media deviceUid", &control.device_uid)?;
            if !policies.insert(&control.device_uid) {
                return Err("duplicate device media policy".into());
            }
            if let MediaPolicy::Primary {
                application_id,
                session_id,
            } = &control.policy
            {
                nonempty("primary applicationId", application_id)?;
                if let Some(session) = session_id {
                    nonempty("primary sessionId", session)?;
                }
            }
        }
        Ok(())
    }
    fn peak_bound(&self, mix: &Mix) -> f64 {
        let mut sends: Vec<_> = self
            .sends
            .iter()
            .filter(|send| send.mix == mix.id)
            .collect();
        sends.sort_by(|a, b| a.source.cmp(&b.source));
        mix.master.amplitude()
            * sends
                .iter()
                .map(|send| send.level().amplitude())
                .sum::<f64>()
    }
}
fn diagnostic(
    diagnostics: &mut Vec<Diagnostic>,
    subject: &str,
    code: &'static str,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic {
        subject: subject.into(),
        code,
        message: message.into(),
    });
}
fn resolution_diagnostic(
    diagnostics: &mut Vec<Diagnostic>,
    subject: &str,
    status: ResolutionStatus,
    format: Option<AudioFormat>,
) -> Result<(), String> {
    match status {
        ResolutionStatus::Missing => diagnostic(diagnostics, subject, "missing-target", "Disconnected: exact identity absent. Reconnect it or edit its stable selector explicitly; no fallback."),
        ResolutionStatus::Ambiguous => diagnostic(diagnostics, subject, "ambiguous-target", "Disconnected: multiple exact matches. Supply a unique stable identity; no fallback."),
        ResolutionStatus::UnsupportedFormat => diagnostic(diagnostics, subject, "unsupported-format", "Disconnected: observed output format unsupported; select mono/stereo with a positive sample rate. Healthy outputs remain planned."),
        ResolutionStatus::Selected => match format {
            None => diagnostic(diagnostics, subject, "format-unverified", "Format unknown. Discover supported mono/stereo before native execution; dry-run proves no audio quality."),
            Some(format) => format.validate().map_err(|error| format!("{subject}: {error}"))?,
        }
    }
    Ok(())
}
fn mapping(from: Option<u16>, to: u16) -> &'static str {
    match (from, to) {
        (None, _) => "unverified",
        (Some(1), 1) => "mono",
        (Some(2), 2) => "stereo",
        (Some(1), 2) => "duplicate-mono",
        (Some(2), 1) => "average-left-right",
        _ => unreachable!("formats validated"),
    }
}
fn capacity(
    config: &Config,
    inventory: &Inventory,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), String> {
    let virtuals = config
        .outputs
        .iter()
        .filter(|output| matches!(output.target, Target::VirtualInput { .. }))
        .count();
    let exported_mixes = config
        .outputs
        .iter()
        .map(|output| &output.mix)
        .collect::<BTreeSet<_>>()
        .len();
    for (name, requested, maximum) in [
        (
            "exported mixes",
            exported_mixes,
            inventory.limits.max_exported_mixes,
        ),
        (
            "output routes",
            config.outputs.len(),
            inventory.limits.max_output_routes,
        ),
        (
            "virtual inputs",
            virtuals,
            inventory.limits.max_virtual_inputs,
        ),
    ] {
        match maximum {
            Some(limit) if requested > limit => return Err(format!("backend-capacity: requested {requested} {name}, observed maximum {limit}; reduce exports or choose a capable backend; no partial plan or truncation")),
            None => diagnostic(diagnostics, "backend", "capacity-unverified", format!("{name} capacity unknown; native execution/export must preflight resources before apply.")),
            _ => {}
        }
    }
    Ok(())
}
fn sessions(inventory: &Inventory) -> Result<BTreeMap<String, MediaSession>, String> {
    let mut sessions = BTreeMap::new();
    for observed in &inventory.media_sessions {
        nonempty("sessionId", &observed.session_id)?;
        nonempty("session applicationId", &observed.application_id)?;
        let mut session = observed.clone();
        if let Some(actions) = &mut session.supported_actions {
            actions.sort();
            actions.dedup();
        }
        if let Some(previous) = sessions.insert(session.session_id.clone(), session.clone()) {
            if previous != session {
                return Err(format!("conflicting media session {}", session.session_id));
            }
        }
    }
    Ok(sessions)
}

pub fn plan(config: &Config, inventory: &Inventory) -> Result<Plan, String> {
    config.validate()?;
    if inventory.schema_version != 2 {
        return Err("unsupported inventory schemaVersion (expected 2)".into());
    }
    let mut diagnostics = Vec::new();
    capacity(config, inventory, &mut diagnostics)?;
    sessions(inventory)?;
    let mut observations = BTreeMap::new();
    for observation in &inventory.sources {
        nonempty("observed source id", &observation.observed_id)?;
        observation.selector.validate()?;
        if let Some(previous) = observations.insert(&observation.observed_id, observation) {
            if previous != observation {
                return Err(format!(
                    "conflicting source observations for {}",
                    observation.observed_id
                ));
            }
        }
    }
    let mut output_observations = BTreeMap::new();
    for observation in &inventory.outputs {
        nonempty("observed output id", &observation.observed_id)?;
        observation.target.validate()?;
        if let Some(previous) = output_observations.insert(&observation.observed_id, observation) {
            if previous != observation {
                return Err(format!(
                    "conflicting output observations for {}",
                    observation.observed_id
                ));
            }
        }
    }
    let mut desired_sources: Vec<_> = config.sources.iter().collect();
    desired_sources.sort_by(|a, b| a.id.cmp(&b.id));
    let mut sources = Vec::new();
    for source in desired_sources {
        let matches: Vec<_> = observations
            .values()
            .filter(|observed| observed.selector == source.selector)
            .collect();
        if matches
            .iter()
            .any(|observed| observed.origin != Origin::External)
        {
            return Err(format!(
                "{}: Mixyard output/producer recapture would create feedback",
                source.id
            ));
        }
        if let Selector::App { application_id } = &source.selector {
            if observations.values().any(|observed| {
                matches!(&observed.selector,
                Selector::CallReturn { application_id: app, .. } if app == application_id)
            }) {
                return Err(format!(
                    "{}: call-return cannot be relabeled as app/media",
                    source.id
                ));
            }
        }
        let status = status(matches.len());
        let format = if status == ResolutionStatus::Selected {
            matches[0].format
        } else {
            None
        };
        resolution_diagnostic(
            &mut diagnostics,
            &format!("source:{}", source.id),
            status,
            format,
        )?;
        sources.push(Resolution {
            source: source.id.clone(),
            selector: source.selector.clone(),
            status,
            observed_ids: matches
                .iter()
                .map(|observed| observed.observed_id.clone())
                .collect(),
            format,
        });
    }
    let resolved: BTreeMap<_, _> = sources
        .iter()
        .map(|source| (&source.source, source))
        .collect();
    let mixes: BTreeMap<_, _> = config.mixes.iter().map(|mix| (&mix.id, mix)).collect();
    let mut sends = Vec::new();
    let mut desired_sends: Vec<_> = config.sends.iter().collect();
    desired_sends.sort_by(|a, b| (&a.source, &a.mix).cmp(&(&b.source, &b.mix)));
    for send in desired_sends {
        let source = resolved[&send.source];
        let mix = mixes[&send.mix];
        let connected = source.status == ResolutionStatus::Selected;
        let rate_conversion = source
            .format
            .map(|format| format.sample_rate_hz != mix.sample_rate_hz);
        if rate_conversion == Some(true) {
            diagnostic(&mut diagnostics, &format!("send:{}->{}", send.source, send.mix), "rate-conversion-required", "Negotiate native rate conversion/clock adaptation; this planner performs no conversion.");
        }
        sends.push(PlannedSend {
            source: send.source.clone(),
            mix: send.mix.clone(),
            gain: send.gain,
            mute: send.mute,
            observed_id: if connected {
                Some(source.observed_ids[0].clone())
            } else {
                None
            },
            effective_gain: if connected {
                send.level().amplitude() * mix.master.amplitude()
            } else {
                0.0
            },
            channel_mapping: if connected {
                mapping(source.format.map(|format| format.channels), mix.channels)
            } else {
                "disconnected"
            },
            rate_conversion_required: rate_conversion,
        });
    }
    let mut outputs = Vec::new();
    let mut desired_outputs: Vec<_> = config.outputs.iter().collect();
    desired_outputs.sort_by(|a, b| a.id.cmp(&b.id));
    for output in desired_outputs {
        let mix = mixes[&output.mix];
        let matches: Vec<_> = output_observations
            .values()
            .filter(|observed| observed.target == output.target)
            .collect();
        let status = status(matches.len());
        let format = if status == ResolutionStatus::Selected {
            matches[0].format
        } else {
            None
        };
        // An observed device limitation degrades only this output; configured
        // formats and backend capacities are still validated as hard errors.
        let status = if format.is_some_and(|format| format.validate().is_err()) {
            ResolutionStatus::UnsupportedFormat
        } else {
            status
        };
        resolution_diagnostic(
            &mut diagnostics,
            &format!("output:{}", output.id),
            status,
            format,
        )?;
        let rate_conversion = if status == ResolutionStatus::Selected {
            format.map(|format| format.sample_rate_hz != mix.sample_rate_hz)
        } else {
            None
        };
        if rate_conversion == Some(true) {
            diagnostic(&mut diagnostics, &format!("output:{}", output.id), "rate-conversion-required", "Negotiate native output conversion and independent clock adaptation before execution.");
        }
        outputs.push(PlannedOutput {
            id: output.id.clone(),
            mix: output.mix.clone(),
            target: output.target.clone(),
            gain: output.gain,
            mute: output.mute,
            allow_mic_monitor: output.allow_mic_monitor,
            status,
            observed_ids: matches
                .iter()
                .map(|observed| observed.observed_id.clone())
                .collect(),
            effective_gain: if status == ResolutionStatus::Selected {
                output.level().amplitude()
            } else {
                0.0
            },
            peak_bound: config.peak_bound(mix) * output.level().amplitude(),
            channel_mapping: if status != ResolutionStatus::Selected {
                "disconnected"
            } else if let Some(format) = format {
                mapping(Some(mix.channels), format.channels)
            } else {
                "unverified"
            },
            rate_conversion_required: rate_conversion,
        });
    }
    Ok(Plan {
        schema_version: 2,
        mode: "dry-run",
        config_revision: config.revision,
        inventory_revision: inventory.revision,
        inventory_evidence: inventory.evidence,
        platform: inventory.platform,
        mixes: mixes
            .values()
            .map(|mix| {
                let mut groups = mix.exclude_groups.clone();
                groups.sort();
                MixPlan {
                    id: mix.id.clone(),
                    name: mix.name.clone(),
                    channels: mix.channels,
                    sample_rate_hz: mix.sample_rate_hz,
                    master: mix.master,
                    exclude_groups: groups,
                    peak_bound: config.peak_bound(mix),
                }
            })
            .collect(),
        sources,
        sends,
        outputs,
        diagnostics,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OriginAttribution {
    DeviceAttributed,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DispatchOwnership {
    Mixyard,
    Os,
    Unknown,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaRequest {
    pub schema_version: u32,
    pub device_uid: Option<String>,
    pub action: MediaAction,
    pub origin_attribution: OriginAttribution,
    pub dispatch_ownership: DispatchOwnership,
    #[serde(default)]
    pub remembered_playing_session_ids: Vec<String>,
    /// Native callers copy rememberOnConfirmedPauseContext here and only the
    /// rememberOnConfirmedPause IDs whose Pause outcomes they confirmed into
    /// rememberedPlayingSessionIds. A dry-run result alone confirms nothing.
    /// Legacy raw IDs have no continuity evidence and cannot resume.
    #[serde(default)]
    pub remembered_playing_context: Option<PauseHistoryContext>,
}

/// Versioned, caller-held history context; this pure planner stores no history.
/// Every desired config edit must advance revision, including remove/restore.
/// Revision must never be reused for a different config or rolled back. Native
/// callers must discard history across config replacement/reset. Inventory
/// revision is deliberately excluded: paused capture streams may disappear.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PauseHistoryContext {
    pub schema_version: u32,
    pub device_uid: String,
    pub config_revision: u64,
    pub policy: MediaPolicy,
    pub routed_application_ids: Vec<String>,
}
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlayerDecision {
    pub session_id: String,
    pub application_id: String,
    pub playback_state: PlaybackState,
    pub planned_action: Option<MediaAction>,
    pub decision: &'static str,
    pub dispatch_outcome: &'static str,
    pub affected_device_uids: Vec<String>,
}
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaPlan {
    pub schema_version: u32,
    pub mode: &'static str,
    pub config_revision: u64,
    pub inventory_revision: u64,
    pub inventory_evidence: Evidence,
    pub device_uid: Option<String>,
    pub requested_action: MediaAction,
    pub status: &'static str,
    pub message: &'static str,
    pub scope: &'static str,
    pub device_only_quiet_hint: &'static str,
    pub retry_policy: &'static str,
    pub players: Vec<PlayerDecision>,
    pub remembered_playing_session_ids: Vec<String>,
    pub remembered_playing_context: Option<PauseHistoryContext>,
    /// Candidates only: no native outcome is confirmed by this planner.
    pub remember_on_confirmed_pause: Vec<String>,
    pub remember_on_confirmed_pause_context: Option<PauseHistoryContext>,
    pub unavailable_remembered_session_ids: Vec<String>,
    pub routing_diagnostics: Vec<Diagnostic>,
}

fn routed_apps(config: &Config, routing: &Plan, device: &str) -> BTreeSet<String> {
    let mixes: BTreeSet<_> = routing
        .outputs
        .iter()
        .filter(|output| {
            output.effective_gain > 0.0
                && matches!(&output.target, Target::Device { device_uid } if device_uid == device)
        })
        .map(|output| &output.mix)
        .collect();
    let sources: BTreeMap<_, _> = config
        .sources
        .iter()
        .map(|source| (&source.id, &source.selector))
        .collect();
    let masters: BTreeMap<_, _> = config
        .mixes
        .iter()
        .map(|mix| (&mix.id, mix.master.amplitude()))
        .collect();
    config
        .sends
        .iter()
        .filter(|send| {
            mixes.contains(&send.mix) && send.level().amplitude() * masters[&send.mix] > 0.0
        })
        .filter_map(|send| match sources[&send.source] {
            Selector::App { application_id } => Some(application_id.clone()),
            _ => None,
        })
        .collect()
}

pub fn plan_media(
    config: &Config,
    inventory: &Inventory,
    request: &MediaRequest,
) -> Result<MediaPlan, String> {
    if request.schema_version != 2 {
        return Err("unsupported media request schemaVersion (expected 2)".into());
    }
    let routing = plan(config, inventory)?;
    let mut memory = request.remembered_playing_session_ids.clone();
    memory.sort();
    memory.dedup();
    let mut result = MediaPlan { schema_version: 2, mode: "dry-run-media-control", config_revision: config.revision,
        inventory_revision: inventory.revision, inventory_evidence: inventory.evidence, device_uid: request.device_uid.clone(),
        requested_action: request.action, status: "unknown-origin", message: "No attributable origin device; no broadcast or fallback.",
        scope: "global-media-session", device_only_quiet_hint: "Player pause affects every device sharing the player; device-only quiet requires an output mute/gate.",
        retry_policy: if matches!(request.action, MediaAction::Next | MediaAction::Previous) { "never-automatically-retry" } else { "reobserve-outcomes-before-retry" },
        players: Vec::new(), remembered_playing_session_ids: Vec::new(), remembered_playing_context: None,
        remember_on_confirmed_pause: Vec::new(), remember_on_confirmed_pause_context: None,
        unavailable_remembered_session_ids: memory.clone(), routing_diagnostics: routing.diagnostics.clone() };
    let Some(device) = request
        .device_uid
        .as_deref()
        .filter(|id| !id.trim().is_empty())
    else {
        return Ok(result);
    };
    if request.origin_attribution != OriginAttribution::DeviceAttributed {
        return Ok(result);
    }
    if request.dispatch_ownership != DispatchOwnership::Mixyard {
        result.status = "dispatch-not-owned";
        result.message = "Exclusive dispatch ownership unavailable; leave OS handling untouched, no second dispatch.";
        return Ok(result);
    }
    let policy = config
        .media_controls
        .iter()
        .find(|policy| policy.device_uid == device)
        .map(|control| &control.policy);
    if policy.is_none() || matches!(policy, Some(MediaPolicy::OsDefault)) {
        result.status = "os-default";
        result.message =
            "Device-scoped control is off; normal OS session handling retains ownership.";
        return Ok(result);
    }
    let output = routing.outputs.iter().find(
        |output| matches!(&output.target, Target::Device { device_uid } if device_uid == device),
    );
    let Some(output) = output else {
        result.status = "unknown-device";
        result.message = "Origin has no configured physical output; no broadcast or fallback.";
        return Ok(result);
    };
    if output.status != ResolutionStatus::Selected {
        result.status = "device-disconnected";
        result.message =
            "Origin output is missing/ambiguous or has an unsupported format; no player fallback.";
        return Ok(result);
    }
    let apps = routed_apps(config, &routing, device);
    let sessions = sessions(inventory)?;
    let mut selected: Vec<_> = sessions
        .values()
        .filter(|session| apps.contains(&session.application_id))
        .collect();
    if let Some(MediaPolicy::Primary {
        application_id,
        session_id,
    }) = policy
    {
        selected.retain(|session| {
            &session.application_id == application_id
                && session_id
                    .as_ref()
                    .is_none_or(|id| id == &session.session_id)
        });
        if selected.len() != 1 {
            result.status = if selected.is_empty() {
                "missing-primary"
            } else {
                "ambiguous-primary"
            };
            result.message = "Primary app/session absent, unrouted or ambiguous; select an exact primary, no fallback.";
            return Ok(result);
        }
    }
    if selected.is_empty() {
        result.status = "no-routed-players";
        result.message =
            "No observed media sessions belong to enabled configured device routes; no fallback.";
        return Ok(result);
    }
    let context = PauseHistoryContext {
        schema_version: 1,
        device_uid: device.into(),
        config_revision: config.revision,
        policy: policy.expect("enabled policy checked above").clone(),
        routed_application_ids: apps.into_iter().collect(),
    };
    if request.remembered_playing_context.as_ref() == Some(&context) {
        let (available, unavailable) = memory
            .into_iter()
            .partition(|id| selected.iter().any(|session| &session.session_id == id));
        result.remembered_playing_session_ids = available;
        result.unavailable_remembered_session_ids = unavailable;
        if !result.remembered_playing_session_ids.is_empty() {
            result.remembered_playing_context = Some(context.clone());
        }
    }
    let devices: BTreeSet<_> = routing
        .outputs
        .iter()
        .filter_map(|output| match &output.target {
            Target::Device { device_uid } => Some(device_uid.as_str()),
            _ => None,
        })
        .collect();
    for session in &selected {
        result.players.push(PlayerDecision {
            session_id: session.session_id.clone(),
            application_id: session.application_id.clone(),
            playback_state: session.playback_state,
            planned_action: None,
            decision: "not-selected-for-action",
            dispatch_outcome: "not-dispatched",
            affected_device_uids: devices
                .iter()
                .filter(|device| {
                    routed_apps(config, &routing, device).contains(&session.application_id)
                })
                .map(|id| (*id).into())
                .collect(),
        });
    }
    let mut action = request.action;
    let mut candidates: BTreeSet<_> = selected
        .iter()
        .map(|session| session.session_id.as_str())
        .collect();
    if request.action == MediaAction::PlayPause {
        if selected
            .iter()
            .any(|session| session.playback_state == PlaybackState::Unknown)
        {
            result.status = "unknown-playback-state";
            result.message = "Group PlayPause needs known states; observe each player before resolving one explicit group action.";
            for player in &mut result.players {
                player.decision = if player.playback_state == PlaybackState::Unknown {
                    "unknown-playback-state"
                } else {
                    "group-blocked"
                };
            }
            return Ok(result);
        }
        if selected
            .iter()
            .any(|session| session.playback_state == PlaybackState::Playing)
        {
            action = MediaAction::Pause;
            candidates.retain(|id| {
                selected.iter().any(|session| {
                    session.session_id == *id && session.playback_state == PlaybackState::Playing
                })
            });
        } else {
            action = MediaAction::Play;
            candidates.retain(|id| {
                result
                    .remembered_playing_session_ids
                    .iter()
                    .any(|remembered| remembered == id)
            });
            if candidates.is_empty() {
                result.status = "no-resume-history";
                result.message = "No remembered previously-playing sessions have matching device/config/policy context and current membership; use explicit Play, no fallback.";
                return Ok(result);
            }
            if selected.iter().any(|session| {
                candidates.contains(session.session_id.as_str())
                    && session.playback_state != PlaybackState::Paused
            }) {
                result.status = "resume-state-changed";
                result.message = "Remembered session no longer paused; reobserve or choose explicit Play, no automatic restart.";
                for player in &mut result.players {
                    player.decision = "resume-state-changed";
                }
                return Ok(result);
            }
        }
    }
    let mut blocked = false;
    for (session, player) in selected.iter().zip(&mut result.players) {
        if !candidates.contains(session.session_id.as_str()) {
            continue;
        }
        player.decision = match &session.supported_actions {
            None => {
                blocked = true;
                "unknown-capability"
            }
            Some(actions) if !actions.contains(&action) => {
                blocked = true;
                "unsupported-action"
            }
            Some(_) => "planned",
        };
    }
    if blocked {
        result.status = "capability-blocked";
        result.message = "A target lacks known support for the explicit action; no partial group dispatch is planned.";
        for player in &mut result.players {
            if player.decision == "planned" {
                player.decision = "group-blocked";
            }
        }
        return Ok(result);
    }
    for player in &mut result.players {
        if player.decision == "planned" {
            player.planned_action = Some(action);
            if action == MediaAction::Pause && player.playback_state == PlaybackState::Playing {
                result
                    .remember_on_confirmed_pause
                    .push(player.session_id.clone());
            }
        }
    }
    if !result.remember_on_confirmed_pause.is_empty() {
        result.remember_on_confirmed_pause_context = Some(context);
    }
    result.status = "ready";
    result.message = "Session-global actions resolved from desired routes and observations; no action dispatched, native outcomes remain unknown.";
    Ok(result)
}

#[cfg(test)]
mod tests;
