//! Protected read-only graph validation, simulation and server-owned observation.
//! Profile writes/automatic activation deliberately have a separate ownership path.
use super::{error, json, Reply, State};
use hyper::StatusCode;
use mixyard::automation as core;
use serde::Deserialize;

fn rejected(e: core::Error) -> Reply {
    let status = if e.code == "invalid-json" {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    error(status, e.code, &e.message)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Validation {
    graph: core::Graph,
    catalog: Vec<core::FactDefinition>,
}

pub(super) fn validate(bytes: &[u8]) -> Reply {
    let body: Validation = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(e) => return error(StatusCode::BAD_REQUEST, "invalid-json", &e.to_string()),
    };
    // Static validation needs no observer or profile. No fact is invented as
    // known, and no path is executed just to import an editor configuration.
    let request = core::Request {
        graph: body.graph,
        catalog: body.catalog,
        snapshot: core::Snapshot {
            schema_version: 1,
            revision: 0,
            epoch: "validation".into(),
            facts: vec![],
        },
        now_ms: 0,
        context: core::Context {
            config_revision: 0,
            store_revision: 0,
            inventory_revision: 0,
            manual_revision: 0,
        },
        manual_override: false,
    };
    match core::validate(&request) {
        Ok(()) => json(&serde_json::json!({"valid":true})),
        Err(e) => rejected(e),
    }
}

pub(super) fn evaluate(bytes: &[u8]) -> Reply {
    match core::parse_request(bytes).and_then(|request| core::evaluate(&request)) {
        // This result always describes provided values at the provided time.
        // A client's "platform-observed" claim does not become live authority.
        Ok(evaluation) => json(&serde_json::json!({"mode":"simulation","evaluation":evaluation})),
        Err(e) => rejected(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

pub(super) fn context(bytes: &[u8], state: &State) -> Reply {
    if let Err(e) = serde_json::from_slice::<Empty>(bytes) {
        return error(StatusCode::BAD_REQUEST, "invalid-json", &e.to_string());
    }
    state.activation.context()
}
