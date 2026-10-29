//! Test executable, never installed under a production helper name.
use std::{fs, io::{self, Write}, process::{self, Command}, thread, time::Duration};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["descendant"] { thread::sleep(Duration::from_secs(60)); return; }
    assert_eq!(args, ["snapshot"]);
    let exe = std::env::current_exe().unwrap();
    fs::write(exe.with_extension("pid"), process::id().to_string()).unwrap();
    let mode = fs::read_to_string(exe.with_extension("mode")).unwrap();
    match mode.as_str() {
        "hang" => {
            let child = Command::new(&exe).arg("descendant").spawn().unwrap();
            fs::write(exe.with_extension("descendant"), child.id().to_string()).unwrap();
            thread::sleep(Duration::from_secs(60));
        },
        "stderr" => { eprint!("private native selector never forward"); },
        "nonzero" => { process::exit(1); },
        "oversize" => { io::stdout().write_all(&vec![b'x'; 1024*1024+1]).unwrap(); return; },
        _ => {},
    }
    let bytes = fs::read(exe.with_extension("json")).unwrap();
    io::stdout().write_all(&bytes).unwrap();
}
