//! Native named pipes with a protected current-user SID DACL, no remote clients.
//! All pipe I/O is overlapped, polled by the bounded lifecycle loop.
use std::{
    io::{self, Read, Write},
    mem, ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::{Pipes::*, Threading::*, IO::*},
};
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn win_error() -> io::Error {
    io::Error::last_os_error()
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn handle(raw: HANDLE) -> io::Result<Handle> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        Err(win_error())
    } else {
        Ok(Handle(raw))
    }
}
struct Local(*mut core::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn process_sid(process: HANDLE) -> io::Result<String> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return Err(win_error());
        }
        let token = handle(token)?;
        let mut len = 0;
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut len);
        if len == 0 {
            return Err(win_error());
        }
        // Aligned storage for TOKEN_USER and its trailing SID.
        let mut buffer = vec![0usize; (len as usize).div_ceil(mem::size_of::<usize>())];
        if GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            len,
            &mut len,
        ) == 0
        {
            return Err(win_error());
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(win_error());
        }
        let allocation = Local(text.cast());
        let mut end = 0;
        while *text.add(end) != 0 {
            end += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, end));
        drop(allocation);
        Ok(sid)
    }
}
fn current_sid() -> io::Result<String> {
    process_sid(unsafe { GetCurrentProcess() })
}
pub fn endpoint(instance: &str) -> io::Result<String> {
    Ok(format!(r"\\.\pipe\mixyard-{}-{instance}", current_sid()?))
}
fn check_process(pid: u32) -> io::Result<()> {
    let process = handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
    if process_sid(process.0)? != current_sid()? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "IPC peer belongs to another user",
        ));
    }
    Ok(())
}
struct Operation {
    // Heap addresses remain stable until completion/cancellation, including Drop.
    overlapped: Box<OVERLAPPED>,
    event: Handle,
    buffer: Vec<u8>,
    pending: bool,
}
impl Operation {
    fn new(size: usize) -> io::Result<Self> {
        let event = handle(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) })?;
        let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { mem::zeroed() });
        overlapped.hEvent = event.0;
        Ok(Self {
            overlapped,
            event,
            buffer: vec![0; size],
            pending: false,
        })
    }
    fn reset(&mut self) {
        unsafe {
            ResetEvent(self.event.0);
        }
    }
    fn completed(&mut self, pipe: HANDLE) -> io::Result<u32> {
        let mut bytes = 0;
        if unsafe { GetOverlappedResult(pipe, &*self.overlapped, &mut bytes, 0) } != 0 {
            self.pending = false;
            Ok(bytes)
        } else {
            let error = win_error();
            if error.raw_os_error() == Some(ERROR_IO_INCOMPLETE as i32) {
                Err(io::ErrorKind::WouldBlock.into())
            } else {
                self.pending = false;
                Err(error)
            }
        }
    }
    fn cancel(&mut self, pipe: HANDLE) {
        if self.pending {
            unsafe {
                CancelIoEx(pipe, &*self.overlapped);
                // Cancellation completion must precede freeing OVERLAPPED/buffer.
                let mut bytes = 0;
                GetOverlappedResult(pipe, &*self.overlapped, &mut bytes, 1);
            }
            self.pending = false;
        }
    }
}
pub struct Stream {
    pipe: Handle,
    read: Operation,
    read_offset: usize,
    read_available: usize,
    write: Operation,
    server: bool,
}
impl Stream {
    fn new(pipe: Handle, server: bool) -> io::Result<Self> {
        Ok(Self {
            pipe,
            // Inventory clients request bounded 64 KiB chunks. Request/event
            // callers retain their smaller slices and separate frame limits.
            read: Operation::new(64 * 1024)?,
            read_offset: 0,
            read_available: 0,
            write: Operation::new(0)?,
            server,
        })
    }
}
impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.read_available != 0 {
            let n = buf.len().min(self.read_available);
            buf[..n].copy_from_slice(&self.read.buffer[self.read_offset..self.read_offset + n]);
            self.read_offset += n;
            self.read_available -= n;
            return Ok(n);
        }
        let result = if self.read.pending {
            self.read.completed(self.pipe.0)
        } else {
            self.read.reset();
            let mut bytes = 0;
            let len = buf.len().min(self.read.buffer.len()) as u32;
            if unsafe {
                ReadFile(
                    self.pipe.0,
                    self.read.buffer.as_mut_ptr(),
                    len,
                    &mut bytes,
                    &mut *self.read.overlapped,
                )
            } != 0
            {
                Ok(bytes)
            } else {
                let e = win_error();
                if e.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                    self.read.pending = true;
                    Err(io::ErrorKind::WouldBlock.into())
                } else {
                    Err(e)
                }
            }
        };
        match result {
            Ok(n) => {
                // The caller may provide a smaller slice while an earlier
                // overlapped read is pending. Retain its unconsumed bytes.
                let count = buf.len().min(n as usize);
                buf[..count].copy_from_slice(&self.read.buffer[..count]);
                self.read_offset = count;
                self.read_available = n as usize - count;
                Ok(count)
            }
            Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => Ok(0),
            Err(e) => Err(e),
        }
    }
}
impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.write.pending {
            return self.write.completed(self.pipe.0).map(|n| n as usize);
        }
        self.write.reset();
        self.write.buffer.clear();
        self.write.buffer.extend_from_slice(buf);
        let mut bytes = 0;
        if unsafe {
            WriteFile(
                self.pipe.0,
                self.write.buffer.as_ptr(),
                buf.len() as u32,
                &mut bytes,
                &mut *self.write.overlapped,
            )
        } != 0
        {
            Ok(bytes as usize)
        } else {
            let e = win_error();
            if e.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                self.write.pending = true;
                Err(io::ErrorKind::WouldBlock.into())
            } else {
                Err(e)
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.read.cancel(self.pipe.0);
        self.write.cancel(self.pipe.0);
        if self.server {
            unsafe {
                DisconnectNamedPipe(self.pipe.0);
            }
        }
    }
}
fn create_pipe(name: &str, first: bool) -> io::Result<Handle> {
    unsafe {
        let sddl = wide(&format!("O:{0}D:P(A;;GA;;;{0})", current_sid()?));
        let mut descriptor = ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        ) == 0
        {
            return Err(win_error());
        }
        let allocation = Local(descriptor);
        let security = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let flags = PIPE_ACCESS_DUPLEX
            | FILE_FLAG_OVERLAPPED
            | if first {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                0
            };
        let pipe = handle(CreateNamedPipeW(
            wide(name).as_ptr(),
            flags,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            17,
            8192,
            8192,
            0,
            &security,
        ));
        drop(allocation);
        pipe
    }
}
pub struct Listener {
    name: String,
    pending: Option<Handle>,
    connect: Operation,
    disconnected: bool,
}
impl Listener {
    pub fn bind(endpoint: &str) -> io::Result<Self> {
        let mut listener = Self {
            name: endpoint.into(),
            pending: Some(create_pipe(endpoint, true)?),
            connect: Operation::new(0)?,
            disconnected: false,
        };
        listener.start_connect()?;
        Ok(listener)
    }
    fn start_connect(&mut self) -> io::Result<()> {
        self.disconnected = false;
        self.connect.reset();
        let pipe = self.pending.as_ref().unwrap();
        if unsafe { ConnectNamedPipe(pipe.0, &mut *self.connect.overlapped) } != 0 {
            return Ok(());
        }
        let e = win_error();
        match e.raw_os_error().map(|v| v as u32) {
            Some(ERROR_IO_PENDING) => {
                self.connect.pending = true;
                Ok(())
            }
            Some(ERROR_PIPE_CONNECTED) => Ok(()),
            // A client may open and close the new instance before this call.
            // Retain it until accept creates a replacement, without retrying here.
            Some(ERROR_NO_DATA) => {
                self.disconnected = true;
                Ok(())
            }
            _ => Err(e),
        }
    }
    pub fn accept(&mut self) -> io::Result<Option<Stream>> {
        if self.connect.pending {
            match self.connect.completed(self.pending.as_ref().unwrap().0) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e)
                    if matches!(
                        e.raw_os_error().map(|n| n as u32),
                        Some(ERROR_NO_DATA | ERROR_BROKEN_PIPE)
                    ) =>
                {
                    self.disconnected = true
                }
                Err(e) => return Err(e),
            }
        }
        // Maintain an instance continuously; never release the name between clients.
        let next = create_pipe(&self.name, false)?;
        let pipe = self.pending.replace(next).unwrap();
        let disconnected = self.disconnected;
        self.start_connect()?;
        if disconnected {
            return Ok(None);
        }
        let mut pid = 0;
        if unsafe { GetNamedPipeClientProcessId(pipe.0, &mut pid) } == 0 {
            return Ok(None);
        }
        if check_process(pid).is_err() {
            return Ok(None);
        }
        Ok(Some(Stream::new(pipe, true)?))
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(pipe) = &self.pending {
            self.connect.cancel(pipe.0);
        }
    }
}
pub fn connect(endpoint: &str) -> io::Result<Stream> {
    connect_cancellable(
        endpoint,
        &std::sync::atomic::AtomicBool::new(false),
        super::CLIENT_TIMEOUT,
    )
}

pub fn connect_cancellable(
    endpoint: &str,
    cancelled: &std::sync::atomic::AtomicBool,
    deadline: std::time::Duration,
) -> io::Result<Stream> {
    use std::sync::atomic::Ordering;
    if cancelled.load(Ordering::Acquire) {
        return Err(io::ErrorKind::Interrupted.into());
    }
    let started = std::time::Instant::now();
    let pipe = loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let result = handle(unsafe {
            CreateFileW(
                wide(endpoint).as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        });
        match result {
            Ok(pipe) => break pipe,
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && started.elapsed() < deadline =>
            {
                std::thread::sleep(super::TICK)
            }
            Err(e) => return Err(e),
        }
    };
    let mut pid = 0;
    if unsafe { GetNamedPipeServerProcessId(pipe.0, &mut pid) } == 0 {
        return Err(win_error());
    }
    check_process(pid)?;
    Stream::new(pipe, false)
}

#[cfg(test)]
#[test]
fn actual_early_disconnect_before_connect_preserves_admission_and_ownership() {
    let name = endpoint(&format!("early-close-{}", std::process::id())).unwrap();
    // Deliberately leave the actual kernel instance unarmed: no timing race,
    // test hook, or simulated ConnectNamedPipe result.
    let mut listener = Listener {
        name: name.clone(),
        pending: Some(create_pipe(&name, true).unwrap()),
        connect: Operation::new(0).unwrap(),
        disconnected: false,
    };
    drop(connect(&name).unwrap());
    assert_eq!(
        unsafe {
            ConnectNamedPipe(
                listener.pending.as_ref().unwrap().0,
                &mut *listener.connect.overlapped,
            )
        },
        0
    );
    assert_eq!(win_error().raw_os_error(), Some(ERROR_NO_DATA as i32));
    // Exercise production admission on that same disconnected kernel instance.
    listener.start_connect().unwrap();
    assert!(listener.disconnected);
    assert!(!listener.connect.pending);
    assert_eq!(
        create_pipe(&name, true).err().unwrap().raw_os_error(),
        Some(ERROR_ACCESS_DENIED as i32)
    );
    assert!(listener.accept().unwrap().is_none());
    assert!(listener.connect.pending);
    assert!(!listener.disconnected);
    assert_eq!(
        create_pipe(&name, true).err().unwrap().raw_os_error(),
        Some(ERROR_ACCESS_DENIED as i32)
    );

    let mut client = connect(&name).unwrap();
    let started = std::time::Instant::now();
    let mut server = loop {
        if let Some(stream) = listener.accept().unwrap() {
            break stream;
        }
        assert!(started.elapsed() < super::CLIENT_TIMEOUT);
        std::thread::sleep(super::TICK);
    };
    loop {
        match client.write(b"x") {
            Ok(n) => {
                assert_eq!(n, 1);
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("recovered pipe write: {e}"),
        }
        assert!(started.elapsed() < super::CLIENT_TIMEOUT);
        std::thread::sleep(super::TICK);
    }
    let mut byte = [0];
    loop {
        match server.read(&mut byte) {
            Ok(n) => {
                assert_eq!(n, 1);
                assert_eq!(&byte, b"x");
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("recovered pipe read: {e}"),
        }
        assert!(started.elapsed() < super::CLIENT_TIMEOUT);
        std::thread::sleep(super::TICK);
    }
}

#[cfg(test)]
#[test]
fn actual_named_pipe_has_only_current_user_protected_dacl() {
    let name = endpoint(&format!("acl-{}", std::process::id())).unwrap();
    let listener = Listener::bind(&name).unwrap();
    unsafe {
        let mut owner = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        assert_eq!(
            GetSecurityInfo(
                listener.pending.as_ref().unwrap().0,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor
            ),
            ERROR_SUCCESS
        );
        let allocation = Local(descriptor);
        let mut control = 0;
        let mut revision = 0;
        assert_ne!(
            GetSecurityDescriptorControl(descriptor, &mut control, &mut revision),
            0
        );
        assert_ne!(control & SE_DACL_PROTECTED, 0);
        assert!(!dacl.is_null());
        assert_eq!(
            (*dacl).AceCount,
            1,
            "no grants to Everyone, groups or anonymous clients"
        );
        let mut ace = ptr::null_mut();
        assert_ne!(GetAce(dacl, 0, &mut ace), 0);
        let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
        assert_eq!(ace.Header.AceType, 0); // ACCESS_ALLOWED_ACE_TYPE
        assert_eq!(ace.Header.AceFlags, 0);
        let mut sid = ptr::null_mut();
        assert_ne!(
            ConvertStringSidToSidW(wide(&current_sid().unwrap()).as_ptr(), &mut sid),
            0
        );
        let sid_allocation = Local(sid);
        assert_ne!(EqualSid(owner, sid), 0);
        assert_ne!(
            EqualSid((&ace.SidStart as *const u32).cast_mut().cast(), sid),
            0
        );
        drop(sid_allocation);
        drop(allocation);
    }
}
