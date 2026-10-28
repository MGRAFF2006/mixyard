use super::{error, response, Reply};
use hyper::{body::Bytes, StatusCode};
use std::{
    collections::HashMap,
    fs,
    io::{self, Read},
    path::Path,
};

const FILE_LIMIT: usize = 16 * 1024 * 1024;
const TOTAL_LIMIT: usize = 64 * 1024 * 1024;
const FILE_COUNT_LIMIT: usize = 2048;
pub(super) struct Assets(HashMap<String, (Bytes, &'static str)>);
impl Assets {
    pub fn load(root: &Path) -> io::Result<Self> {
        if !root.join("index.html").is_file() {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("Web assets missing at {}. Run scripts/run-web.sh to install/build the interface, or build apps/web and pass --assets DIRECTORY.", root.display())));
        }
        let root = root.canonicalize()?;
        let mut assets = HashMap::new();
        let mut total = 0;
        load_directory(&root, &root, &mut assets, &mut total, 0)?;
        Ok(Self(assets))
    }
    pub fn get(&self, path: &str) -> Reply {
        // Accept only literal, normal URL components. Encoded traversal, platform
        // separators, directories and hidden files never reach a filesystem API.
        if path.contains(['%', '\\'])
            || path
                .split('/')
                .skip(1)
                .any(|part| part.starts_with('.') || part.is_empty())
                && path != "/"
        {
            return error(StatusCode::NOT_FOUND, "not-found", "Static asset not found");
        }
        let path = if path == "/" { "/index.html" } else { path };
        match self.0.get(path) {
            Some((bytes, mime)) => response(StatusCode::OK, mime, bytes.clone()),
            None => error(StatusCode::NOT_FOUND, "not-found", "Static asset not found"),
        }
    }
}
fn load_directory(
    root: &Path,
    directory: &Path,
    assets: &mut HashMap<String, (Bytes, &'static str)>,
    total: &mut usize,
    depth: usize,
) -> io::Result<()> {
    if depth > 16 {
        return Err(io::Error::other("Static assets exceed 16 directory levels"));
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        let kind = metadata.file_type();
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(io::Error::other(
                    "Static assets must not contain reparse points",
                ));
            }
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if kind.is_symlink() || (!kind.is_dir() && !kind.is_file()) {
            return Err(io::Error::other(
                "Static assets must be regular files/directories, without symlinks",
            ));
        }
        if kind.is_dir() {
            load_directory(root, &entry.path(), assets, total, depth + 1)?;
        } else {
            if assets.len() >= FILE_COUNT_LIMIT {
                return Err(io::Error::other("Static assets exceed 2048 files"));
            }
            let mut bytes = Vec::new();
            fs::File::open(entry.path())?
                .take((FILE_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)?;
            *total += bytes.len();
            if bytes.len() > FILE_LIMIT || *total > TOTAL_LIMIT {
                return Err(io::Error::other(
                    "Static assets exceed 16 MiB per file or 64 MiB total",
                ));
            }
            let path = entry.path();
            let key = format!(
                "/{}",
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            );
            let mime = match path.extension().and_then(|x| x.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("js" | "mjs") => "text/javascript; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("json" | "map") => "application/json",
                Some("svg") => "image/svg+xml",
                Some("png") => "image/png",
                Some("ico") => "image/x-icon",
                Some("woff2") => "font/woff2",
                Some("woff") => "font/woff",
                _ => "application/octet-stream",
            };
            assets.insert(key, (bytes.into(), mime));
        }
    }
    Ok(())
}
