use std::{
    collections::hash_map::DefaultHasher,
    env, fs,
    hash::{Hash, Hasher},
    io::Read,
    path::Path,
};

fn setting(key: &str, default: &str) -> String {
    match env::var(key) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => default.into(),
        Err(error) => panic!("{key}: {error}"),
    }
}

fn library_file(name: &str, os: &str, target_env: &str, kind: &str) -> String {
    match (os, target_env, kind) {
        ("windows", "msvc", _) => format!("{name}.lib"),
        (_, _, "static") => format!("lib{name}.a"),
        ("windows", _, _) => format!("lib{name}.dll.a"),
        ("macos", _, _) => format!("lib{name}.dylib"),
        _ => format!("lib{name}.so"),
    }
}

// Link only explicitly selected features. No probing, package installation or
// fallback to a different codec/library is performed by the build.
fn main() {
    for codec in ["ZSTD", "OPUS"] {
        let prefix = format!("MIXYARD_{codec}");
        for suffix in ["LIB_DIR", "LIB_NAME", "LINK_KIND"] {
            println!("cargo:rerun-if-env-changed={prefix}_{suffix}");
        }
        if env::var_os(format!("CARGO_FEATURE_SYSTEM_{codec}")).is_none() {
            continue;
        }
        let kind = setting(&format!("{prefix}_LINK_KIND"), "dylib");
        assert!(
            matches!(kind.as_str(), "dylib" | "static"),
            "{prefix}_LINK_KIND must be dylib or static"
        );
        let name = setting(&format!("{prefix}_LIB_NAME"), &codec.to_ascii_lowercase());
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')),
            "{prefix}_LIB_NAME must be a bare library name without extension"
        );
        let mut linked_name = name.clone();
        if let Some(dir) = env::var_os(format!("{prefix}_LIB_DIR")) {
            let path = Path::new(&dir);
            let text = path.to_str().expect("library path must be UTF-8");
            assert!(
                path.is_absolute() && path.is_dir() && !text.contains(['\r', '\n']),
                "{prefix}_LIB_DIR must be an existing absolute directory"
            );
            let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
            let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
            let file = library_file(&name, &os, &target_env, &kind);
            let selected = path.join(&file);
            assert!(
                selected.is_file(),
                "{prefix}_LIB_DIR is missing selected library {file}"
            );
            let canonical = selected.canonicalize().expect("resolve selected library");
            let canonical_text = canonical
                .to_str()
                .expect("selected library path must be UTF-8");
            assert!(
                !canonical_text.contains(['\r', '\n']),
                "invalid selected library path"
            );
            // Cargo must rerun validation after removal/replacement, including
            // changes to the target of an installed development symlink.
            println!("cargo:rerun-if-changed={text}");
            println!("cargo:rerun-if-changed={}", selected.display());
            println!("cargo:rerun-if-changed={canonical_text}");

            // Native -L paths are global. Two independently selected directories
            // can both contain libopus/libzstd, so bare names can bind the wrong
            // artifact. Give the exact selected bytes a private unique link name.
            // This hash prevents accidental name collisions; SHA256 provenance
            // is recorded separately by the hosted qualification harness.
            let mut hash = DefaultHasher::new();
            canonical.hash(&mut hash);
            kind.hash(&mut hash);
            let mut input = fs::File::open(&canonical).expect("open selected library");
            let mut bytes = [0; 8192];
            loop {
                let length = input.read(&mut bytes).expect("read selected library");
                if length == 0 {
                    break;
                }
                hash.write(&bytes[..length]);
            }
            linked_name = format!(
                "mixyard_codec_{}_selected_{:016x}",
                codec.to_ascii_lowercase(),
                hash.finish()
            );
            // Construct the platform filename from its parts. A custom name
            // such as "lib" can also occur in the platform's "lib" prefix.
            let alias = library_file(&linked_name, &os, &target_env, &kind);
            let output =
                env::var_os("OUT_DIR").expect("Cargo OUT_DIR required for explicit libraries");
            let output = Path::new(&output);
            let output_text = output.to_str().expect("OUT_DIR must be UTF-8");
            assert!(
                output.is_absolute() && !output_text.contains(['\r', '\n']),
                "invalid OUT_DIR"
            );
            fs::copy(&canonical, output.join(alias)).expect("stage exact selected library");
            println!("cargo:rustc-link-search=native={output_text}");
        }
        println!("cargo:rustc-link-lib={kind}={linked_name}");
    }
}
