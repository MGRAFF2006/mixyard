//! Owned production HTTP process fixture.
#![allow(dead_code)]
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
static NEXT: AtomicU32 = AtomicU32::new(0);
pub struct Fixture {
    pub root: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "mixyard-web-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("assets")).unwrap();
        fs::write(
            root.join("assets/index.html"),
            "<!doctype html><title>Web fixture</title>",
        )
        .unwrap();
        fs::write(root.join("assets/app.js"), "console.log('fixture');").unwrap();
        fs::write(root.join("secret.txt"), "outside secret").unwrap();
        Self { root }
    }
    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mixyard-web"));
        command
            .arg("--store")
            .arg(self.root.join("store"))
            .arg("--assets")
            .arg(self.root.join("assets"))
            .args(["--port", "0", "--instance", &self.instance()]);
        command
    }
    pub fn instance(&self) -> String {
        format!(
            "web-{}-{}",
            std::process::id(),
            self.root
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .rsplit('-')
                .next()
                .unwrap()
        )
    }
    pub fn start(&self) -> Service {
        self.start_command(self.command())
    }
    pub fn start_command(&self, mut command: Command) -> Service {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(stdout).read_line(&mut line).unwrap();
            let _ = tx.send(line);
        });
        let mut service = Service {
            child,
            address: String::new(),
            token: String::new(),
        };
        let line = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("web startup deadline");
        service.address = line
            .trim()
            .strip_prefix("Mixyard web: http://")
            .expect("printed URL")
            .into();
        assert!(service.address.starts_with("127.0.0.1:"));
        let session = service.request("GET", "/api/session", &[], "");
        assert_eq!(session.status, 200);
        assert_eq!(session.json()["apiVersion"], 1);
        assert_eq!(session.json()["audioAvailable"], false);
        service.token = session.json()["csrfToken"].as_str().unwrap().into();
        assert!(service.token.len() >= 32);
        service
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
pub struct Service {
    child: Child,
    pub address: String,
    pub token: String,
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub struct Reply {
    pub status: u16,
    pub headers: String,
    pub body: Vec<u8>,
}
impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}
impl Service {
    pub fn request(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
        request(&self.address, method, path, headers, body)
    }
    pub fn post(&self, path: &str, body: Value) -> Reply {
        self.request(
            "POST",
            path,
            &[
                ("X-Mixyard-Token", &self.token),
                ("Content-Type", "application/json"),
            ],
            &body.to_string(),
        )
    }
    pub fn ok(&self, path: &str, body: Value) -> Value {
        let reply = self.post(path, body);
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        reply.json()
    }
    pub fn edit(&self, revision: u64, edit: Value) -> Value {
        self.ok(
            "/api/profiles/edit",
            json!({"expectedRevision":revision,"edit":edit}),
        )
    }
}
pub fn request(
    address: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Reply {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let host = if headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        String::new()
    } else {
        format!("Host: {address}\r\n")
    };
    let extra: String = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\n{host}{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    let _ = stream.write_all(body.as_bytes());
    let mut bytes = Vec::new();
    match stream.read_to_end(&mut bytes) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset && !bytes.is_empty() => {}
        Err(e) => panic!("HTTP response: {e}"),
    }
    let split = bytes
        .windows(4)
        .position(|x| x == b"\r\n\r\n")
        .expect("HTTP response headers");
    let headers = String::from_utf8(bytes[..split].to_vec()).unwrap();
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    Reply {
        status,
        headers,
        body: bytes[split + 4..].to_vec(),
    }
}
