//! Bounded passive local observation; prints potentially private facts only on explicit invocation.
use mixyard::context::{Collector, Options};
use std::sync::atomic::AtomicBool;

fn main() {
    if std::env::args().len() != 1 {
        eprintln!("usage: mixyard-context (read-only local observation; no client facts)");
        std::process::exit(2);
    }
    let result = Collector::new(Options::default())
        .and_then(|mut collector| collector.capture(&AtomicBool::new(false)));
    match result {
        Ok(capture) => match serde_json::to_string_pretty(&capture) {
            Ok(json) => println!("{json}"),
            Err(_) => {
                eprintln!("context serialization failed");
                std::process::exit(4);
            }
        },
        Err(error) => {
            eprintln!("context: {error}");
            std::process::exit(4);
        }
    }
}
