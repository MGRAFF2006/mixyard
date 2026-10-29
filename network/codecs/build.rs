use std::{env, path::Path};

fn setting(key: &str, default: &str) -> String {
    match env::var(key) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => default.into(),
        Err(error) => panic!("{key}: {error}"),
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
        if let Some(dir) = env::var_os(format!("{prefix}_LIB_DIR")) {
            let path = Path::new(&dir);
            let text = path.to_str().expect("library path must be UTF-8");
            assert!(
                path.is_absolute() && path.is_dir() && !text.contains(['\r', '\n']),
                "{prefix}_LIB_DIR must be an existing absolute directory"
            );
            let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
            let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
            let file = match (os.as_str(), target_env.as_str(), kind.as_str()) {
                ("windows", "msvc", _) => format!("{name}.lib"),
                (_, _, "static") => format!("lib{name}.a"),
                ("windows", _, _) => format!("lib{name}.dll.a"),
                ("macos", _, _) => format!("lib{name}.dylib"),
                _ => format!("lib{name}.so"),
            };
            assert!(
                path.join(&file).is_file(),
                "{prefix}_LIB_DIR is missing selected library {file}"
            );
            println!("cargo:rustc-link-search=native={text}");
        }
        println!("cargo:rustc-link-lib={kind}={name}");
    }
}
