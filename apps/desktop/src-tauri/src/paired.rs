use mixyard_desktop_control::{Invitation, OpenReply, Paired, PlatformVault, Reply, Target};
use std::sync::Arc;
use tauri::State;
type Client = Arc<Paired<PlatformVault>>;

async fn worker<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|_| {
        "Native paired worker failed. Refresh and review before another write.".to_string()
    })?
}
#[tauri::command]
pub async fn paired_list(client: State<'_, Client>) -> Result<Vec<Target>, String> {
    let client = client.inner().clone();
    worker(move || client.list()).await
}
#[tauri::command]
pub async fn paired_pair(
    endpoint: String,
    display_name: String,
    invitation: String,
    client: State<'_, Client>,
) -> Result<Target, String> {
    if invitation.len() > 8192 {
        return Err("Invitation exceeds 8 KiB".into());
    }
    let invitation: Invitation =
        serde_json::from_str(&invitation).map_err(|_| "Invalid out-of-band invitation JSON")?;
    let client = client.inner().clone();
    worker(move || client.pair(endpoint, display_name, invitation)).await
}
#[tauri::command]
pub async fn paired_open(
    target_id: String,
    credential_id: String,
    client: State<'_, Client>,
) -> Result<OpenReply, String> {
    let client = client.inner().clone();
    worker(move || client.open(&target_id, &credential_id)).await
}
#[tauri::command]
pub async fn paired_request(
    target_id: String,
    generation: u64,
    raw: String,
    client: State<'_, Client>,
) -> Result<Reply, String> {
    if raw.len() > 1024 * 1024 {
        return Err("Request exceeds 1 MiB".into());
    }
    let client = client.inner().clone();
    worker(move || client.request(&target_id, generation, &raw)).await
}
#[tauri::command]
pub fn paired_disconnect(
    target_id: String,
    expected_generation: Option<u64>,
    client: State<'_, Client>,
) -> Result<(), String> {
    client.disconnect(&target_id, expected_generation)
}
#[tauri::command]
pub async fn paired_forget(target_id: String, client: State<'_, Client>) -> Result<(), String> {
    let client = client.inner().clone();
    worker(move || client.forget(&target_id)).await
}
#[tauri::command]
pub async fn paired_revoke(
    target_id: String,
    credential_id: String,
    client: State<'_, Client>,
) -> Result<(), String> {
    let client = client.inner().clone();
    worker(move || {
        let session = client.open(&target_id, &credential_id)?;
        if session.status != 200 {
            return Err(format!(
                "Cannot refresh controller before revocation (HTTP {}).",
                session.status
            ));
        }
        let reply = client.revoke_self(&target_id, session.generation)?;
        if reply.status != 200 {
            return Err(format!(
                "Revocation rejected (HTTP {}); reconcile on the owning service.",
                reply.status
            ));
        }
        Ok(())
    })
    .await
}
