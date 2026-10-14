//! One worker, one subscription, no command queue or runtime lifecycle ownership.
use mixyard::runtime::{ReadOnlyClient, RuntimeEvent};
use serde::Serialize;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub phase: &'static str,
    pub instance: Option<String>,
    pub event: Option<RuntimeEvent>,
    pub error: Option<String>,
}
impl ConnectionStatus {
    fn disconnected() -> Self {
        Self {
            phase: "disconnected",
            instance: None,
            event: None,
            error: None,
        }
    }
}
struct Inner {
    generation: u64,
    desired: Option<String>,
    status: ConnectionStatus,
}
pub struct Connection(Arc<(Mutex<Inner>, Condvar)>);
impl Connection {
    pub fn new() -> std::io::Result<Self> {
        let shared = Arc::new((
            Mutex::new(Inner {
                generation: 0,
                desired: None,
                status: ConnectionStatus::disconnected(),
            }),
            Condvar::new(),
        ));
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("runtime-readonly".into())
            .spawn(move || {
                let (lock, wake) = &*worker;
                loop {
                    let (generation, instance) = {
                        let mut inner = lock.lock().expect("connection lock");
                        while inner.desired.is_none() {
                            inner = wake.wait(inner).expect("connection lock");
                        }
                        (inner.generation, inner.desired.clone().unwrap())
                    };
                    // All transport I/O, including endpoint owner checks, stays off the UI thread.
                    let result = ReadOnlyClient::connect(&instance);
                    let mut inner = lock.lock().expect("connection lock");
                    if inner.generation != generation {
                        continue;
                    }
                    match result {
                        Err(error) => {
                            inner.desired = None;
                            inner.status.phase = "disconnected";
                            inner.status.error = Some(error.to_string());
                            inner.status.event = None;
                        }
                        Ok(mut client) => {
                            inner.status.phase = "connected";
                            inner.status.event =
                                Some(RuntimeEvent::Snapshot(client.initial_snapshot().clone()));
                            drop(inner);
                            loop {
                                let result = client.poll();
                                let mut inner = lock.lock().expect("connection lock");
                                if inner.generation != generation {
                                    break;
                                }
                                match result {
                                    Ok(Some(event)) => {
                                        inner.status.event = Some(event);
                                        inner.status.phase = "stopped";
                                        inner.desired = None;
                                        break;
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        inner.desired = None;
                                        inner.status.phase = "disconnected";
                                        inner.status.event = None;
                                        inner.status.error = Some(error.to_string());
                                        break;
                                    }
                                }
                                let _ = wake
                                    .wait_timeout(inner, Duration::from_millis(25))
                                    .expect("connection lock");
                            }
                        }
                    }
                }
            })?;
        Ok(Self(shared))
    }
    pub fn status(&self) -> ConnectionStatus {
        self.0 .0.lock().expect("connection lock").status.clone()
    }
    pub fn connect(&self, instance: String) -> Result<ConnectionStatus, String> {
        if instance.is_empty()
            || instance.len() > 32
            || !instance
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("Use 1–32 ASCII letters, digits or hyphens for the local instance.".into());
        }
        let mut inner = self.0 .0.lock().expect("connection lock");
        if inner.desired.is_some() {
            return Err("Disconnect the current connection before connecting again.".into());
        }
        inner.generation += 1;
        inner.desired = Some(instance.clone());
        inner.status = ConnectionStatus {
            phase: "connecting",
            instance: Some(instance),
            event: None,
            error: None,
        };
        self.0 .1.notify_one();
        Ok(inner.status.clone())
    }
    pub fn disconnect(&self) -> ConnectionStatus {
        let mut inner = self.0 .0.lock().expect("connection lock");
        inner.generation += 1;
        inner.desired = None;
        inner.status = ConnectionStatus::disconnected();
        self.0 .1.notify_one();
        inner.status.clone()
    }
}
