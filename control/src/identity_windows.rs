//! Control secrets: private ACLs at creation and lifetime-pinned local ancestry.
//! Adapted from runtime/inventory_windows.rs; profile-store policy stays separate.
use super::{LIMIT, NAMES};
use crate::{fail, Failure, Result};
use std::{
    ffi::{c_void, OsStr},
    fs::File,
    io::{Read, Write},
    mem,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::{
        SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
        Threading::*,
    },
};

struct Handle(File);
impl Handle {
    fn raw(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}
fn unsafe_store(reason: &str) -> Failure {
    fail(503, "unsafe-identity-store", reason)
}
fn handle(raw: HANDLE) -> Result<Handle> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        Err(unsafe_store("windows-identity-windows-handle-unavailable"))
    } else {
        Ok(Handle(unsafe { File::from_raw_handle(raw) }))
    }
}
fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn sid() -> Result<String> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(unsafe_store("windows-identity-account-unavailable"));
        }
        let token = handle(token)?;
        let mut size = 0;
        GetTokenInformation(token.raw(), TokenUser, ptr::null_mut(), 0, &mut size);
        let mut buffer = vec![0usize; (size as usize).div_ceil(mem::size_of::<usize>())];
        if size == 0
            || GetTokenInformation(
                token.raw(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            ) == 0
        {
            return Err(unsafe_store("windows-identity-account-unavailable"));
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(unsafe_store("windows-identity-account-unavailable"));
        }
        let _allocation = Local(text.cast());
        let mut len = 0;
        while *text.add(len) != 0 {
            len += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(text, len))
            .map_err(|_| unsafe_store("windows-identity-account-unavailable"))
    }
}
struct Security {
    allocation: Local,
}
impl Security {
    fn new() -> Result<Self> {
        let text = wide(format!("O:{0}D:P(A;;FA;;;{0})", sid()?));
        let mut descriptor = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(unsafe_store("windows-identity-private-acl-unavailable"));
        }
        Ok(Self {
            allocation: Local(descriptor),
        })
    }
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.allocation.0,
            bInheritHandle: 0,
        }
    }
}
fn private_acl(file: HANDLE) -> Result<()> {
    unsafe {
        let mut owner = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        if GetSecurityInfo(
            file,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut acl,
            ptr::null_mut(),
            &mut descriptor,
        ) != ERROR_SUCCESS
        {
            return Err(unsafe_store("windows-identity-private-acl-unavailable"));
        }
        let _allocation = Local(descriptor);
        let expected = Security::new()?;
        let mut expected_owner = ptr::null_mut();
        let mut defaulted = 0;
        if GetSecurityDescriptorOwner(expected.allocation.0, &mut expected_owner, &mut defaulted)
            == 0
            || EqualSid(owner, expected_owner) == 0
        {
            return Err(unsafe_store("windows-identity-state-insecure-owner"));
        }
        let mut control = 0;
        let mut revision = 0;
        if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
            || control & SE_DACL_PROTECTED == 0
            || acl.is_null()
            || IsValidAcl(acl) == 0
            || (*acl).AceCount != 1
        {
            return Err(unsafe_store("windows-identity-state-insecure-acl"));
        }
        let mut raw = ptr::null_mut();
        if GetAce(acl, 0, &mut raw) == 0 {
            return Err(unsafe_store("windows-identity-state-insecure-acl"));
        }
        let ace = &*raw.cast::<ACCESS_ALLOWED_ACE>();
        if u32::from(ace.Header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || usize::from(ace.Header.AceSize) < mem::size_of::<ACCESS_ALLOWED_ACE>() + 4
            || ace.Header.AceFlags != 0
            || ace.Mask != FILE_ALL_ACCESS
            || EqualSid(
                (&ace.SidStart as *const u32).cast_mut().cast(),
                expected_owner,
            ) == 0
        {
            return Err(unsafe_store("windows-identity-state-insecure-acl"));
        }
    }
    Ok(())
}

// These principals already control the current account or Windows itself.
// TrustedInstaller's fixed service SID is documented in Microsoft's Windows
// file-permission reference; no account-name/domain lookup is needed here.
struct TrustedAncestorSids(Vec<Local>);
impl TrustedAncestorSids {
    fn new() -> Result<Self> {
        let mut sids = Vec::new();
        for text in [
            sid()?,
            "S-1-5-18".into(),     // LocalSystem
            "S-1-5-32-544".into(), // Builtin Administrators
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464".into(),
        ] {
            let mut raw = ptr::null_mut();
            if unsafe { ConvertStringSidToSidW(wide(text).as_ptr(), &mut raw) } == 0 {
                return Err(unsafe_store("windows-identity-ancestor-trust-unavailable"));
            }
            sids.push(Local(raw));
        }
        Ok(Self(sids))
    }
    fn contains(&self, sid: PSID) -> bool {
        unsafe { IsValidSid(sid) != 0 && self.0.iter().any(|s| EqualSid(sid, s.0) != 0) }
    }
}

fn ancestor_acl(file: HANDLE, trusted: &TrustedAncestorSids) -> Result<()> {
    unsafe {
        let mut owner = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        if GetSecurityInfo(
            file,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut acl,
            ptr::null_mut(),
            &mut descriptor,
        ) != ERROR_SUCCESS
        {
            return Err(unsafe_store("windows-identity-ancestor-acl-unavailable"));
        }
        let _allocation = Local(descriptor);
        if !trusted.contains(owner) {
            return Err(unsafe_store(
                "windows-identity-state-insecure-ancestor-owner",
            ));
        }
        if acl.is_null() || IsValidAcl(acl) == 0 {
            return Err(unsafe_store("windows-identity-state-insecure-ancestor-acl"));
        }
        let mapping = GENERIC_MAPPING {
            GenericRead: FILE_GENERIC_READ,
            GenericWrite: FILE_GENERIC_WRITE,
            GenericExecute: FILE_GENERIC_EXECUTE,
            GenericAll: FILE_ALL_ACCESS,
        };
        for index in 0..u32::from((*acl).AceCount) {
            let mut raw = ptr::null_mut();
            if GetAce(acl, index, &mut raw) == 0 {
                return Err(unsafe_store("windows-identity-state-insecure-ancestor-acl"));
            }
            let header = &*raw.cast::<ACE_HEADER>();
            // Inherit-only entries do not grant rights on this directory.
            // Every actual descendant on the selected path is checked itself.
            if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
                continue;
            }
            if u32::from(header.AceType) == ACCESS_DENIED_ACE_TYPE {
                continue;
            }
            if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
                || usize::from(header.AceSize) < mem::size_of::<ACCESS_ALLOWED_ACE>() + 4
            {
                return Err(unsafe_store(
                    "windows-identity-state-unsupported-ancestor-acl",
                ));
            }
            let ace = &*raw.cast::<ACCESS_ALLOWED_ACE>();
            let trustee = (&ace.SidStart as *const u32).cast_mut().cast();
            if IsValidSid(trustee) == 0
                || GetLengthSid(trustee) as usize + 8 > usize::from(header.AceSize)
            {
                return Err(unsafe_store("windows-identity-state-insecure-ancestor-acl"));
            }
            let mut mask = ace.Mask;
            MapGenericMask(&mut mask, &mapping);
            // Creating an unrelated child or reading a parent cannot replace
            // the protected leaf. Delete, delete-child and owner/ACL changes can.
            if mask & (DELETE | FILE_DELETE_CHILD | WRITE_DAC | WRITE_OWNER) != 0
                && !trusted.contains(trustee)
            {
                return Err(unsafe_store("windows-identity-state-insecure-ancestor-acl"));
            }
        }
    }
    Ok(())
}
fn information(file: HANDLE, directory: bool) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file, &mut info) } == 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        || (!directory && info.nNumberOfLinks != 1)
    {
        return Err(unsafe_store(
            "windows-identity-state-reparse-hardlink-or-invalid-type",
        ));
    }
    Ok(info)
}
fn open(
    path: &Path,
    access: u32,
    share: u32,
    disposition: u32,
    security: *const SECURITY_ATTRIBUTES,
) -> Result<Handle> {
    handle(unsafe {
        CreateFileW(
            wide(path).as_ptr(),
            access,
            share,
            security,
            disposition,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    })
}
// Retain handles without FILE_SHARE_DELETE for every ancestor. Reparse rejection
// alone is insufficient if a directory can be exchanged after validation.
// FILE_LIST_DIRECTORY is required: metadata-only handles are exempt from the
// sharing checks and therefore do not prevent rename (hosted kernel regression).
fn pin_ancestors(path: &Path) -> Result<Vec<Handle>> {
    let mut current = PathBuf::new();
    let mut pins = Vec::new();
    let trusted = TrustedAncestorSids::new()?;
    for part in path.components() {
        match part {
            Component::Prefix(prefix)
                if matches!(prefix.kind(), std::path::Prefix::Verbatim(_)) =>
            {
                current.push(part)
            }
            Component::RootDir | Component::Normal(_) => current.push(part),
            _ => {
                return Err(unsafe_store(
                    "windows-identity-state-requires-local-absolute-path",
                ))
            }
        }
        if !current.has_root() {
            continue;
        }
        let directory = open(
            &current,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            OPEN_EXISTING,
            ptr::null(),
        )?;
        information(directory.raw(), true)?;
        local_ntfs(directory.raw())?;
        ancestor_acl(directory.raw(), &trusted)?;
        pins.push(directory);
    }
    if pins.is_empty() {
        return Err(unsafe_store(
            "windows-identity-state-requires-local-absolute-path",
        ));
    }
    Ok(pins)
}

// Resolve the DOS drive once to a volume GUID. Later path operations never depend
// on a mutable drive-letter mapping. Only local fixed NTFS volumes are admitted.
fn resolved_path(path: &Path) -> Result<PathBuf> {
    let mut parts = path.components();
    if !path.is_absolute()
        || !matches!(parts.next(), Some(Component::Prefix(p)) if matches!(p.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)))
        || !matches!(parts.next(), Some(Component::RootDir))
    {
        return Err(unsafe_store("Local absolute drive path required"));
    }
    let mut root = PathBuf::new();
    for part in path.components().take(2) {
        root.push(part);
    }
    let root = open(
        &root,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        OPEN_EXISTING,
        ptr::null(),
    )?;
    information(root.raw(), true)?;
    local_ntfs(root.raw())?;
    let mut buffer = vec![0u16; 32768];
    let size = unsafe {
        GetFinalPathNameByHandleW(
            root.raw(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            VOLUME_NAME_GUID,
        )
    } as usize;
    if size == 0 || size >= buffer.len() {
        return Err(unsafe_store("Volume identity unavailable"));
    }
    let volume = std::ffi::OsString::from_wide(&buffer[..size]);
    if !volume.to_string_lossy().starts_with(r"\\?\Volume{")
        || unsafe { GetDriveTypeW(wide(&volume).as_ptr()) } != 3
    // DRIVE_FIXED
    {
        return Err(unsafe_store("Only local fixed NTFS volumes supported"));
    }
    let mut resolved = PathBuf::from(volume);
    for part in parts {
        let Component::Normal(name) = part else {
            return Err(unsafe_store("Unsafe path component"));
        };
        let text = name.to_string_lossy();
        let stem = text.split('.').next().unwrap_or("").to_ascii_uppercase();
        if text.ends_with(['.', ' '])
            || text.chars().any(|c| c < ' ' || ":*?\"<>|".contains(c))
            || ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
            || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4)
        {
            return Err(unsafe_store("Unsafe or ambiguous path component"));
        }
        resolved.push(name);
    }
    if resolved.parent().is_none() {
        return Err(unsafe_store("Dedicated identity directory required"));
    }
    Ok(resolved)
}
fn local_ntfs(file: HANDLE) -> Result<()> {
    let mut name = [0u16; 32];
    let mut flags = 0;
    if unsafe {
        GetVolumeInformationByHandleW(
            file,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut flags,
            name.as_mut_ptr(),
            name.len() as u32,
        )
    } == 0
        || !name.starts_with(&wide("NTFS"))
        || flags & FILE_PERSISTENT_ACLS == 0
    {
        return Err(unsafe_store("Local NTFS persistent ACL support required"));
    }
    Ok(())
}
fn delete(file: HANDLE) -> Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file,
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            mem::size_of_val(&info) as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
fn rename(file: HANDLE, target: &Path) -> Result<()> {
    let name = wide(target);
    let bytes = mem::offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2;
    // The API requires a variable-length structure with pointer alignment.
    let mut buffer = vec![0usize; bytes.div_ceil(mem::size_of::<usize>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = true;
        (*info).RootDirectory = ptr::null_mut();
        (*info).FileNameLength = ((name.len() - 1) * 2) as u32;
        ptr::copy_nonoverlapping(
            name.as_ptr(),
            ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            name.len(),
        );
        if SetFileInformationByHandle(file, FileRenameInfo, info.cast(), bytes as u32) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}

pub(super) struct Store {
    path: PathBuf,
    pins: Vec<Handle>,
    lock: Handle,
    retain_lock: bool,
}
impl Store {
    pub(super) fn open(path: &Path, initialize: bool) -> Result<Self> {
        let path = resolved_path(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| unsafe_store("Dedicated identity directory required"))?;
        // Require the immediate parent to exist; never recursively create secret
        // ancestry under an unchecked path. Pins survive until the store closes.
        let mut pins = pin_ancestors(parent)?;
        let security = Security::new()?;
        if initialize
            && unsafe { CreateDirectoryW(wide(&path).as_ptr(), &security.attributes()) } == 0
            && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let directory = open(
            &path,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            OPEN_EXISTING,
            ptr::null(),
        )?;
        information(directory.raw(), true)?;
        private_acl(directory.raw())?;
        pins.push(directory);
        check_entries(&path)?;
        let lock = private_create(&path.join("owner.lock"))
            .map_err(|_| fail(409, "identity-busy", "Owner lock exists or cannot be created; stop owner and inspect before manual recovery"))?;
        Ok(Self {
            path,
            pins,
            lock,
            retain_lock: false,
        })
    }
    pub(super) fn pending(&self) -> Result<bool> {
        Ok(read_file(&self.path.join("pending.json"))?.is_some())
    }
    pub(super) fn read(&self) -> Result<Option<Vec<u8>>> {
        let Some(mut file) = read_file(&self.path.join("state.json"))? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        (&mut file.0).take(LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > LIMIT {
            return Err(unsafe_store("Identity store exceeds 1 MiB"));
        }
        Ok(Some(bytes))
    }
    pub(super) fn check(&self) -> Result<()> {
        let trusted = TrustedAncestorSids::new()?;
        for directory in &self.pins {
            information(directory.raw(), true)?;
            ancestor_acl(directory.raw(), &trusted)?;
        }
        private_acl(self.pins.last().unwrap().raw())?;
        information(self.lock.raw(), false)?;
        private_acl(self.lock.raw())?;
        check_entries(&self.path)
    }
    pub(super) fn retain_lock(&mut self, retain: bool) {
        self.retain_lock = retain;
    }
    pub(super) fn write(&self, bytes: &[u8]) -> Result<()> {
        self.check()?;
        // Validate and pin any existing destination until the private pending
        // file is fully synced. Never overwrite a foreign, shared or linked file.
        let previous = read_file(&self.path.join("state.json"))?;
        let mut pending = private_create(&self.path.join("pending.json"))?;
        pending.0.write_all(bytes)?;
        pending.0.sync_all()?;
        self.check()?;
        information(pending.raw(), false)?;
        private_acl(pending.raw())?;
        drop(previous);
        // Rename the still-open source handle: no close/reopen or path-source
        // substitution window. All destination ancestors remain pinned.
        rename(pending.raw(), &self.path.join("state.json"))?;
        pending.0.sync_all()?;
        Ok(())
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        if !self.retain_lock {
            let _ = delete(self.lock.raw());
        }
    }
}
fn private_create(path: &Path) -> Result<Handle> {
    let security = Security::new()?;
    let file = open(
        path,
        GENERIC_READ | GENERIC_WRITE | DELETE | READ_CONTROL,
        FILE_SHARE_READ,
        CREATE_NEW,
        &security.attributes(),
    )?;
    information(file.raw(), false)?;
    private_acl(file.raw())?;
    Ok(file)
}
fn read_file(path: &Path) -> Result<Option<Handle>> {
    let raw = unsafe {
        CreateFileW(
            wide(path).as_ptr(),
            GENERIC_READ | READ_CONTROL,
            FILE_SHARE_READ,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE && unsafe { GetLastError() } == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    let file = handle(raw)?;
    information(file.raw(), false)?;
    private_acl(file.raw())?;
    Ok(Some(file))
}
fn check_entries(path: &Path) -> Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if !NAMES.iter().any(|name| entry.file_name() == *name) {
            return Err(unsafe_store("Identity directory contains foreign files"));
        }
        // Metadata-only access can inspect our exclusive owner/pending handles.
        let file = open(
            &entry.path(),
            FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            OPEN_EXISTING,
            ptr::null(),
        )?;
        information(file.raw(), false)?;
        private_acl(file.raw())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "identity_windows_tests.rs"]
mod tests;
