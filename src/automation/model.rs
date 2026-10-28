use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ValueType {
    Bool,
    Number,
    Text,
    TextSet,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum Value {
    Bool(bool),
    Number(f64),
    Text(String),
    TextSet(Vec<String>),
}
impl Value {
    pub fn value_type(&self) -> ValueType {
        match self {
            Self::Bool(_) => ValueType::Bool,
            Self::Number(_) => ValueType::Number,
            Self::Text(_) => ValueType::Text,
            Self::TextSet(_) => ValueType::TextSet,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactKey {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Global,
    Entity,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EvidencePolicy {
    Any,
    UserDeclaration,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactDefinition {
    pub name: String,
    pub value_type: ValueType,
    pub scope: Scope,
    pub max_age_ms: u64,
    pub evidence_policy: EvidencePolicy,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FactStatus {
    Known,
    Unknown,
    Unsupported,
    Error,
    Ambiguous,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Evidence {
    PlatformObserved,
    UserDeclared,
    Derived,
    Fixture,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Completeness {
    Complete,
    Partial,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fact {
    pub key: FactKey,
    pub status: FactStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    pub provider: String,
    pub evidence: Evidence,
    pub observed_at_ms: Option<u64>,
    pub valid_for_ms: u64,
    pub completeness: Completeness,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub revision: u64,
    /// Opaque monotonic clock boot/session identity, shared by all fact timestamps.
    pub epoch: String,
    pub facts: Vec<Fact>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Predicate {
    Eq {
        value: Value,
    },
    Ne {
        value: Value,
    },
    Lt {
        value: f64,
    },
    Le {
        value: f64,
    },
    Gt {
        value: f64,
    },
    Ge {
        value: f64,
    },
    /// Inclusive bounds. Civil day/time ranges must be expressed using explicit provider facts.
    Range {
        min: f64,
        max: f64,
    },
    In {
        values: Vec<Value>,
    },
    Contains {
        value: String,
    },
    ContainsAll {
        values: Vec<String>,
    },
    ContainsAny {
        values: Vec<String>,
    },
    /// Inclusive cardinality bounds; sets contain unique exact identities.
    Count {
        min: u32,
        max: u32,
    },
    /// Known scalar, or nonempty known text set. Missing/unknown is never false.
    Present {},
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Condition {
    All { conditions: Vec<Condition> },
    Any { conditions: Vec<Condition> },
    Not { condition: Box<Condition> },
    Fact { key: FactKey, predicate: Predicate },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "scope", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Override {
    Send {
        source: String,
        mix: String,
        mute: Option<bool>,
        gain: Option<f64>,
    },
    Output {
        output: String,
        mute: Option<bool>,
        gain: Option<f64>,
    },
    Mix {
        mix: String,
        mute: Option<bool>,
        gain: Option<f64>,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Action {
    NoChange {},
    Select {
        #[serde(default, rename = "profileId", skip_serializing_if = "Option::is_none")]
        profile_id: Option<String>,
        overrides: Vec<Override>,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Node {
    Decision {
        id: String,
        condition: Condition,
        #[serde(rename = "onTrue")]
        on_true: String,
        #[serde(rename = "onFalse")]
        on_false: String,
        #[serde(rename = "onUnknown")]
        on_unknown: String,
    },
    Terminal {
        id: String,
        action: Action,
    },
}
impl Node {
    pub fn id(&self) -> &str {
        match self {
            Self::Decision { id, .. } | Self::Terminal { id, .. } => id,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Graph {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub revision: u64,
    pub root: String,
    pub nodes: Vec<Node>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Context {
    pub config_revision: u64,
    pub store_revision: u64,
    pub inventory_revision: u64,
    pub manual_revision: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub graph: Graph,
    pub catalog: Vec<FactDefinition>,
    pub snapshot: Snapshot,
    pub now_ms: u64,
    pub context: Context,
    pub manual_override: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateToken {
    pub graph_id: String,
    pub graph_revision: u64,
    pub snapshot_revision: u64,
    pub epoch: String,
    pub evaluated_at_ms: u64,
    pub context: Context,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Truth {
    True,
    False,
    Unknown,
}
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FactTrace {
    pub key: FactKey,
    pub predicate: Predicate,
    pub result: Truth,
    /// Index in Evaluation.observations; a large set is copied at most once.
    pub observation_index: Option<usize>,
    pub age_ms: Option<u64>,
    pub fresh: bool,
    pub unknown_reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Trace {
    Decision {
        node: String,
        result: Truth,
        next: String,
    },
    Fact {
        node: String,
        detail: FactTrace,
    },
    Terminal {
        node: String,
    },
    ManualOverride,
}
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Evaluation {
    pub schema_version: u32,
    pub token: StateToken,
    pub terminal: Option<String>,
    pub action: Action,
    pub observations: Vec<Fact>,
    pub trace: Vec<Trace>,
    pub trace_truncated: bool,
    pub audio_applied: bool,
}

/// Vocabulary only: this function does not install or simulate any observer.
/// Subjects are exact device/app/stream/peer/flag identities, never display-name matches.
pub fn standard_catalog() -> Vec<FactDefinition> {
    use Scope::{Entity, Global};
    use ValueType::{Bool, Number, Text, TextSet};
    let entries = [
        ("device.connected", TextSet, Global),
        ("device.default-input", Text, Global),
        ("device.default-output", Text, Global),
        ("device.selected", TextSet, Global),
        ("device.kind", Text, Entity),
        ("device.capabilities", TextSet, Entity),
        ("device.channels", Number, Entity),
        ("device.sample-rate-hz", Number, Entity),
        ("device.hardware-monitor-declared", Bool, Entity),
        ("application.running", TextSet, Global),
        ("application.foreground", Text, Global),
        ("application.fullscreen", Bool, Entity),
        ("application.playback", Bool, Entity),
        ("application.capture", Bool, Entity),
        ("application.call-active", Bool, Entity),
        ("stream.active", TextSet, Global),
        ("peer.connected", TextSet, Global),
        ("network.connected", Bool, Global),
        ("network.ssid", Text, Global),
        ("power.source", Text, Global),
        ("power.battery-percent", Number, Global),
        ("host.identity", Text, Global),
        ("civil.timezone", Text, Global),
        ("civil.date", Text, Global),
        ("civil.weekday", Number, Global),
        ("civil.minute-of-day", Number, Global),
        ("manual.flag", Bool, Entity),
        ("profile.current", Text, Global),
    ];
    entries
        .into_iter()
        .map(|(name, value_type, scope)| FactDefinition {
            name: name.into(),
            value_type,
            scope,
            max_age_ms: 5_000,
            evidence_policy: if name == "device.hardware-monitor-declared" {
                EvidencePolicy::UserDeclaration
            } else {
                EvidencePolicy::Any
            },
        })
        .collect()
}
