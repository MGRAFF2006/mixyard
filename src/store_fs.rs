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
        check_file_metadata(&path, &meta)?;
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
pub(crate) fn check_file(path: &Path) -> Result<bool, Error> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(Error::io_at("inspect file", path, e)),
    };
    check_file_metadata(path, &meta)?;
    Ok(true)
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
