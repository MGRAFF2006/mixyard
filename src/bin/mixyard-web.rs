//! Local browser console; never owns audio or the runtime lifecycle.
#[path = "../web/mod.rs"]
mod web;

fn main() {
    if let Err(error) = web::run() {
        eprintln!("mixyard-web: {error}");
        std::process::exit(1);
    }
}
