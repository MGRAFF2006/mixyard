//! Pinned Unix state directory and owned helper process group (Linux tests/macOS).
use super::inventory::Result;
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};

pub(super) struct Store {
    directory: File,
}
fn private(meta: &fs::Metadata, directory: bool) -> bool {
    meta.uid() == unsafe { libc::geteuid() }
        && meta.mode() & 0o777 == if directory { 0o700 } else { 0o600 }
        && if directory {
            meta.is_dir()
        } else {
            meta.is_file() && meta.nlink() == 1
        }
}
impl Store {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| "inventory-state-directory-unavailable")?;
        if !private(
            &directory
                .metadata()
                .map_err(|_| "inventory-state-directory-unavailable")?,
            true,
        ) {
            return Err("inventory-state-directory-insecure");
        }
        no_extra_acl(&directory)?;
        Ok(Self { directory })
    }
    fn open_at(&self, name: &str, flags: i32, mode: u32) -> io::Result<File> {
        let name = CString::new(name).map_err(|_| io::ErrorKind::InvalidInput)?;
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                mode as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    pub(super) fn read(&self) -> Result<Option<Vec<u8>>> {
        let mut file = match self.open_at("inventory-revision-v1", libc::O_RDONLY, 0) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("inventory-state-unavailable-do-not-reset"),
        };
        let meta = file.metadata().map_err(|_| "inventory-state-unavailable")?;
        if !private(&meta, false) || meta.len() != 80 {
            return Err("inventory-state-insecure-or-corrupt-do-not-reset");
        }
        no_extra_acl(&file)?;
        let mut state = vec![0; 80];
        file.read_exact(&mut state)
            .map_err(|_| "inventory-state-corrupt-do-not-reset")?;
        Ok(Some(state))
    }
    pub(super) fn write(&self, state: &[u8; 80]) -> Result<()> {
        let name = super::inventory_state::temporary_name()?;
        let temporary = CString::new(name.as_str()).unwrap();
        let target = c"inventory-revision-v1";
        let write = || -> io::Result<()> {
            let mut file =
                self.open_at(&name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)?;
            if !private(&file.metadata()?, false) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            no_extra_acl(&file).map_err(io::Error::other)?;
            file.write_all(state)?;
            file.sync_all()?;
            if unsafe {
                libc::renameat(
                    self.directory.as_raw_fd(),
                    temporary.as_ptr(),
                    self.directory.as_raw_fd(),
                    target.as_ptr(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            self.directory.sync_all()
        };
        if write().is_err() {
            unsafe {
                libc::unlinkat(self.directory.as_raw_fd(), temporary.as_ptr(), 0);
            }
            return Err("inventory-state-atomic-write-failed-do-not-reset");
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub(super) fn state_directory(instance: &str) -> Result<PathBuf> {
    // Resolve the account database, never a client-supplied path or HOME override.
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0u8; 65536];
    if unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    } != 0
        || result.is_null()
    {
        return Err("inventory-state-account-home-unavailable");
    }
    if unsafe { (*result).pw_dir.is_null() } {
        return Err("inventory-state-account-home-unavailable");
    }
    let home = unsafe { std::ffi::CStr::from_ptr((*result).pw_dir) };
    let mut path = PathBuf::from(std::ffi::OsStr::from_bytes(home.to_bytes()));
    if !path.is_absolute() {
        return Err("inventory-state-account-home-invalid");
    }
    let mut pins = trusted_ancestors(&path)?;
    let mut parent = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|_| "inventory-state-directory-unavailable")?;
    for (part, require_private) in [
        ("Library", false),
        ("Application Support", false),
        ("MixyardInventory", true),
        (instance, true),
    ] {
        let name = CString::new(part).map_err(|_| "inventory-state-directory-invalid")?;
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0
            && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists
        {
            return Err("inventory-state-directory-unavailable");
        }
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err("inventory-state-directory-unavailable");
        }
        let next = unsafe { File::from_raw_fd(fd) };
        let meta = next
            .metadata()
            .map_err(|_| "inventory-state-directory-unavailable")?;
        if meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o022 != 0
            || (require_private && !private(&meta, true))
        {
            return Err("inventory-state-directory-insecure");
        }
        no_extra_acl(&next)?;
        pins.push(parent);
        parent = next;
        path.push(part);
    }
    Ok(path)
}

#[cfg(any(target_os = "macos", test))]
fn trusted_ancestors(path: &Path) -> Result<Vec<File>> {
    let mut pins = Vec::new();
    for ancestor in path.ancestors() {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(ancestor)
            .map_err(|_| "native-inventory-install-directory-untrusted")?;
        let meta = file
            .metadata()
            .map_err(|_| "native-inventory-install-directory-untrusted")?;
        if !meta.is_dir()
            || (meta.uid() != 0 && meta.uid() != unsafe { libc::geteuid() })
            || meta.mode() & 0o022 != 0
        {
            return Err("native-inventory-install-directory-untrusted");
        }
        no_extra_acl(&file)?;
        pins.push(file);
    }
    Ok(pins)
}

#[cfg(target_os = "macos")]
use std::{os::unix::ffi::OsStrExt, path::PathBuf};
#[cfg(any(target_os = "macos", test))]
#[path = "inventory_process_unix.rs"]
mod process;
#[cfg(any(target_os = "macos", test))]
pub(super) use process::Process;

#[cfg(target_os = "macos")]
fn no_extra_acl(file: &File) -> Result<()> {
    // Darwin mode bits do not describe additional ACL grants. Deny entries are
    // harmless; reject every extended allow ACE rather than guessing its scope.
    use std::ffi::c_void;
    unsafe extern "C" {
        fn acl_get_fd(fd: libc::c_int) -> *mut c_void;
        fn acl_free(acl: *mut c_void) -> libc::c_int;
        fn acl_valid(acl: *mut c_void) -> libc::c_int;
        fn acl_get_entry(
            acl: *mut c_void,
            entry_id: libc::c_int,
            entry: *mut *mut c_void,
        ) -> libc::c_int;
        fn acl_get_tag_type(entry: *mut c_void, tag: *mut libc::c_int) -> libc::c_int;
    }
    let acl = unsafe { acl_get_fd(file.as_raw_fd()) };
    if acl.is_null() {
        // acl_get_fd uses fstatx_np on this pinned descriptor; ENOENT denotes
        // absent FILESEC_ACL, not a pathname race (Apple Libc acl_file.c).
        return if io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            Ok(())
        } else {
            Err("inventory-unix-acl-unavailable")
        };
    }
    let check = || -> Result<()> {
        if unsafe { acl_valid(acl) } != 0 {
            return Err("inventory-unix-acl-invalid");
        }
        let mut entry = std::ptr::null_mut();
        let mut entry_id = 0; // ACL_FIRST_ENTRY
        loop {
            if unsafe { acl_get_entry(acl, entry_id, &mut entry) } != 0 {
                return if io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL) {
                    Ok(())
                } else {
                    Err("inventory-unix-acl-unavailable")
                };
            }
            let mut tag = 0;
            if unsafe { acl_get_tag_type(entry, &mut tag) } != 0 || tag != 2 {
                return Err("inventory-unix-extra-acl-grants");
            }
            entry_id = -1; // ACL_NEXT_ENTRY
        }
    };
    let result = check();
    unsafe {
        acl_free(acl);
    }
    result
}
#[cfg(not(target_os = "macos"))]
fn no_extra_acl(_file: &File) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
    #[test]
    fn pinned_state_rejects_permissions_links_damage_and_preserves_previous_bytes() {
        let directory =
            std::env::temp_dir().join(format!("mixyard-unix-state-{}", std::process::id()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let store = Store::open(&directory).unwrap();
        let initial = [1; 80];
        store.write(&initial).unwrap();
        assert_eq!(store.read().unwrap().unwrap(), initial);
        let state = directory.join("inventory-revision-v1");
        fs::set_permissions(&state, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.read().is_err());
        fs::set_permissions(&state, fs::Permissions::from_mode(0o600)).unwrap();
        let saved = directory.join("saved");
        fs::hard_link(&state, &saved).unwrap();
        assert!(store.read().is_err());
        fs::remove_file(&saved).unwrap();
        fs::rename(&state, &saved).unwrap();
        symlink(&saved, &state).unwrap();
        assert!(store.read().is_err());
        assert_eq!(fs::read(&saved).unwrap(), initial);
        fs::remove_file(&state).unwrap();
        fs::rename(&saved, &state).unwrap();
        fs::write(&state, b"truncated").unwrap();
        assert!(store.read().is_err());
        // The descriptor stays with the original directory if its pathname moves.
        let moved = directory.with_extension("moved");
        fs::rename(&directory, &moved).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        store.write(&initial).unwrap();
        assert!(!directory.join("inventory-revision-v1").exists());
        assert_eq!(
            fs::read(moved.join("inventory-revision-v1")).unwrap(),
            initial
        );
        drop(store);
        fs::remove_dir_all(directory).unwrap();
        fs::remove_dir_all(moved).unwrap();
    }
}
