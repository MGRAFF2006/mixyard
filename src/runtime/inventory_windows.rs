//! Current-SID-only revision files and suspended, job-owned native helper.
use super::inventory::Result;
use std::{
    ffi::{c_void, OsStr},
    fs::File,
    io::{Read, Write},
    mem,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::FromRawHandle,
    },
    path::{Component, Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::{
        Com::CoTaskMemFree, JobObjects::*, Pipes::*, SystemServices::ACCESS_ALLOWED_ACE_TYPE,
        Threading::*,
    },
    UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn handle(raw: HANDLE) -> Result<Handle> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        Err("inventory-windows-handle-unavailable")
    } else {
        Ok(Handle(raw))
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
            return Err("inventory-account-unavailable");
        }
        let token = handle(token)?;
        let mut size = 0;
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut size);
        let mut buffer = vec![0usize; (size as usize).div_ceil(mem::size_of::<usize>())];
        if size == 0
            || GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            ) == 0
        {
            return Err("inventory-account-unavailable");
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err("inventory-account-unavailable");
        }
        let _allocation = Local(text.cast());
        let mut len = 0;
        while *text.add(len) != 0 {
            len += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(text, len))
            .map_err(|_| "inventory-account-unavailable")
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
            return Err("inventory-private-acl-unavailable");
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
            return Err("inventory-private-acl-unavailable");
        }
        let _allocation = Local(descriptor);
        let expected = Security::new()?;
        let mut expected_owner = ptr::null_mut();
        let mut defaulted = 0;
        if GetSecurityDescriptorOwner(expected.allocation.0, &mut expected_owner, &mut defaulted)
            == 0
            || EqualSid(owner, expected_owner) == 0
        {
            return Err("inventory-state-insecure-owner");
        }
        let mut control = 0;
        let mut revision = 0;
        if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
            || control & SE_DACL_PROTECTED == 0
            || acl.is_null()
            || (*acl).AceCount != 1
        {
            return Err("inventory-state-insecure-acl");
        }
        let mut raw = ptr::null_mut();
        if GetAce(acl, 0, &mut raw) == 0 {
            return Err("inventory-state-insecure-acl");
        }
        let ace = &*raw.cast::<ACCESS_ALLOWED_ACE>();
        if u32::from(ace.Header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || ace.Header.AceFlags != 0
            || ace.Mask != FILE_ALL_ACCESS
            || EqualSid(
                (&ace.SidStart as *const u32).cast_mut().cast(),
                expected_owner,
            ) == 0
        {
            return Err("inventory-state-insecure-acl");
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
        return Err("inventory-state-reparse-hardlink-or-invalid-type");
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
    for part in path.components() {
        match part {
            Component::Prefix(prefix)
                if matches!(
                    prefix.kind(),
                    std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                ) =>
            {
                current.push(part)
            }
            Component::RootDir | Component::Normal(_) => current.push(part),
            _ => return Err("inventory-state-requires-local-absolute-path"),
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
        information(directory.0, true)?;
        pins.push(directory);
    }
    if pins.is_empty() {
        return Err("inventory-state-requires-local-absolute-path");
    }
    Ok(pins)
}

pub(super) fn state_directory(instance: &str) -> Result<PathBuf> {
    let mut raw = ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, ptr::null_mut(), &mut raw) } < 0 {
        return Err("inventory-state-local-app-data-unavailable");
    }
    let mut len = 0;
    unsafe {
        while *raw.add(len) != 0 {
            len += 1;
        }
    }
    let mut path = PathBuf::from(std::ffi::OsString::from_wide(unsafe {
        std::slice::from_raw_parts(raw, len)
    }));
    unsafe {
        CoTaskMemFree(raw.cast());
    }
    let mut pins = pin_ancestors(&path)?;
    let security = Security::new()?;
    let leaf = format!("instance-{instance}");
    for part in ["MixyardInventory", leaf.as_str()] {
        path.push(part);
        if unsafe { CreateDirectoryW(wide(&path).as_ptr(), &security.attributes()) } == 0
            && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
        {
            return Err("inventory-state-directory-unavailable");
        }
        let file = open(
            &path,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            OPEN_EXISTING,
            ptr::null(),
        )?;
        information(file.0, true)?;
        private_acl(file.0)?;
        pins.push(file);
    }
    Ok(path)
}

pub(super) struct Store {
    path: PathBuf,
    _pins: Vec<Handle>,
}
impl Store {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let pins = pin_ancestors(path)?;
        private_acl(pins.last().unwrap().0)?;
        Ok(Self {
            path: path.into(),
            _pins: pins,
        })
    }
    pub(super) fn read(&self) -> Result<Option<Vec<u8>>> {
        let path = self.path.join("inventory-revision-v1");
        let raw = unsafe {
            CreateFileW(
                wide(&path).as_ptr(),
                GENERIC_READ | READ_CONTROL,
                FILE_SHARE_READ,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return if unsafe { GetLastError() } == ERROR_FILE_NOT_FOUND {
                Ok(None)
            } else {
                Err("inventory-state-unavailable-do-not-reset")
            };
        }
        let file = handle(raw)?;
        let info = information(file.0, false)?;
        private_acl(file.0)?;
        if info.nFileSizeHigh != 0 || info.nFileSizeLow != 80 {
            return Err("inventory-state-insecure-or-corrupt-do-not-reset");
        }
        let raw = file.0;
        mem::forget(file);
        let mut file = unsafe { File::from_raw_handle(raw) };
        let mut state = vec![0; 80];
        file.read_exact(&mut state)
            .map_err(|_| "inventory-state-corrupt-do-not-reset")?;
        Ok(Some(state))
    }
    pub(super) fn write(&self, state: &[u8; 80]) -> Result<()> {
        let temporary = self.path.join(super::inventory_state::temporary_name()?);
        let target = self.path.join("inventory-revision-v1");
        let write = || -> Result<()> {
            let security = Security::new()?;
            let file = open(
                &temporary,
                GENERIC_WRITE | READ_CONTROL,
                0,
                CREATE_NEW,
                &security.attributes(),
            )?;
            information(file.0, false)?;
            private_acl(file.0)?;
            let raw = file.0;
            mem::forget(file);
            let mut file = unsafe { File::from_raw_handle(raw) };
            file.write_all(state)
                .and_then(|_| file.sync_all())
                .map_err(|_| "inventory-state-atomic-write-failed-do-not-reset")?;
            drop(file);
            if unsafe {
                MoveFileExW(
                    wide(&temporary).as_ptr(),
                    wide(&target).as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err("inventory-state-atomic-write-failed-do-not-reset");
            }
            Ok(())
        };
        if write().is_err() {
            let _ = std::fs::remove_file(&temporary);
            return Err("inventory-state-atomic-write-failed-do-not-reset");
        }
        Ok(())
    }
}

struct Pipe {
    read: Handle,
    write: Handle,
}
fn pipe() -> Result<Pipe> {
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    let security = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    if unsafe { CreatePipe(&mut read, &mut write, &security, 0) } == 0 {
        return Err("native-inventory-pipe-unavailable");
    }
    let pipe = Pipe {
        read: handle(read)?,
        write: handle(write)?,
    };
    if unsafe { SetHandleInformation(pipe.read.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err("native-inventory-pipe-unavailable");
    }
    Ok(pipe)
}
struct Attributes {
    storage: Vec<usize>,
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.storage.as_mut_ptr().cast());
        }
    }
}

pub(super) struct Process {
    process: Handle,
    job: Handle,
    stdout: Handle,
    stderr: Handle,
    _installation: Vec<Handle>,
}
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            TerminateJobObject(self.job.0, 1);
            WaitForSingleObject(self.process.0, INFINITE);
        }
    }
}
impl Process {
    pub(super) fn start(fixture: Option<&Path>) -> Result<Self> {
        let (path, installation) = if let Some(path) = fixture {
            if !path.is_absolute() {
                return Err("inventory-fixture-requires-absolute-path");
            }
            (path.to_path_buf(), Vec::new())
        } else {
            let path = std::env::current_exe()
                .map_err(|_| "native-inventory-install-location-unavailable")?
                .with_file_name("mixyard-windows-inventory.exe");
            let mut pins = pin_ancestors(path.parent().unwrap())?;
            private_acl(pins.last().unwrap().0)?;
            let file = open(
                &path,
                GENERIC_READ | READ_CONTROL,
                FILE_SHARE_READ,
                OPEN_EXISTING,
                ptr::null(),
            )?;
            information(file.0, false)?;
            private_acl(file.0)?;
            pins.push(file);
            (path, pins)
        };
        let job = handle(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err("native-inventory-job-unavailable");
        }
        let out = pipe()?;
        let err = pipe()?;
        let inheritable = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: 1,
        };
        let input = handle(unsafe {
            CreateFileW(
                wide("NUL").as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &inheritable,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        })?;
        let mut size = 0;
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
        }
        let mut storage = vec![0usize; size.div_ceil(mem::size_of::<usize>())];
        if unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut size)
        } == 0
        {
            return Err("native-inventory-process-attributes-unavailable");
        }
        let mut attributes = Attributes { storage };
        let mut handles = [input.0, out.write.0, err.write.0];
        if unsafe {
            UpdateProcThreadAttribute(
                attributes.storage.as_mut_ptr().cast(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_mut_ptr().cast(),
                mem::size_of_val(&handles),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        } == 0
        {
            return Err("native-inventory-process-attributes-unavailable");
        }
        let mut startup: STARTUPINFOEXW = unsafe { mem::zeroed() };
        startup.StartupInfo.cb = mem::size_of_val(&startup) as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = input.0;
        startup.StartupInfo.hStdOutput = out.write.0;
        startup.StartupInfo.hStdError = err.write.0;
        startup.lpAttributeList = attributes.storage.as_mut_ptr().cast();
        let mut command = wide(format!(
            "\"{}\" snapshot",
            path.to_str()
                .ok_or("native-inventory-helper-path-invalid")?
        ));
        let mut info: PROCESS_INFORMATION = unsafe { mem::zeroed() };
        if unsafe {
            CreateProcessW(
                wide(&path).as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                ptr::null(),
                ptr::null(),
                &startup.StartupInfo,
                &mut info,
            )
        } == 0
        {
            return Err("native-inventory-helper-unavailable");
        }
        let process = handle(info.hProcess)?;
        let thread = handle(info.hThread)?;
        if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
            unsafe {
                TerminateProcess(process.0, 1);
                WaitForSingleObject(process.0, INFINITE);
            }
            return Err("native-inventory-job-assignment-failed");
        }
        let child = Self {
            process,
            job,
            stdout: out.read,
            stderr: err.read,
            _installation: installation,
        };
        if unsafe { ResumeThread(thread.0) } == u32::MAX {
            return Err("native-inventory-resume-failed");
        }
        Ok(child)
    }
    pub(super) fn poll(&mut self, output: &mut Vec<u8>) -> Result<bool> {
        let mut out_done = false;
        for _ in 0..32 {
            let Some(block) = read_available(self.stdout.0)? else {
                break;
            };
            if block.is_empty() {
                out_done = true;
                break;
            }
            if output.len() + block.len() > super::native_inventory::MAX_OUTPUT {
                return Err("native-inventory-output-exceeds-1MiB");
            }
            output.extend(block);
        }
        let err_done = match read_available(self.stderr.0)? {
            Some(bytes) if bytes.is_empty() => true,
            Some(_) => return Err("native-inventory-diagnostic-output-or-partial-permission"),
            None => false,
        };
        match unsafe { WaitForSingleObject(self.process.0, 0) } {
            WAIT_TIMEOUT => Ok(false),
            WAIT_OBJECT_0 => {
                let mut code = 1;
                if unsafe { GetExitCodeProcess(self.process.0, &mut code) } == 0 || code != 0 {
                    return Err("native-inventory-helper-failed-or-permission-denied");
                }
                Ok(out_done && err_done)
            }
            _ => Err("native-inventory-wait-error"),
        }
    }
}
fn read_available(pipe: HANDLE) -> Result<Option<Vec<u8>>> {
    let mut available = 0;
    if unsafe {
        PeekNamedPipe(
            pipe,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            &mut available,
            ptr::null_mut(),
        )
    } == 0
    {
        return if unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
            Ok(Some(Vec::new()))
        } else {
            Err("native-inventory-pipe-error")
        };
    }
    if available == 0 {
        return Ok(None);
    }
    let mut buffer = vec![0; available.min(65536) as usize];
    let mut count = 0;
    // Anonymous byte pipe, sole reader, and read at most the bytes just peeked.
    if unsafe {
        ReadFile(
            pipe,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            &mut count,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err("native-inventory-pipe-error");
    }
    buffer.truncate(count as usize);
    Ok(Some(buffer))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kernel_private_acl_hardlink_corruption_and_atomic_replacement_checks() {
        let name = format!("state-test-{}", std::process::id());
        let directory = state_directory(&name).unwrap();
        let store = Store::open(&directory).unwrap();
        let first = [1; 80];
        store.write(&first).unwrap();
        let path = directory.join("inventory-revision-v1");
        assert_eq!(store.read().unwrap().unwrap(), first);
        let second = [2; 80];
        store.write(&second).unwrap();
        assert_eq!(store.read().unwrap().unwrap(), second);
        let saved = directory.join("hardlink");
        std::fs::hard_link(&path, &saved).unwrap();
        assert!(store.read().is_err());
        std::fs::remove_file(saved).unwrap();
        std::fs::write(&path, b"corrupt").unwrap();
        assert!(store.read().is_err());
        // Broaden the existing file DACL to prove the kernel metadata gate, not
        // merely equality with a security descriptor we just constructed.
        let file = open(
            &path,
            WRITE_DAC | READ_CONTROL,
            0,
            OPEN_EXISTING,
            ptr::null(),
        )
        .unwrap();
        let text = wide(format!("O:{0}D:P(A;;FA;;;{0})(A;;GR;;;WD)", sid().unwrap()));
        let mut raw = ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    SDDL_REVISION_1,
                    &mut raw,
                    ptr::null_mut(),
                )
            },
            0
        );
        let allocation = Local(raw);
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = ptr::null_mut();
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(allocation.0, &mut present, &mut acl, &mut defaulted)
            },
            0
        );
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    file.0,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    acl,
                    ptr::null_mut(),
                )
            },
            ERROR_SUCCESS
        );
        assert!(private_acl(file.0).is_err());
        drop(file);
        assert!(store.read().is_err());
        let saved_state = directory.join("saved-state");
        std::fs::rename(&path, &saved_state).unwrap();
        match std::os::windows::fs::symlink_file(&saved_state, &path) {
            Ok(()) => {
                assert!(store.read().is_err());
                assert_eq!(std::fs::read(&saved_state).unwrap(), b"corrupt");
                std::fs::remove_file(&path).unwrap();
            }
            Err(error) if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD as i32) => {
                eprintln!("File-reparse fixture unavailable: Windows symlink privilege not held");
            }
            Err(error) => panic!("file reparse fixture: {error}"),
        }
        std::fs::rename(saved_state, &path).unwrap();
        // No delete sharing means a pinned directory cannot be swapped to make
        // subsequent path-based operations reach an attacker's replacement.
        assert!(std::fs::rename(&directory, directory.with_extension("moved")).is_err());
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
