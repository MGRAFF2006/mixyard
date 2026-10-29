//! Content-only revision state shared by providers. No selectors are persisted.
use super::inventory::Result;
use crate::Inventory;
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) fn revision(inventory: &mut Inventory, directory: &Path) -> Result<()> {
    inventory.revision = 0;
    let bytes = serde_json::to_vec(inventory).map_err(|_| "inventory-serialization-failed")?;
    if bytes.len() > super::MAX_INVENTORY_FRAME - 256 {
        return Err("inventory-response-exceeds-1MiB-reduce-visible-graph");
    }
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let store = super::inventory_os::Store::open(directory)?;
    let previous = store.read()?;
    let (revision, state) = advance(previous.as_deref(), digest)?;
    if let Some(state) = state {
        store.write(&state)?;
    }
    inventory.revision = revision;
    Ok(())
}

fn advance(previous: Option<&[u8]>, digest: [u8; 32]) -> Result<(u64, Option<[u8; 80]>)> {
    let mut old = 0;
    if let Some(state) = previous {
        if state.len() != 80
            || &state[..8] != b"MXYINV01"
            || Sha256::digest(&state[..48])[..] != state[48..]
        {
            return Err("inventory-state-corrupt-do-not-reset");
        }
        old = u64::from_be_bytes(state[8..16].try_into().unwrap());
        if old == 0 {
            return Err("inventory-state-corrupt-do-not-reset");
        }
        if state[16..48] == digest {
            return Ok((old, None));
        }
    }
    let next = old.checked_add(1).ok_or("inventory-revision-exhausted")?;
    let mut state = [0; 80];
    state[..8].copy_from_slice(b"MXYINV01");
    state[8..16].copy_from_slice(&next.to_be_bytes());
    state[16..48].copy_from_slice(&digest);
    let checksum = Sha256::digest(&state[..48]);
    state[48..].copy_from_slice(&checksum);
    Ok((next, Some(state)))
}

pub(super) fn temporary_name() -> Result<String> {
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|_| "inventory-state-entropy-unavailable")?;
    Ok(format!(
        "inventory-revision-{:032x}.tmp",
        u128::from_be_bytes(random)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revisions_survive_restart_reversion_and_reject_damage_or_exhaustion() {
        let (first, a) = advance(None, [1; 32]).unwrap();
        let a = a.unwrap();
        assert_eq!(first, 1);
        assert_eq!(advance(Some(&a), [1; 32]).unwrap(), (1, None));
        let (next, b) = advance(Some(&a), [2; 32]).unwrap();
        assert_eq!(next, 2);
        assert_eq!(advance(Some(&b.unwrap()), [1; 32]).unwrap().0, 3);
        for i in [0, 8, 16, 48, 79] {
            let mut damaged = a;
            damaged[i] ^= 1;
            assert_eq!(
                advance(Some(&damaged), [1; 32]).unwrap_err(),
                "inventory-state-corrupt-do-not-reset"
            );
        }
        for counter in [0, u64::MAX] {
            let mut state = a;
            state[8..16].copy_from_slice(&counter.to_be_bytes());
            let checksum = Sha256::digest(&state[..48]);
            state[48..].copy_from_slice(&checksum);
            assert!(advance(Some(&state), [2; 32]).is_err());
        }
    }
}
