//! Local browser console and reviewed stored-profile automation; never owns audio.
#[path = "../web/mod.rs"]
mod web;

fn main() {
    if let Err(error) = web::run() {
        eprintln!("mixyard-web: {error}");
        std::process::exit(1);
    }
}
