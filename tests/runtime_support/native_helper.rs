//! Test executable, never installed under a production helper name.
use std::{
    fs,
    io::{self, Write},
    process::{self, Command},
    thread,
    time::Duration,
};
fn publish_pid(exe: &std::path::Path, extension: &str, pid: u32) {
    // Readers use existence as the startup signal. Publish only complete bytes,
    // never the empty file between create/truncate and write on another thread.
    let pending = exe.with_extension(format!("{extension}.{}.pending", process::id()));
    fs::write(&pending, pid.to_string()).unwrap();
    fs::rename(pending, exe.with_extension(extension)).unwrap();
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["descendant"] {
        thread::sleep(Duration::from_secs(60));
        return;
    }
    assert_eq!(args, ["snapshot"]);
    let exe = std::env::current_exe().unwrap();
    publish_pid(&exe, "pid", process::id());
    let mode = fs::read_to_string(exe.with_extension("mode")).unwrap();
    match mode.as_str() {
        "hang" => {
            let mut child = Command::new(&exe).arg("descendant").spawn().unwrap();
            publish_pid(&exe, "descendant", child.id());
            child.wait().unwrap();
        }
        "stderr" => {
            eprint!("private native selector never forward");
        }
        "nonzero" => {
            process::exit(1);
        }
        "oversize" => {
            io::stdout()
                .write_all(&vec![b'x'; 1024 * 1024 + 1])
                .unwrap();
            return;
        }
        _ => {}
    }
    let bytes = fs::read(exe.with_extension("json")).unwrap();
    io::stdout().write_all(&bytes).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_markers_are_complete_during_concurrent_reads() {
        let root = std::env::var_os("MIXYARD_NATIVE_FIXTURE_TMP")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let directory = root.join(format!(
            "mixyard-pid-publication-{}-{}",
            process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let exe = directory.join("owned-fixture");
        let writer_exe = exe.clone();
        let writer = thread::spawn(move || {
            for _ in 0..512 {
                for extension in ["pid", "descendant"] {
                    publish_pid(&writer_exe, extension, process::id());
                }
            }
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !writer.is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "fixture marker publication deadline"
            );
            for extension in ["pid", "descendant"] {
                match fs::read_to_string(exe.with_extension(extension)) {
                    Ok(value) => assert_eq!(value.parse::<u32>().unwrap(), process::id()),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => panic!("reading owned startup marker: {error}"),
                }
            }
        }
        writer.join().unwrap();
        for extension in ["pid", "descendant"] {
            assert_eq!(
                fs::read_to_string(exe.with_extension(extension)).unwrap(),
                process::id().to_string()
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
