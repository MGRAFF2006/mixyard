use std::{
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::PathBuf,
};
pub type Stream = UnixStream;
fn denied(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
fn uid() -> u32 {
    unsafe { libc::geteuid() }
}
pub fn endpoint(instance: &str) -> io::Result<String> {
    let root = if cfg!(target_os = "macos") {
        "/private/tmp"
    } else {
        "/tmp"
    };
    Ok(format!("{root}/mixyard-{}-{instance}/runtime.sock", uid()))
}
fn directory(path: &std::path::Path, create: bool) -> io::Result<fs::Metadata> {
    if create {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.uid() != uid() || meta.mode() & 0o777 != 0o700 {
        return Err(denied(
            "runtime directory must be a real current-user-owned directory with mode 0700",
        ));
    }
    Ok(meta)
}
fn socket(path: &std::path::Path) -> io::Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_socket() || meta.uid() != uid() || meta.mode() & 0o777 != 0o600 {
        return Err(denied(
            "endpoint must be a real current-user-owned socket with mode 0600",
        ));
    }
    Ok(meta)
}
fn check_peer(stream: &Stream) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    let peer_uid = unsafe {
        let mut cred: libc::ucred = std::mem::zeroed();
        let mut len = std::mem::size_of_val(&cred) as libc::socklen_t;
        if libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        ) != 0
        {
            return Err(io::Error::last_os_error());
        }
        cred.uid
    };
    #[cfg(target_os = "macos")]
    let peer_uid = unsafe {
        let mut peer_uid = 0;
        let mut gid = 0;
        if libc::getpeereid(stream.as_raw_fd(), &mut peer_uid, &mut gid) != 0 {
            return Err(io::Error::last_os_error());
        }
        peer_uid
    };
    if peer_uid != uid() {
        return Err(denied("IPC peer belongs to a different user"));
    }
    stream.set_nonblocking(true)
}
pub struct Listener {
    listener: UnixListener,
    path: PathBuf,
    socket_identity: (u64, u64),
    directory_identity: (u64, u64),
}
impl Listener {
    #[cfg(target_os = "linux")]
    pub fn owned_directory(&self) -> io::Result<PathBuf> {
        let path = self.path.parent().unwrap();
        let meta = directory(path, false)?;
        if (meta.dev(), meta.ino()) != self.directory_identity {
            return Err(denied("runtime directory identity changed"));
        }
        Ok(path.to_path_buf())
    }
    pub fn bind(endpoint: &str) -> io::Result<Self> {
        let path = PathBuf::from(endpoint);
        let parent = path.parent().unwrap();
        let dir = directory(parent, true)?;
        // Never unlink a preexisting endpoint, even a stale socket owned by us.
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "endpoint exists; no automatic stale-path removal",
                ))
            }
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let meta = socket(&path)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            path,
            socket_identity: (meta.dev(), meta.ino()),
            directory_identity: (dir.dev(), dir.ino()),
        })
    }
    pub fn accept(&mut self) -> io::Result<Option<Stream>> {
        match self.listener.accept() {
            Ok((stream, _)) => match check_peer(&stream) {
                Ok(()) => Ok(Some(stream)),
                Err(_) => Ok(None),
            },
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        let parent = self.path.parent().unwrap();
        if let (Ok(dir), Ok(meta)) = (directory(parent, false), socket(&self.path)) {
            if (dir.dev(), dir.ino()) == self.directory_identity
                && (meta.dev(), meta.ino()) == self.socket_identity
            {
                // Only the endpoint created by this listener. Keep the private directory.
                if let Err(e) = fs::remove_file(&self.path) {
                    eprintln!("runtime endpoint cleanup: {e}");
                }
            }
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
    let path = std::path::Path::new(endpoint);
    directory(path.parent().unwrap(), false)?;
    socket(path)?;
    // std's UnixStream::connect can block indefinitely on a full backlog.
    // Make the socket nonblocking before connect; EINPROGRESS has a deadline.
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = unsafe { UnixStream::from_raw_fd(raw) };
    if unsafe { libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    stream.set_nonblocking(true)?;
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = endpoint.as_bytes();
    if bytes.len() >= address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unix endpoint path is too long",
        ));
    }
    for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
        *target = *byte as libc::c_char;
    }
    #[cfg(target_os = "macos")]
    {
        address.sun_len = std::mem::size_of_val(&address) as u8;
    }
    let connected = unsafe {
        libc::connect(
            raw,
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    };
    if connected != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error);
        }
        let mut poll = libc::pollfd {
            fd: raw,
            events: libc::POLLOUT,
            revents: 0,
        };
        let started = std::time::Instant::now();
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if started.elapsed() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let result = unsafe { libc::poll(&mut poll, 1, 10) };
            if result > 0 {
                break;
            }
            if result < 0 {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e);
                }
            }
        }
        if let Some(error) = stream.take_error()? {
            return Err(error);
        }
    }
    check_peer(&stream)?;
    Ok(stream)
}

/// Wake as soon as bounded inventory I/O can continue, including EOF/errors.
/// The caller rechecks cancellation and its total deadline after every wait.
pub(super) fn wait_inventory(
    stream: &Stream,
    writing: bool,
    timeout: std::time::Duration,
) -> io::Result<()> {
    let mut descriptor = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: if writing { libc::POLLOUT } else { libc::POLLIN },
        revents: 0,
    };
    let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
    if unsafe { libc::poll(&mut descriptor, 1, millis) } < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}
