use super::{error, json, Reply, State, MAX_RESPONSE};
use hyper::StatusCode;
use mixyard::{diagnostics, profiles, runtime::ReadOnlyClient, Config, Inventory};
use serde::{de::DeserializeOwned, Deserialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigBody {
    config: Config,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectBody {
    config: Config,
    inventory: Inventory,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EditBody {
    expected_revision: u64,
    edit: Edit,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Edit {
    Draft {
        config: Config,
    },
    Save {
        id: Option<String>,
        name: String,
        config: Config,
    },
    Rename {
        id: String,
        name: String,
    },
    Load {
        id: String,
    },
    Undo {},
}
impl From<Edit> for profiles::Edit {
    fn from(edit: Edit) -> Self {
        match edit {
            Edit::Draft { config } => Self::Draft(config),
            Edit::Save { id, name, config } => Self::Save { id, name, config },
            Edit::Rename { id, name } => Self::Rename { id, name },
            Edit::Load { id } => Self::Load { id },
            Edit::Undo {} => Self::Undo,
        }
    }
}
fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Reply> {
    serde_json::from_slice(bytes)
        .map_err(|e| error(StatusCode::BAD_REQUEST, "invalid-json", &e.to_string()))
}
fn profile_error(e: profiles::Error) -> Reply {
    let status = match e.code {
        "store-conflict" | "store-busy" | "recovery-required" | "commit-uncertain" => {
            StatusCode::CONFLICT
        }
        "profile-not-found" => StatusCode::NOT_FOUND,
        "invalid-policy" | "invalid-name" | "no-undo" | "revision-exhausted" => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, e.code, &e.message)
}
pub(super) fn dispatch(path: &str, bytes: &[u8], state: &State) -> Reply {
    // Keep typed JSON extraction separate from the transport's protection checks.
    let result = || -> Result<Reply, Reply> {
        Ok(match path {
            "/api/status" => {
                let _: Empty = parse(bytes)?;
                match ReadOnlyClient::connect(&state.instance) {
                    Ok(client) => json(
                        &serde_json::json!({"connected":true,"snapshot":client.initial_snapshot(),"error":null}),
                    ),
                    Err(e) => json(
                        &serde_json::json!({"connected":false,"snapshot":null,"error":e.to_string()}),
                    ),
                }
            }
            "/api/validate" => {
                let body: ConfigBody = parse(bytes)?;
                body.config
                    .validate()
                    .map_err(|e| error(StatusCode::UNPROCESSABLE_ENTITY, "invalid-policy", &e))?;
                json(&serde_json::json!({"valid":true}))
            }
            "/api/inspect" => {
                let body: InspectBody = parse(bytes)?;
                json(&diagnostics::inspect(&body.config, &body.inventory))
            }
            "/api/profiles/read" => {
                let _: Empty = parse(bytes)?;
                json(
                    &state
                        .store
                        .read_bounded(MAX_RESPONSE)
                        .map_err(profile_error)?,
                )
            }
            "/api/profiles/edit" => {
                let body: EditBody = parse(bytes)?;
                // Refuse a pre-existing oversized store before the core's atomic
                // edit. Trusted local writers must still respect the store contract.
                state
                    .store
                    .read_bounded(MAX_RESPONSE)
                    .map_err(profile_error)?;
                json(
                    &state
                        .store
                        .edit(body.expected_revision, body.edit.into())
                        .map_err(profile_error)?,
                )
            }
            _ => error(StatusCode::NOT_FOUND, "not-found", "Unknown API endpoint"),
        })
    };
    result().unwrap_or_else(|response| response)
}
