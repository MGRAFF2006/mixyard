//! Authenticated transports enter the same typed policy/store handlers as local HTTP.
//! This adapter owns the automation worker: do not run another owner on this store.
use super::{activation, api, assets, State, MAX_RESPONSE};
use mixyard::{profiles, runtime};
use std::{path::Path, sync::Arc};
use tokio::sync::Semaphore;

pub struct Canonical {
    state: State,
}

impl Canonical {
    pub fn new(
        store: &Path,
        assets: &Path,
        instance: String,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        runtime::endpoint(&instance)?;
        // Validate before starting the owner thread or touching automation state.
        let profile_path = store;
        let store = profiles::Store::new(profile_path)?;
        store.read_bounded(MAX_RESPONSE)?;
        let assets = assets::Assets::load(assets)?;
        let activation = activation::Service::start(profile_path, instance.clone());
        Ok(Self {
            state: State {
                authority: String::new(),
                token: String::new(),
                instance,
                store,
                assets,
                activation,
                operations: Arc::new(Semaphore::new(4)),
            },
        })
    }

    pub fn revision(&self) -> Result<u64, String> {
        self.state
            .store
            .read_bounded(MAX_RESPONSE)
            .map(|s| s.store_revision)
            .map_err(|e| e.to_string())
    }

    /// Call only after identity/session/role admission. No HTTP CSRF token is auth.
    pub fn dispatch(&self, path: &str, body: &[u8]) -> (u16, serde_json::Value) {
        let response = api::dispatch(path, body, &self.state);
        let status = response.status().as_u16();
        // Canonical replies are already fully serialized, bounded bodies; no I/O.
        let bytes = response
            .into_body()
            .into_inner()
            .expect("canonical JSON body");
        (
            status,
            serde_json::from_slice(&bytes).expect("canonical JSON reply"),
        )
    }
}
