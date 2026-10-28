//! Internal private-local-store path checks shared with the legacy profile store.
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

#[derive(Debug)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub exit_code: u8,
}
impl Error {
    pub(crate) fn new(code: &'static str, message: impl Into<String>, exit_code: u8) -> Self {
        Self {
            code,
            message: message.into(),
            exit_code,
        }
    }
    pub(crate) fn io_at(operation: &str, path: &Path, error: io::Error) -> Self {
        Self::io(format!("{operation} {}: {error}", path.display()))
    }
    pub(crate) fn io(error: impl std::fmt::Display) -> Self {
        Self::new("io-error", error.to_string(), 4)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}

pub(crate) fn check_directory(directory: &Path, names: &[&str]) -> Result<bool, Error> {
    check_ancestors(directory)?;
    let meta = match fs::symlink_metadata(directory) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(Error::io_at("inspect directory", directory, e)),
    };
    if !meta.is_dir() {
        return Err(unsafe_path(directory));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(unsafe_path(directory));
        }
    }
    for entry in
        fs::read_dir(directory).map_err(|e| Error::io_at("list directory", directory, e))?
    {
        let entry = entry.map_err(|e| Error::io_at("read directory entry", directory, e))?;
        if !names.iter().any(|name| entry.file_name() == *name) {
            return Err(Error::new(
                "foreign-store-path",
                "Store directory contains unrelated files; use a dedicated empty directory",
                4,
            ));
        }
        let path = entry.path();
        // Windows supplies non-following metadata with the directory entry.
        // Reopening a disappearing sentinel can return AccessDenied while it is
        // pending deletion (even though directory enumeration succeeded).
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Error::io_at("inspect directory entry", &path, e)),
        };
        check_enumerated_metadata(&path, &meta)?;
    }
    Ok(true)
}
fn unsafe_path(path: &Path) -> Error {
    Error::new("unsafe-store-path", format!("{}: expected a private directory or regular single-link file, without symlinks/reparse points", path.display()), 4)
}
pub(crate) fn check_ancestors(path: &Path) -> Result<(), Error> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if is_link(&meta) || !meta.is_dir() {
                    return Err(unsafe_path(ancestor));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io_at("inspect ancestor", ancestor, e)),
        }
    }
    Ok(())
}
fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        } // FILE_ATTRIBUTE_REPARSE_POINT
    }
    meta.file_type().is_symlink()
}
fn unlinked(meta: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.nlink() == 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}
fn check_enumerated_metadata(path: &Path, meta: &fs::Metadata) -> Result<(), Error> {
    // A successful stat can describe an inode unlinked during the syscall.
    // It is no longer an entry to validate. Direct data access checks its path
    // again below; this does not admit a reachable hardlink or unsafe file.
    if unlinked(meta) {
        return Ok(());
    }
    check_file_metadata(path, meta)
}
pub(crate) fn check_file(path: &Path) -> Result<bool, Error> {
    check_file_with(path, || fs::symlink_metadata(path))
}
fn check_file_with(
    path: &Path,
    mut inspect: impl FnMut() -> io::Result<fs::Metadata>,
) -> Result<bool, Error> {
    for _ in 0..2 {
        let meta = match inspect() {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(Error::io_at("inspect file", path, e)),
        };
        if unlinked(&meta) {
            // Do not turn an old, unlinked profiles.json into an empty store:
            // inspect the current path once more, including all safety checks.
            continue;
        }
        check_file_metadata(path, &meta)?;
        return Ok(true);
    }
    Err(Error::new(
        "store-busy",
        format!(
            "{} changed during file inspection; reread before retrying",
            path.display()
        ),
        5,
    ))
}
fn check_file_metadata(path: &Path, meta: &fs::Metadata) -> Result<(), Error> {
    if is_link(meta) || !meta.is_file() {
        return Err(unsafe_path(path));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.nlink() != 1 || meta.permissions().mode() & 0o077 != 0 {
            return Err(unsafe_path(path));
        }
    }
    Ok(())
}
pub(crate) fn create_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
pub(crate) fn create_directory(path: &Path) -> Result<(), Error> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|e| Error::io_at("create directory", path, e))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::PathBuf;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut nonce = [0u8; 8];
            getrandom::fill(&mut nonce).unwrap();
            let path = std::env::temp_dir().join(format!(
                "mixyard-unlinked-{}-{}",
                std::process::id(),
                u64::from_ne_bytes(nonce)
            ));
            create_directory(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn vanished_metadata_is_skipped_but_direct_paths_are_revalidated_and_bounded() {
        let fixture = Fixture::new();
        let path = fixture.0.join("profiles.json");
        let removed = create_file(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(removed.metadata().unwrap().nlink(), 0);
        check_enumerated_metadata(&path, &removed.metadata().unwrap()).unwrap();
        assert!(!check_file(&path).unwrap());

        let mut replacement = create_file(&path).unwrap();
        replacement.write_all(b"replacement").unwrap();
        let mut calls = 0;
        assert!(check_file_with(&path, || {
            calls += 1;
            if calls == 1 {
                removed.metadata()
            } else {
                fs::symlink_metadata(&path)
            }
        })
        .unwrap());
        assert_eq!(calls, 2);
        assert_eq!(fs::read(&path).unwrap(), b"replacement");

        calls = 0;
        let error = check_file_with(&path, || {
            calls += 1;
            removed.metadata()
        })
        .unwrap_err();
        assert_eq!(calls, 2);
        assert_eq!(error.code, "store-busy");

        // A replacement that is reachable but unsafe must still fail, even
        // after observing zero-link metadata for its predecessor.
        let linked = fixture.0.join("another-link");
        fs::hard_link(&path, &linked).unwrap();
        calls = 0;
        assert_eq!(
            check_file_with(&path, || {
                calls += 1;
                if calls == 1 {
                    removed.metadata()
                } else {
                    fs::symlink_metadata(&path)
                }
            })
            .unwrap_err()
            .code,
            "unsafe-store-path"
        );
        fs::remove_file(linked).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(check_file(&path).unwrap_err().code, "unsafe-store-path");
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
    }
}
