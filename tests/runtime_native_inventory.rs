//! Real runtime processes and private IPC on Windows/macOS. No audio operations.
#![cfg(any(windows, target_os = "macos"))]
use mixyard::{
    runtime::{self, ReadOnlyClient, ServerError},
    Evidence, Inventory,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
fn platform() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else {
        "macos"
    }
}
fn fixture() -> Value {
    json!({"schemaVersion":2,"revision":0,"platform":platform(),"evidence":"native-discovery",
        "sources":[{"observedId":"source:0","selector":{"kind":"mic","deviceUid":"exact-端点-9007199254740993"},"origin":"external","format":{"channels":2,"sampleRateHz":44100}}],
        "outputs":[{"observedId":"output:0","target":{"kind":"device","deviceUid":"output"},"format":null}],
        "mediaSessions":[],"limits":{"maxExportedMixes":null,"maxOutputRoutes":null,"maxVirtualInputs":null}})
}
fn helper_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let path =
            std::env::temp_dir().join(format!("mixyard-inventory-compiler-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let binary = path.join(if cfg!(windows) {
            "fixture.exe"
        } else {
            "fixture"
        });
        let status = Command::new("rustc")
            .args(["--edition=2021", "-C", "debuginfo=0", "-O"])
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/runtime_support/native_helper.rs"),
            )
            .arg("-o")
            .arg(&binary)
            .status()
            .unwrap();
        assert!(status.success());
        binary
    })
}
struct Runtime {
    instance: String,
    directory: PathBuf,
    helper: PathBuf,
    child: Option<Child>,
    executable: PathBuf,
    fixture: bool,
}
impl Runtime {
    fn new() -> Self {
        let instance = format!(
            "n-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let directory = std::env::temp_dir().join(format!("mixyard-{instance}"));
        fs::create_dir(&directory).unwrap();
        let helper = directory.join(if cfg!(windows) {
            "fixture.exe"
        } else {
            "fixture"
        });
        fs::copy(helper_binary(), &helper).unwrap();
        let runtime = Self {
            instance,
            directory,
            helper,
            child: None,
            executable: env!("CARGO_BIN_EXE_mixyard-runtime").into(),
            fixture: true,
        };
        runtime.mode("ok");
        runtime.graph(&fixture());
        runtime
    }
    fn mode(&self, mode: &str) {
        fs::write(self.helper.with_extension("mode"), mode).unwrap();
    }
    fn graph(&self, value: &Value) {
        fs::write(self.helper.with_extension("json"), format!("{value}\n")).unwrap();
    }
    fn start(&mut self) {
        let mut command = Command::new(&self.executable);
        command.args(["serve", "--instance", &self.instance]);
        if self.fixture {
            command.arg("--inventory-fixture-helper").arg(&self.helper);
        }
        self.child = Some(
            command
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while ReadOnlyClient::connect(&self.instance).is_err() {
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "runtime exited before startup"
            );
            assert!(Instant::now() < deadline, "runtime startup deadline");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn inventory(&self) -> Inventory {
        ReadOnlyClient::inventory(&self.instance, &AtomicBool::new(false)).unwrap()
    }
    fn error(&self) -> ServerError {
        let start = Instant::now();
        let error = ReadOnlyClient::inventory(&self.instance, &AtomicBool::new(false)).unwrap_err();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "typed client deadline: {error}"
        );
        error
            .get_ref()
            .unwrap()
            .downcast_ref::<ServerError>()
            .unwrap()
            .clone()
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            runtime::client(&self.instance, "shutdown", &mut Vec::new()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while child.try_wait().unwrap().is_none() {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    fn pids(&self) -> (u32, u32) {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let (Ok(parent), Ok(child)) = (
                fs::read_to_string(self.helper.with_extension("pid")),
                fs::read_to_string(self.helper.with_extension("descendant")),
            ) {
                return (parent.parse().unwrap(), child.parse().unwrap());
            }
            assert!(Instant::now() < deadline, "fixture child startup");
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn state_directory(&self) -> PathBuf {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            use windows_sys::Win32::{
                System::Com::CoTaskMemFree,
                UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
            };
            let mut raw = std::ptr::null_mut();
            assert!(
                unsafe {
                    SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, std::ptr::null_mut(), &mut raw)
                } >= 0
            );
            let mut len = 0;
            unsafe {
                while *raw.add(len) != 0 {
                    len += 1;
                }
            }
            let root = PathBuf::from(std::ffi::OsString::from_wide(unsafe {
                std::slice::from_raw_parts(raw, len)
            }));
            unsafe {
                CoTaskMemFree(raw.cast());
            }
            root.join("MixyardInventory")
                .join(format!("instance-{}", self.instance))
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::ffi::OsStrExt;
            let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
            let mut result = std::ptr::null_mut();
            let mut buffer = vec![0u8; 65536];
            assert_eq!(
                unsafe {
                    libc::getpwuid_r(
                        libc::geteuid(),
                        entry.as_mut_ptr(),
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                        &mut result,
                    )
                },
                0
            );
            assert!(!result.is_null());
            let home = unsafe { std::ffi::CStr::from_ptr((*result).pw_dir) };
            PathBuf::from(std::ffi::OsStr::from_bytes(home.to_bytes()))
                .join("Library/Application Support/MixyardInventory")
                .join(&self.instance)
        }
    }
    fn clear_pids(&self) {
        for extension in ["pid", "descendant"] {
            let _ = fs::remove_file(self.helper.with_extension(extension));
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.directory);
        let _ = fs::remove_dir_all(self.state_directory());
    }
}
fn gone(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{Foundation::*, System::Threading::*};
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return unsafe { GetLastError() } == ERROR_INVALID_PARAMETER;
        }
        let done = unsafe { WaitForSingleObject(handle, 0) } == WAIT_OBJECT_0;
        unsafe {
            CloseHandle(handle);
        }
        done
    }
    #[cfg(target_os = "macos")]
    {
        unsafe { libc::kill(pid as i32, 0) != 0 && *libc::__error() == libc::ESRCH }
    }
}
fn reaped(pids: (u32, u32)) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !gone(pids.0) || !gone(pids.1) {
        assert!(
            Instant::now() < deadline,
            "owned helper/descendant still running"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn real_runtime_ipc_fixture_provenance_revision_and_restart() {
    let mut r = Runtime::new();
    r.start();
    let first = r.inventory();
    assert_eq!(first.evidence, Evidence::Simulated);
    assert_eq!(
        first.sources[0].selector,
        mixyard::Selector::Mic {
            device_uid: "exact-端点-9007199254740993".into()
        }
    );
    assert_eq!(first.sources[0].format.unwrap().sample_rate_hz, 44100);
    assert!(first.outputs[0].format.is_none());
    assert!(first.revision > 0);
    assert_eq!(first.revision, r.inventory().revision);
    r.stop();
    r.start();
    assert_eq!(first.revision, r.inventory().revision);
    let mut changed = fixture();
    changed["sources"][0]["selector"]["deviceUid"] = json!("changed");
    r.graph(&changed);
    let next = r.inventory().revision;
    assert!(next > first.revision);
    r.graph(&fixture());
    assert!(r.inventory().revision > next);
    let status = ReadOnlyClient::connect(&r.instance)
        .unwrap()
        .initial_snapshot()
        .clone();
    assert!(!status.availability.backend_available);
    for command in ["apply", "mute", "native-action"] {
        assert!(runtime::client(&r.instance, command, &mut Vec::new()).is_err());
    }
    r.stop();
}
#[test]
fn native_helper_failure_is_unavailable_private_and_preserves_revision() {
    let mut r = Runtime::new();
    r.start();
    let first = r.inventory().revision;
    for mode in ["stderr", "nonzero", "oversize", "hang"] {
        r.mode(mode);
        let error = r.error();
        assert_eq!(error.code, "inventory-unavailable");
        assert!(!error.message.contains("private native"));
        if mode == "hang" {
            reaped(r.pids());
        }
    }
    r.mode("ok");
    for bytes in ["[\n", "{}\n", "{\"schemaVersion\":2,\"schemaVersion\":2}\n"] {
        fs::write(r.helper.with_extension("json"), bytes).unwrap();
        assert_eq!(r.error().code, "inventory-unavailable");
    }
    r.graph(&fixture());
    assert_eq!(r.inventory().revision, first);
    r.stop();
}
#[test]
fn corrupt_and_exhausted_durable_state_fail_closed_over_real_ipc() {
    use sha2::{Digest, Sha256};
    let mut r = Runtime::new();
    r.start();
    let first = r.inventory().revision;
    r.stop();
    let path = r.state_directory().join("inventory-revision-v1");
    let original = fs::read(&path).unwrap();
    assert_eq!(original.len(), 80);
    let mut corrupted = original.clone();
    corrupted[10] ^= 1;
    fs::write(&path, &corrupted).unwrap();
    r.start();
    assert_eq!(r.error().message, "inventory-state-corrupt-do-not-reset");
    r.stop();
    assert_eq!(fs::read(&path).unwrap(), corrupted);
    fs::write(&path, &original).unwrap();
    r.start();
    assert_eq!(r.inventory().revision, first);
    r.stop();
    let mut exhausted = original.clone();
    exhausted[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
    let checksum = Sha256::digest(&exhausted[..48]);
    exhausted[48..].copy_from_slice(&checksum);
    fs::write(&path, &exhausted).unwrap();
    let mut changed = fixture();
    changed["sources"][0]["selector"]["deviceUid"] = json!("changed");
    r.graph(&changed);
    r.start();
    assert_eq!(r.error().message, "inventory-revision-exhausted");
    r.stop();
    assert_eq!(fs::read(&path).unwrap(), exhausted);
    fs::write(&path, &original).unwrap();
    let hardlink = r.state_directory().join("hardlink");
    fs::hard_link(&path, &hardlink).unwrap();
    r.start();
    assert_eq!(r.error().code, "inventory-unavailable");
    r.stop();
    fs::remove_file(hardlink).unwrap();
}
#[test]
fn cancelled_client_and_shutdown_kill_owned_job_or_process_group() {
    let mut r = Runtime::new();
    r.mode("hang");
    r.start();
    for shutdown in [false, true] {
        r.clear_pids();
        let instance = r.instance.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let client = thread::spawn(move || ReadOnlyClient::inventory(&instance, &signal));
        let pids = r.pids();
        assert_eq!(r.error().code, "busy");
        assert!(ReadOnlyClient::connect(&r.instance).is_ok());
        if shutdown {
            r.stop();
        } else {
            cancel.store(true, Ordering::Release);
        }
        assert!(client.join().unwrap().is_err());
        reaped(pids);
    }
}
#[test]
fn large_native_schema_snapshot_uses_real_bounded_inventory_ipc() {
    let mut r = Runtime::new();
    let mut value = fixture();
    let mut sources = Vec::new();
    for i in 0..150 {
        let mut s = value["sources"][0].clone();
        s["observedId"] = json!(format!("source:{i}"));
        s["selector"]["deviceUid"] = json!(format!("native-device-{i}"));
        sources.push(s);
    }
    value["sources"] = json!(sources);
    r.graph(&value);
    r.start();
    let inventory = r.inventory();
    assert_eq!(inventory.sources.len(), 150);
    assert!(serde_json::to_vec(&inventory).unwrap().len() > runtime::MAX_FRAME);
    r.stop();
}
#[test]
fn installed_native_sdk_helper_to_real_runtime_ipc() {
    let Some(executable) = std::env::var_os("MIXYARD_TEST_NATIVE_RUNTIME") else {
        eprintln!("installed native helper proof runs in hosted native CI");
        return;
    };
    let mut r = Runtime::new();
    r.fixture = false;
    r.executable = executable.into();
    r.start();
    // Accept a real OS discovery failure, but never a missing installation,
    // insecure state, malformed schema, or an unimplemented provider.
    match ReadOnlyClient::inventory(&r.instance, &AtomicBool::new(false)) {
        Ok(first) => {
            assert_eq!(first.evidence, Evidence::NativeDiscovery);
            assert!(first.revision > 0);
            let revision = r.inventory().revision;
            assert!(revision >= first.revision);
            r.stop();
            r.start();
            assert!(r.inventory().revision >= revision);
        }
        Err(error) => {
            let error = error
                .get_ref()
                .unwrap()
                .downcast_ref::<ServerError>()
                .unwrap();
            assert_eq!(error.code, "inventory-unavailable");
            assert!(
                matches!(
                    error.message.as_str(),
                    "native-inventory-helper-failed-or-permission-denied"
                        | "native-inventory-diagnostic-output-or-partial-permission"
                ),
                "unexpected provider failure: {error:?}"
            );
        }
    }
    assert!(
        !ReadOnlyClient::connect(&r.instance)
            .unwrap()
            .initial_snapshot()
            .availability
            .backend_available
    );
    r.stop();
}
