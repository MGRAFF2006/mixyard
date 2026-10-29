use super::*;
use std::{
    os::unix::process::CommandExt,
    process::{Child, ChildStderr, ChildStdout, Command, Stdio},
};
pub(in crate::runtime) struct Process {
    child: Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    _installation: Vec<File>,
}
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}
fn nonblocking(fd: i32) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err("native-inventory-pipe-unavailable");
    }
    Ok(())
}
impl Process {
    pub(in crate::runtime) fn start(fixture: Option<&Path>) -> Result<Self> {
        let (path, installation) = if let Some(path) = fixture {
            if !path.is_absolute() {
                return Err("inventory-fixture-requires-absolute-path");
            }
            (path.to_path_buf(), Vec::new())
        } else {
            let path = std::env::current_exe()
                .map_err(|_| "native-inventory-install-location-unavailable")?
                .with_file_name("mixyard-macos-inventory");
            let mut pins = trusted_ancestors(path.parent().unwrap())?;
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)
                .map_err(|_| "native-inventory-installed-helper-unavailable")?;
            let meta = file
                .metadata()
                .map_err(|_| "native-inventory-installed-helper-unavailable")?;
            if !meta.is_file()
                || meta.nlink() != 1
                || (meta.uid() != 0 && meta.uid() != unsafe { libc::geteuid() })
                || meta.mode() & 0o022 != 0
                || meta.mode() & 0o111 == 0
                || meta.mode() & 0o6000 != 0
            {
                return Err("native-inventory-installed-helper-untrusted");
            }
            no_extra_acl(&file)?;
            pins.push(file);
            (path, pins)
        };
        let mut child = Command::new(path)
            .arg("snapshot")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| "native-inventory-helper-unavailable")?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let process = Self {
            child,
            stdout,
            stderr,
            _installation: installation,
        };
        nonblocking(process.stdout.as_raw_fd())?;
        nonblocking(process.stderr.as_raw_fd())?;
        Ok(process)
    }
    pub(in crate::runtime) fn poll(&mut self, output: &mut Vec<u8>) -> Result<bool> {
        let mut block = [0; 65536];
        let mut out_done = false;
        for _ in 0..32 {
            match self.stdout.read(&mut block) {
                Ok(0) => {
                    out_done = true;
                    break;
                }
                Ok(n) => {
                    if output.len() + n > super::super::native_inventory::MAX_OUTPUT {
                        return Err("native-inventory-output-exceeds-1MiB");
                    }
                    output.extend_from_slice(&block[..n]);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => return Err("native-inventory-pipe-error"),
            }
        }
        let err_done = match self.stderr.read(&mut block) {
            Ok(0) => true,
            Ok(_) => return Err("native-inventory-diagnostic-output-or-partial-permission"),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => false,
            Err(_) => return Err("native-inventory-pipe-error"),
        };
        let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        if unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } != 0
        {
            return Err("native-inventory-wait-error");
        }
        let info = unsafe { info.assume_init() };
        #[cfg(target_os = "linux")]
        let (pid, status) = unsafe { (info.si_pid(), info.si_status()) };
        #[cfg(target_os = "macos")]
        let (pid, status) = (info.si_pid, info.si_status);
        if pid != 0 {
            if info.si_code != libc::CLD_EXITED || status != 0 {
                return Err("native-inventory-helper-failed-or-permission-denied");
            }
            return Ok(out_done && err_done);
        }
        Ok(false)
    }
}
