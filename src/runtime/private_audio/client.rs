//! Explicit private client; never starts the runtime or chooses startup paths.
use super::protocol::*;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{self, Read, Write},
    time::{Duration, Instant},
};
pub const USAGE:&str = "Usage: mixyard private-audio status --instance I
       mixyard private-audio prepare --instance I --instance-nonce HEX64 --expected-store-revision N --expected-config-revision N
       mixyard private-audio link --instance I --instance-nonce HEX64 --prepared-token TOKEN --output-id ID --node-id N --node-serial N --port-id N --port-serial N
       mixyard private-audio apply --instance I --instance-nonce HEX64 --prepared-token TOKEN --expected-store-revision N --expected-config-revision N --expected-inventory-revision N
       mixyard private-audio stop --instance I --instance-nonce HEX64 --control-epoch N --active-generation N|null --pending-operation-id ID|null --reserved-generation N|null
Linux owned-private-generated-v1 only. Start the runtime explicitly with an owned descriptor and canonical profiles directory. Prepare publishes endpoints; link uses observed private consumer IDs/serials. Apply requires native commit receipts. Stop requires the exact cached stopTarget and normal child reaping. A request timeout/disconnect leaves the operation owned; inspect status. After Stop, Prepare explicitly starts fresh private children for the next revision.";
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
pub fn endpoint(instance: &str) -> io::Result<String> {
    let lifecycle = crate::runtime::endpoint(instance)?;
    Ok(std::path::Path::new(&lifecycle)
        .with_file_name("private-audio.sock")
        .to_string_lossy()
        .into_owned())
}

pub fn parse(args: &[OsString]) -> io::Result<(String, Request)> {
    let Some(command) = args.first().and_then(|s| s.to_str()) else {
        return Err(invalid(
            "private-audio requires status|prepare|link|apply|stop",
        ));
    };
    let (pairs, remainder) = args[1..].as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(invalid("private-audio flags require values"));
    }
    let mut flags = BTreeMap::new();
    for pair in pairs {
        let key = pair[0].to_str().ok_or_else(|| invalid("non UTF-8 flag"))?;
        let value = pair[1].to_str().ok_or_else(|| invalid("non UTF-8 value"))?;
        if !key.starts_with("--") || flags.insert(key, value).is_some() {
            return Err(invalid("invalid or duplicate private-audio flag"));
        }
    }
    let instance = flags
        .remove("--instance")
        .ok_or_else(|| invalid("--instance is required"))?
        .to_owned();
    endpoint(&instance)?;
    fn take(map: &mut BTreeMap<&str, &str>, key: &str) -> io::Result<String> {
        map.remove(key)
            .map(str::to_owned)
            .ok_or_else(|| invalid(format!("missing {key}")))
    }
    fn number(map: &mut BTreeMap<&str, &str>, key: &str) -> io::Result<u64> {
        take(map, key)?
            .parse()
            .map_err(|_| invalid(format!("invalid {key}")))
    }
    fn optional(map: &mut BTreeMap<&str, &str>, key: &str) -> io::Result<Option<u64>> {
        let value = take(map, key)?;
        if value == "null" {
            Ok(None)
        } else {
            Ok(Some(
                value
                    .parse()
                    .map_err(|_| invalid(format!("invalid {key}")))?,
            ))
        }
    }
    let operation = match command {
        "status" => Operation::Status,
        "prepare" => Operation::Prepare {
            instance_nonce: take(&mut flags, "--instance-nonce")?,
            expected_store_revision: number(&mut flags, "--expected-store-revision")?,
            expected_config_revision: number(&mut flags, "--expected-config-revision")?,
        },
        "link" => Operation::Link {
            instance_nonce: take(&mut flags, "--instance-nonce")?,
            prepared_token: take(&mut flags, "--prepared-token")?,
            output_id: take(&mut flags, "--output-id")?,
            consumer: Endpoint {
                node_id: number(&mut flags, "--node-id")?
                    .try_into()
                    .map_err(|_| invalid("node ID overflow"))?,
                node_serial: number(&mut flags, "--node-serial")?,
                port_id: number(&mut flags, "--port-id")?
                    .try_into()
                    .map_err(|_| invalid("port ID overflow"))?,
                port_serial: number(&mut flags, "--port-serial")?,
            },
        },
        "apply" => Operation::Apply {
            instance_nonce: take(&mut flags, "--instance-nonce")?,
            prepared_token: take(&mut flags, "--prepared-token")?,
            expected_store_revision: number(&mut flags, "--expected-store-revision")?,
            expected_config_revision: number(&mut flags, "--expected-config-revision")?,
            expected_inventory_revision: number(&mut flags, "--expected-inventory-revision")?,
        },
        "stop" => {
            let instance_nonce = take(&mut flags, "--instance-nonce")?;
            let control_epoch = number(&mut flags, "--control-epoch")?;
            let active_generation = optional(&mut flags, "--active-generation")?;
            let op = take(&mut flags, "--pending-operation-id")?;
            let generation = optional(&mut flags, "--reserved-generation")?;
            let pending_install =
                match (op.as_str(), generation) {
                    ("null", None) => None,
                    ("null", Some(_)) | (_, None) => return Err(invalid(
                        "pending operation and reserved generation must both be null or present",
                    )),
                    (_, Some(reserved_generation)) => Some(PendingInstall {
                        operation_id: op,
                        reserved_generation,
                    }),
                };
            Operation::Stop {
                expected: StopTarget {
                    instance_nonce,
                    control_epoch,
                    active_generation,
                    pending_install,
                },
            }
        }
        _ => return Err(invalid("unsupported private-audio command")),
    };
    if !flags.is_empty() {
        return Err(invalid("unknown private-audio flag"));
    }
    let request = Request {
        private_audio_version: VERSION,
        request_id: format!("cli-{}", std::process::id()),
        operation,
    };
    request.validate().map_err(invalid)?;
    if serde_json::to_vec(&request)?.len() > REQUEST_LIMIT {
        return Err(invalid("private request exceeds 8192 bytes"));
    }
    Ok((instance, request))
}
pub fn run(args: &[OsString], output: &mut impl Write) -> io::Result<()> {
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        return writeln!(output, "{USAGE}");
    }
    let (instance, request) = parse(args)?;
    exchange(&instance, &request, output)
}
pub fn exchange(instance: &str, request: &Request, output: &mut impl Write) -> io::Result<()> {
    request.validate().map_err(invalid)?;
    let mut bytes = serde_json::to_vec(request)?;
    if bytes.len() > REQUEST_LIMIT {
        return Err(invalid("private request too large"));
    }
    bytes.push(b'\n');
    let mut stream = crate::runtime::transport::connect(&endpoint(instance)?)?;
    let started = Instant::now();
    let mut sent = 0;
    let mut input = Vec::new();
    loop {
        if sent < bytes.len() {
            match stream.write(&bytes[sent..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => sent += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
        }
        if sent == bytes.len() {
            let mut buffer = [0; 4096];
            match stream.read(&mut buffer) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    input.extend_from_slice(&buffer[..n]);
                    if let Some(end) = input.iter().position(|b| *b == b'\n') {
                        if end > RESULT_LIMIT || end + 1 != input.len() {
                            return Err(invalid("private response frame invalid"));
                        }
                        let reply: Reply = serde_json::from_slice(&input[..end])?;
                        if reply.private_audio_version != VERSION
                            || reply.request_id != request.request_id
                            || reply.result.is_some() == reply.error.is_some()
                        {
                            return Err(invalid("private response envelope invalid"));
                        }
                        output.write_all(&input)?;
                        output.flush()?;
                        return if reply.error.is_some() {
                            Err(io::Error::other(
                                "private request rejected; see JSON response",
                            ))
                        } else {
                            Ok(())
                        };
                    }
                    if input.len() > RESULT_LIMIT {
                        return Err(invalid("private response exceeds 65536 bytes"));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
        }
        if started.elapsed() >= Duration::from_secs(5) {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn exact_stop_tuple_and_closed_flags() {
        let nonce = "a".repeat(64);
        let base = [
            "stop",
            "--instance",
            "test",
            "--instance-nonce",
            &nonce,
            "--control-epoch",
            "2",
            "--active-generation",
            "1",
            "--pending-operation-id",
            "install-3",
            "--reserved-generation",
            "2",
        ];
        let (_, request) = parse(&args(&base)).unwrap();
        let Operation::Stop { expected } = request.operation else {
            panic!()
        };
        assert_eq!(expected.pending_install.unwrap().reserved_generation, 2);
        let mut bad = args(&base);
        bad.extend(args(&["--server", "/bin/false"]));
        assert!(parse(&bad).is_err());
        let mut bad = args(&base);
        bad[10] = "null".into();
        assert!(parse(&bad).is_err());
        assert!(parse(&args(&["status"])).is_err());
        assert!(parse(&args(&["status", "--instance", "x", "--instance", "y"])).is_err());
    }
    #[test]
    fn closed_wire_rejects_unknown_keys_versions_and_pcm() {
        for raw in [
            r#"{"privateAudioVersion":1,"requestId":"x","operation":{"kind":"status","pcm":[]}}"#,
            r#"{"privateAudioVersion":1,"requestId":"x","operation":{"kind":"status"},"extra":1}"#,
        ] {
            assert!(serde_json::from_str::<Request>(raw).is_err());
        }
        let request: Request = serde_json::from_str(
            r#"{"privateAudioVersion":2,"requestId":"x","operation":{"kind":"status"}}"#,
        )
        .unwrap();
        assert!(request.validate().is_err());
    }
}
