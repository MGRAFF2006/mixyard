//! Hosted kernel/ACL regressions. Every ACL mutation targets an owned fixture.
use super::*;
use crate::{IdentityStore, Permission};
use std::{fs, process::Command};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mixyard-private-{}",
            crate::identity::random().unwrap()
        ));
        let security = Security::new().unwrap();
        assert_ne!(
            unsafe { CreateDirectoryW(wide(&path).as_ptr(), &security.attributes()) },
            0
        );
        Self(path)
    }
    fn identity(&self) -> PathBuf {
        self.0.join("identity")
    }
    fn init(&self) -> IdentityStore {
        IdentityStore::open(&self.identity(), Some("127.0.0.1".parse().unwrap())).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn acl(path: &Path, sddl: &str, owner: bool) {
    unsafe {
        let mut descriptor = ptr::null_mut();
        assert_ne!(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(sddl).as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut()
            ),
            0
        );
        let _allocation = Local(descriptor);
        let mut dacl = ptr::null_mut();
        let mut present = 0;
        let mut defaulted = 0;
        assert_ne!(
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted),
            0
        );
        let mut sid = ptr::null_mut();
        let mut flags = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
        if owner {
            assert_ne!(
                GetSecurityDescriptorOwner(descriptor, &mut sid, &mut defaulted),
                0
            );
            flags |= OWNER_SECURITY_INFORMATION;
        }
        assert_eq!(
            SetNamedSecurityInfoW(
                wide(path).as_ptr(),
                SE_FILE_OBJECT,
                flags,
                sid,
                ptr::null_mut(),
                dacl,
                ptr::null()
            ),
            ERROR_SUCCESS
        );
    }
}
fn private(path: &Path, directory: bool) {
    let file = open(
        path,
        READ_CONTROL | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        OPEN_EXISTING,
        ptr::null(),
    )
    .unwrap();
    information(file.raw(), directory).unwrap();
    private_acl(file.raw()).unwrap();
}
fn public_acl(path: &Path) {
    acl(
        path,
        &format!("D:P(A;;FA;;;{})(A;;FA;;;WD)", sid().unwrap()),
        false,
    );
}
fn restore(path: &Path) {
    let sid = sid().unwrap();
    acl(path, &format!("O:{sid}D:P(A;;FA;;;{sid})"), true);
}

#[test]
fn creation_reopen_pair_rotate_revoke_and_persist_stay_private() {
    let fixture = Fixture::new();
    let mut identity = fixture.init();
    let id = identity.device_id().to_owned();
    let fingerprint = identity.trust().certificate_sha256;
    private(&fixture.identity(), true);
    for name in ["state.json", "owner.lock"] {
        private(&fixture.identity().join(name), false);
    }
    assert!(IdentityStore::open(&fixture.identity(), None).is_err());
    let invite = identity.invite(vec![Permission::Settings], 300).unwrap();
    let paired = identity
        .pair(
            invite["invitation"].as_str().unwrap(),
            "Windows ACL fixture".into(),
        )
        .unwrap();
    let first = paired["credential"].as_str().unwrap();
    let rotated = identity.rotate(first).unwrap();
    let next = rotated["credential"].as_str().unwrap().to_owned();
    assert!(identity.authenticate(first).is_err());
    assert!(identity.authenticate(&next).is_ok());
    private(&fixture.identity().join("state.json"), false);
    assert!(!fixture.identity().join("pending.json").exists());
    drop(identity);
    assert!(!fixture.identity().join("owner.lock").exists());
    let mut reopened = IdentityStore::open(&fixture.identity(), None).unwrap();
    assert_eq!(reopened.device_id(), id);
    assert_eq!(reopened.trust().certificate_sha256, fingerprint);
    assert!(reopened.audio_identity_material(&id).is_ok());
    reopened
        .revoke(paired["credentialId"].as_str().unwrap())
        .unwrap();
    drop(reopened);
    let reopened = IdentityStore::open(&fixture.identity(), None).unwrap();
    assert!(reopened.authenticate(&next).is_err());
    private(&fixture.identity().join("state.json"), false);
}

#[test]
fn pending_and_lock_are_private_before_any_secret_write() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.identity(), true).unwrap();
    let pending = private_create(&store.path.join("pending.json")).unwrap();
    private(&fixture.identity(), true);
    private(&fixture.identity().join("owner.lock"), false);
    private(&fixture.identity().join("pending.json"), false);
    assert_eq!(pending.0.metadata().unwrap().len(), 0);
    delete(pending.raw()).unwrap();
}

#[test]
fn unsafe_parent_and_directory_rejected_before_identity_is_written() {
    let fixture = Fixture::new();
    public_acl(&fixture.0);
    assert!(IdentityStore::open(&fixture.identity(), Some("127.0.0.1".parse().unwrap())).is_err());
    assert!(!fixture.identity().exists());
    restore(&fixture.0);
    fs::create_dir(fixture.identity()).unwrap();
    public_acl(&fixture.identity());
    assert!(IdentityStore::open(&fixture.identity(), Some("127.0.0.1".parse().unwrap())).is_err());
    assert!(fs::read_dir(fixture.identity()).unwrap().next().is_none());
}

#[test]
fn shared_state_is_rejected_without_mutation_on_reopen_and_persist() {
    let fixture = Fixture::new();
    let mut identity = fixture.init();
    let state = fixture.identity().join("state.json");
    let before = fs::read(&state).unwrap();
    public_acl(&state);
    assert!(identity.invite(vec![Permission::View], 300).is_err());
    assert_eq!(fs::read(&state).unwrap(), before);
    assert!(!fixture.identity().join("pending.json").exists());
    drop(identity);
    assert!(IdentityStore::open(&fixture.identity(), None).is_err());
    assert_eq!(fs::read(&state).unwrap(), before);
    restore(&state);
    let _reopened = IdentityStore::open(&fixture.identity(), None).unwrap();
}

#[test]
fn ancestor_acl_mutation_is_rejected_before_any_pending_secret_write() {
    let fixture = Fixture::new();
    let mut identity = fixture.init();
    let before = fs::read(fixture.identity().join("state.json")).unwrap();
    public_acl(&fixture.0);
    assert!(identity.invite(vec![Permission::View], 300).is_err());
    assert!(!fixture.identity().join("pending.json").exists());
    assert_eq!(
        fs::read(fixture.identity().join("state.json")).unwrap(),
        before
    );
    restore(&fixture.0);
    identity.invite(vec![Permission::View], 300).unwrap();
}

#[test]
fn different_owner_or_unprotected_dacl_is_not_private() {
    let fixture = Fixture::new();
    let identity = fixture.init();
    drop(identity);
    let state = fixture.identity().join("state.json");
    let before = fs::read(&state).unwrap();
    let sid = sid().unwrap();
    // Hosted runner is an administrator; only this fixture changes ownership.
    acl(&state, &format!("O:BAD:P(A;;FA;;;{sid})"), true);
    assert!(IdentityStore::open(&fixture.identity(), None).is_err());
    restore(&state);
    let expected = Security::new().unwrap();
    unsafe {
        let mut dacl = ptr::null_mut();
        let mut present = 0;
        let mut defaulted = 0;
        assert_ne!(
            GetSecurityDescriptorDacl(
                expected.allocation.0,
                &mut present,
                &mut dacl,
                &mut defaulted
            ),
            0
        );
        assert_eq!(
            SetNamedSecurityInfoW(
                wide(&state).as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null()
            ),
            ERROR_SUCCESS
        );
    }
    assert!(IdentityStore::open(&fixture.identity(), None).is_err());
    assert_eq!(fs::read(&state).unwrap(), before);
    restore(&state);
}

#[test]
fn hardlink_and_replaced_foreign_state_are_rejected() {
    let fixture = Fixture::new();
    let mut identity = fixture.init();
    let state = fixture.identity().join("state.json");
    let alias = fixture.0.join("alias");
    fs::hard_link(&state, &alias).unwrap();
    let before = fs::read(&alias).unwrap();
    assert!(identity.invite(vec![Permission::View], 300).is_err());
    assert_eq!(fs::read(&state).unwrap(), before);
    assert!(!fixture.identity().join("pending.json").exists());
    fs::remove_file(&alias).unwrap();
    fs::rename(&state, &alias).unwrap();
    fs::write(&state, b"foreign-state-do-not-touch").unwrap();
    public_acl(&state);
    assert!(identity.invite(vec![Permission::View], 300).is_err());
    assert_eq!(fs::read(&state).unwrap(), b"foreign-state-do-not-touch");
    assert!(!fixture.identity().join("pending.json").exists());
}

#[test]
fn held_ancestor_and_lock_handles_block_replacement_and_cleanup_is_by_handle() {
    let fixture = Fixture::new();
    let mut identity = fixture.init();
    assert!(fs::rename(&fixture.0, fixture.0.with_extension("swapped")).is_err());
    assert!(fs::rename(fixture.identity(), fixture.0.join("swapped")).is_err());
    assert!(fs::rename(
        fixture.identity().join("owner.lock"),
        fixture.0.join("swapped-lock")
    )
    .is_err());
    assert!(fs::remove_file(fixture.identity().join("owner.lock")).is_err());
    identity.invite(vec![Permission::View], 300).unwrap();
    drop(identity);
    assert!(!fixture.identity().join("owner.lock").exists());
}

#[test]
fn junction_ancestor_and_leaf_never_receive_identity_material() {
    let fixture = Fixture::new();
    let target = fixture.0.join("target");
    fs::create_dir(&target).unwrap();
    let link = fixture.0.join("junction");
    let status = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "Hosted Windows must support owned junction fixtures"
    );
    assert!(
        IdentityStore::open(&link.join("identity"), Some("127.0.0.1".parse().unwrap())).is_err()
    );
    assert!(IdentityStore::open(&link, Some("127.0.0.1".parse().unwrap())).is_err());
    assert!(fs::read_dir(&target).unwrap().next().is_none());
    fs::remove_dir(&link).unwrap();
}

#[test]
fn held_write_handle_prevents_source_substitution_and_failed_commit_keeps_barrier() {
    let fixture = Fixture::new();
    let mut identity = fixture.init();
    let pending_path = fixture.identity().join("pending.json");
    let pending = private_create(&pending_path).unwrap();
    assert!(fs::rename(&pending_path, fixture.0.join("substitute")).is_err());
    assert!(fs::remove_file(&pending_path).is_err());
    // Admission succeeds but CREATE_NEW fails: this is a poisoned write, not a
    // successful invitation. The old state and recovery sentinels survive.
    let before = fs::read(fixture.identity().join("state.json")).unwrap();
    assert!(identity.invite(vec![Permission::View], 300).is_err());
    assert!(identity
        .audio_identity_material(identity.device_id())
        .is_err());
    assert_eq!(
        fs::read(fixture.identity().join("state.json")).unwrap(),
        before
    );
    drop(pending);
    drop(identity);
    private(&fixture.identity().join("owner.lock"), false);
    private(&pending_path, false);
    assert!(IdentityStore::open(&fixture.identity(), None).is_err());
}

#[test]
fn unsupported_paths_are_rejected_without_creating_state() {
    for path in [
        r"\\server\share\identity",
        r"\\.\C:\identity",
        r"C:\identity:stream",
        r"C:\identity.",
        r"C:\NUL",
        r"C:\missing\..\identity",
    ] {
        assert!(
            IdentityStore::open(Path::new(path), Some("127.0.0.1".parse().unwrap())).is_err(),
            "{path}"
        );
    }
}

#[test]
fn reparse_mutation_rights_without_delete_rights_fail_closed() {
    let fixture = Fixture::new();
    for rights in [FILE_WRITE_DATA, FILE_WRITE_ATTRIBUTES, FILE_WRITE_EA] {
        acl(
            &fixture.0,
            &format!("D:P(A;;FA;;;{})(A;;0x{rights:08x};;;WD)", sid().unwrap()),
            false,
        );
        assert!(
            IdentityStore::open(&fixture.identity(), Some("127.0.0.1".parse().unwrap())).is_err()
        );
        assert!(!fixture.identity().exists());
        restore(&fixture.0);
    }
}
