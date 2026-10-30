//! Closed owned-private-generated control protocol. Never lifecycle Availability.
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const SCOPE: &str = "owned-private-generated-v1";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Scope {
    #[serde(rename = "owned-private-generated-v1")]
    OwnedPrivateGeneratedV1,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Generator {
    pub source_id: String,
    pub left: f64,
    pub right: f64,
    pub marker_period: u32,
    pub frame_limit: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrivateLimits {
    pub max_sources: u32,
    pub max_outputs: u32,
    pub max_resident_bytes: u64,
    pub max_frames: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Descriptor {
    pub schema_version: u32,
    pub scope: Scope,
    pub sdk_prefix: std::path::PathBuf,
    pub prefix_manifest: std::path::PathBuf,
    pub prefix_manifest_sha256: String,
    pub companion_executable: std::path::PathBuf,
    pub companion_sha256: String,
    pub sdk_version: String,
    pub sdk_commit: String,
    pub generators: Vec<Generator>,
    pub limits: PrivateLimits,
}
pub const REQUEST_LIMIT: usize = 8192;
pub const RESULT_LIMIT: usize = 65536;
pub const COMPANION_LIMIT: usize = 1024 * 1024;
pub const INSTALL_MS: u64 = 2000;
pub const FENCE_MS: u64 = 2000;
pub const TERM_MS: u64 = 1000;
pub const KILL_MS: u64 = 1000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoint {
    pub node_id: u32,
    pub node_serial: u64,
    pub port_id: u32,
    pub port_serial: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingInstall {
    pub operation_id: String,
    pub reserved_generation: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StopTarget {
    pub instance_nonce: String,
    pub control_epoch: u64,
    #[serde(deserialize_with = "required_nullable")]
    pub active_generation: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub pending_install: Option<PendingInstall>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub private_audio_version: u32,
    pub request_id: String,
    pub operation: Operation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Operation {
    Status,
    #[serde(rename_all = "camelCase")]
    Prepare {
        instance_nonce: String,
        expected_store_revision: u64,
        expected_config_revision: u64,
    },
    #[serde(rename_all = "camelCase")]
    Link {
        instance_nonce: String,
        prepared_token: String,
        output_id: String,
        consumer: Endpoint,
    },
    #[serde(rename_all = "camelCase")]
    Apply {
        instance_nonce: String,
        prepared_token: String,
        expected_store_revision: u64,
        expected_config_revision: u64,
        expected_inventory_revision: u64,
    },
    Stop {
        expected: StopTarget,
    },
}

impl Request {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.private_audio_version != VERSION {
            return Err("unsupported-private-version");
        }
        if self.request_id.is_empty() || self.request_id.len() > 64 {
            return Err("protocol-error");
        }
        let nonce = match &self.operation {
            Operation::Status => return Ok(()),
            Operation::Prepare { instance_nonce, .. }
            | Operation::Link { instance_nonce, .. }
            | Operation::Apply { instance_nonce, .. } => instance_nonce,
            Operation::Stop { expected } => &expected.instance_nonce,
        };
        if !hex_digest(nonce) {
            return Err("protocol-error");
        }
        Ok(())
    }
}

pub fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Idle,
    Preparing,
    Prepared,
    Installing,
    Running,
    Stopping,
    Stopped,
    FailedHeld,
    FailedStopped,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeFormat {
    pub encoding: String,
    pub sample_rate_hz: u32,
    pub channels: Vec<String>,
    pub frames: u32,
}

impl NativeFormat {
    pub fn supported(&self) -> bool {
        self.encoding == "f64-le"
            && self.sample_rate_hz == 48000
            && self.channels == ["FL", "FR"]
            && self.frames == 128
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishedOutput {
    pub output_id: String,
    pub mix_id: String,
    pub label: String,
    pub endpoint: Endpoint,
    pub format: NativeFormat,
    pub linked_consumer: Option<Endpoint>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputCommit {
    pub output_id: String,
    pub native_frame: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Installed {
    pub instance_nonce: String,
    pub control_epoch: u64,
    pub operation_id: String,
    pub generation: u64,
    pub store_revision: u64,
    pub config_revision: u64,
    pub inventory_revision: u64,
    pub request_digest: String,
    pub endpoint_set_digest: String,
    pub clock_id: u64,
    pub native_switch_frame: u64,
    pub engine_first_frame: u64,
    pub first_processed_frame: u64,
    pub output_commits: Vec<OutputCommit>,
    pub owner_install_terminal: bool,
    pub no_late_install: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FenceOutcome {
    CanceledBeforeSwitch,
    AbortedAfterSwitch,
    NoPending,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SwitchEvidence {
    pub instance_nonce: String,
    pub control_epoch: u64,
    pub operation_id: String,
    pub generation: u64,
    pub config_revision: u64,
    pub inventory_revision: u64,
    pub clock_id: u64,
    pub native_switch_frame: u64,
}

pub fn required_nullable<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(de)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fenced {
    pub instance_nonce: String,
    pub control_epoch: u64,
    pub operation_id: String,
    pub outcome: FenceOutcome,
    #[serde(deserialize_with = "required_nullable")]
    pub switched: Option<Installed>,
    #[serde(deserialize_with = "required_nullable")]
    pub switch_evidence: Option<SwitchEvidence>,
    pub owner_quiesced: bool,
    pub pending_pcm_zero: bool,
    pub no_late_install: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Status {
    pub scope: Scope,
    pub phase: Phase,
    pub stop_target: StopTarget,
    pub desired_store_revision: Option<u64>,
    pub desired_config_revision: Option<u64>,
    pub inventory_revision: Option<u64>,
    pub prepared_token: Option<String>,
    pub installed: Option<Installed>,
    pub outputs: Vec<PublishedOutput>,
    pub pending_operation: Option<String>,
    pub last_error: Option<Failure>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    Disabled,
    Busy,
    StaleStore,
    StaleConfig,
    StaleInventory,
    StaleGeneration,
    StaleInstance,
    StaleControlEpoch,
    UnsupportedPrivateTarget,
    UnsupportedPrivateFormat,
    UnqualifiedSdk,
    EndpointChanged,
    ClockUnqualified,
    OwnerFailed,
    StopIncomplete,
    StopRequired,
    ResourceLimit,
    ProtocolError,
    SourceLost,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ResultValue {
    Status {
        status: Status,
    },
    Prepared {
        status: Status,
    },
    Linked {
        status: Status,
    },
    Installed {
        receipt: Installed,
        status: Status,
    },
    #[serde(rename_all = "camelCase")]
    Pending {
        operation_id: String,
        status: Status,
    },
    #[serde(rename_all = "camelCase")]
    StopComplete {
        receipt: Fenced,
        children_reaped: bool,
        status: Status,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reply {
    pub private_audio_version: u32,
    pub request_id: String,
    pub result: Option<ResultValue>,
    pub error: Option<Failure>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesiredSnapshot {
    pub store_revision: u64,
    pub config_revision: u64,
    pub config_sha256: String,
    pub config: crate::Config,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompanionRequest {
    pub companion_version: u32,
    pub instance_nonce: String,
    pub control_epoch: u64,
    pub operation_id: String,
    pub action: CompanionAction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CompanionAction {
    #[serde(rename_all = "camelCase")]
    Prepare {
        generation: u64,
        snapshot: DesiredSnapshot,
    },
    #[serde(rename_all = "camelCase")]
    Link {
        prepared_token: String,
        output_id: String,
        consumer: Endpoint,
    },
    #[serde(rename_all = "camelCase")]
    Stage {
        prepared_token: String,
        expected_inventory_revision: u64,
    },
    #[serde(rename_all = "camelCase")]
    Install {
        staged_token: String,
    },
    #[serde(rename_all = "camelCase")]
    Fence {
        expected: StopTarget,
        new_control_epoch: u64,
    },
    Snapshot,
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompanionReply {
    pub companion_version: u32,
    pub instance_nonce: String,
    pub control_epoch: u64,
    pub operation_id: String,
    pub event: CompanionEvent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CompanionEvent {
    #[serde(rename_all = "camelCase")]
    Ready {
        inventory_revision: u64,
        endpoint_set_digest: String,
    },
    #[serde(rename_all = "camelCase")]
    Prepared {
        prepared_token: String,
        inventory_revision: u64,
        outputs: Vec<PublishedOutput>,
    },
    #[serde(rename_all = "camelCase")]
    Linked {
        inventory_revision: u64,
        outputs: Vec<PublishedOutput>,
    },
    #[serde(rename_all = "camelCase")]
    Staged {
        staged_token: String,
        generation: u64,
        config_revision: u64,
        inventory_revision: u64,
        request_digest: String,
        endpoint_set_digest: String,
    },
    Installed {
        receipt: Installed,
    },
    Fenced {
        receipt: Fenced,
    },
    Snapshot {
        status: Status,
    },
    Fault {
        error: Failure,
    },
    Stopped,
}
