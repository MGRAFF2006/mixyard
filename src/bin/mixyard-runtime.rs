use std::process::ExitCode;
fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!("Usage: mixyard-runtime serve [--instance NAME]\nForeground lifecycle daemon. No audio backend or automatic startup.");
        return ExitCode::SUCCESS;
    }
    let instance = match args.as_slice() {
        [command] if command == "serve" => "default",
        [command, flag, name] if command == "serve" && flag == "--instance" => name,
        _ => {
            eprintln!("Usage: mixyard-runtime serve [--instance NAME]");
            return ExitCode::from(2);
        }
    };
    match mixyard::runtime::serve(instance) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::json!({"ipcVersion":1,"requestId":null,"error":{"code":"runtime-io-error","message":e.to_string()}})
            );
            ExitCode::from(4)
        }
    }
}
