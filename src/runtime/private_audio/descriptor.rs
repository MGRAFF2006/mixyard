//! Startup qualification only; source evidence never grants app/consent authority.
use super::protocol::{hex_digest, Descriptor};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

const MANIFEST_LIMIT: usize = 4 * 1024 * 1024;
const FILE_LIMIT: u64 = 256 * 1024 * 1024;
const PREFIX_LIMIT: u64 = 384 * 1024 * 1024;
const MODULES: [&str; 6] = [
    "protocol-native",
    "client-node",
    "access",
    "spa-node-factory",
    "link-factory",
    "metadata",
];
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    files: BTreeMap<String, Entry>,
    symlinks: BTreeMap<String, String>,
    total_bytes: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    sha256: String,
    bytes: u64,
    mode: u32,
    build_id: Option<String>,
}

pub struct Qualified {
    pub descriptor: Descriptor,
    pub descriptor_path: PathBuf,
    hashes: BTreeMap<PathBuf, String>,
}
fn safe_absolute(path: &Path) -> io::Result<()> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid("expected absolute path without parent components"));
    }
    Ok(())
}
fn owned(path: &Path, directory: bool) -> io::Result<fs::Metadata> {
    safe_absolute(path)?;
    for ancestor in path.ancestors().skip(1) {
        let metadata = fs::symlink_metadata(ancestor)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid("symlink/non-directory ancestor"));
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
        || metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
    {
        return Err(invalid(
            "expected owned non-writable-by-others real directory or single-link file",
        ));
    }
    Ok(metadata)
}
fn bounded(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid("file byte limit exceeded"));
    }
    Ok(bytes)
}
fn digest(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or_else(|| invalid("file size overflow"))?;
        if total > FILE_LIMIT {
            return Err(invalid("qualified file too large"));
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn relative(name: &str) -> io::Result<&Path> {
    let path = Path::new(name);
    if name.is_empty() || !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(invalid("invalid manifest relative path"));
    }
    Ok(path)
}
impl Qualified {
    pub fn load(path: &Path) -> io::Result<Self> {
        owned(path, false)?;
        let descriptor: Descriptor = serde_json::from_slice(&bounded(path, 8192)?)?;
        let d = &descriptor;
        if d.schema_version != 1
            || d.sdk_version != "1.4.8"
            || d.sdk_commit != "8b43ed5c910e892d53de5a96aa01e9280a5b6650"
            || !hex_digest(&d.prefix_manifest_sha256)
            || !hex_digest(&d.companion_sha256)
        {
            return Err(invalid("unqualified pinned experimental SDK descriptor"));
        }
        owned(&d.sdk_prefix, true)?;
        owned(&d.prefix_manifest, false)?;
        let executable = owned(&d.companion_executable, false)?;
        if executable.mode() & 0o111 == 0
            || digest(&d.companion_executable)? != d.companion_sha256
            || digest(&d.prefix_manifest)? != d.prefix_manifest_sha256
        {
            return Err(invalid("descriptor executable or manifest digest mismatch"));
        }
        if d.generators.is_empty()
            || d.generators.len() > d.limits.max_sources as usize
            || d.limits.max_sources == 0
            || d.limits.max_sources > 64
            || d.limits.max_outputs == 0
            || d.limits.max_outputs > 64
            || d.limits.max_frames != 128
            || d.limits.max_resident_bytes == 0
            || d.limits.max_resident_bytes > FILE_LIMIT
        {
            return Err(invalid(
                "private source/output/resident/frame budget invalid",
            ));
        }
        let mut sources = BTreeSet::new();
        for g in &d.generators {
            if g.source_id.is_empty()
                || g.source_id.len() > 128
                || !sources.insert(&g.source_id)
                || !g.left.is_finite()
                || !g.right.is_finite()
                || g.left.abs() > 1.
                || g.right.abs() > 1.
                || g.marker_period == 0
            {
                return Err(invalid("invalid generated source"));
            }
        }
        let manifest: Manifest =
            serde_json::from_slice(&bounded(&d.prefix_manifest, MANIFEST_LIMIT)?)?;
        if manifest.files.len() + manifest.symlinks.len() > 20000
            || manifest.total_bytes > PREFIX_LIMIT
        {
            return Err(invalid("prefix manifest budget exceeded"));
        }
        let mut hashes = BTreeMap::new();
        let mut total = 0u64;
        for (name, entry) in &manifest.files {
            let file = d.sdk_prefix.join(relative(name)?);
            let metadata = owned(&file, false)?;
            total = total
                .checked_add(metadata.len())
                .ok_or_else(|| invalid("prefix size overflow"))?;
            if !hex_digest(&entry.sha256)
                || metadata.len() != entry.bytes
                || metadata.mode() & 0o7777 != entry.mode
                || digest(&file)? != entry.sha256
                || total > PREFIX_LIMIT
            {
                return Err(invalid("SDK manifest file mismatch"));
            }
            if entry
                .build_id
                .as_ref()
                .is_some_and(|id| id.len() > 128 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(invalid("invalid manifest build identity"));
            }
            hashes.insert(file, entry.sha256.clone());
        }
        for (name, target) in &manifest.symlinks {
            let link = d.sdk_prefix.join(relative(name)?);
            let metadata = fs::symlink_metadata(&link)?;
            if !metadata.file_type().is_symlink()
                || metadata.uid() != unsafe { libc::geteuid() }
                || fs::read_link(&link)? != Path::new(target)
                || !fs::canonicalize(&link)?.starts_with(&d.sdk_prefix)
            {
                return Err(invalid("SDK symlink mismatch or escape"));
            }
        }
        if total != manifest.total_bytes {
            return Err(invalid("SDK totalBytes mismatch"));
        }
        let mut queue = vec![d.sdk_prefix.clone()];
        let mut count = 0usize;
        while let Some(dir) = queue.pop() {
            owned(&dir, true)?;
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                count += 1;
                if count > 40000 {
                    return Err(invalid("SDK tree cardinality exceeded"));
                }
                let file = entry.path();
                let metadata = fs::symlink_metadata(&file)?;
                if metadata.is_dir() {
                    queue.push(file);
                } else {
                    let name = file
                        .strip_prefix(&d.sdk_prefix)
                        .map_err(|_| invalid("prefix escape"))?
                        .to_str()
                        .ok_or_else(|| invalid("non UTF-8 SDK path"))?
                        .to_owned();
                    if !manifest.files.contains_key(&name) && !manifest.symlinks.contains_key(&name)
                    {
                        return Err(invalid("unmanifested SDK file"));
                    }
                }
            }
        }
        let mut required = vec![
            "bin/pipewire".to_string(),
            "lib/libpipewire-0.3.so.0".to_string(),
            "lib/spa-0.2/support/libspa-support.so".to_string(),
        ];
        required.extend(
            MODULES
                .iter()
                .map(|m| format!("lib/pipewire-0.3/libpipewire-module-{m}.so")),
        );
        for name in required {
            let file = fs::canonicalize(d.sdk_prefix.join(name))?;
            if !hashes.contains_key(&file) {
                return Err(invalid("required SDK path missing from verified manifest"));
            }
            if file == fs::canonicalize(d.sdk_prefix.join("bin/pipewire"))?
                && fs::metadata(&file)?.mode() & 0o111 == 0
            {
                return Err(invalid("private daemon is not executable"));
            }
        }
        Ok(Self {
            descriptor,
            descriptor_path: path.to_owned(),
            hashes,
        })
    }
    /// Point-only off-RT observation of one child we own; no system-wide claims.
    pub fn observe_bindings(&self, pid: u32, daemon: bool) -> io::Result<()> {
        let maps = String::from_utf8(bounded(
            &PathBuf::from(format!("/proc/{pid}/maps")),
            1024 * 1024,
        )?)
        .map_err(|_| invalid("invalid process mapping text"))?;
        let mut observed = BTreeSet::new();
        for line in maps.lines() {
            let mut fields = line.split_whitespace();
            let (
                Some(_address),
                Some(_permissions),
                Some(_offset),
                Some(device),
                Some(inode),
                Some(path),
            ) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            )
            else {
                continue;
            };
            let file = PathBuf::from(path);
            let name = file.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.contains("libpipewire")
                || name.contains("libspa-")
                || file.starts_with(&self.descriptor.sdk_prefix)
            {
                if fields.next().is_some() {
                    return Err(invalid("critical mapping has unknown/deleted identity"));
                }
                let canonical = fs::canonicalize(&file)?;
                let metadata = owned(&canonical, false)?;
                let (major, minor) = device
                    .split_once(':')
                    .ok_or_else(|| invalid("critical mapping device invalid"))?;
                let major = u64::from_str_radix(major, 16)
                    .map_err(|_| invalid("critical mapping device invalid"))?;
                let minor = u64::from_str_radix(minor, 16)
                    .map_err(|_| invalid("critical mapping device invalid"))?;
                let inode = inode
                    .parse::<u64>()
                    .map_err(|_| invalid("critical mapping inode invalid"))?;
                if metadata.ino() != inode
                    || libc::major(metadata.dev()) as u64 != major
                    || libc::minor(metadata.dev()) as u64 != minor
                {
                    return Err(invalid("critical mapping file identity changed"));
                }
                let expected = self
                    .hashes
                    .get(&canonical)
                    .ok_or_else(|| invalid("unknown critical loaded SDK binding"))?;
                if digest(&canonical)? != *expected {
                    return Err(invalid("loaded SDK file changed"));
                }
                observed.insert(canonical);
            }
        }
        let mut required = vec![
            "lib/libpipewire-0.3.so.0".to_string(),
            "lib/spa-0.2/support/libspa-support.so".to_string(),
        ];
        if daemon {
            required.extend(
                MODULES
                    .iter()
                    .map(|m| format!("lib/pipewire-0.3/libpipewire-module-{m}.so")),
            );
        }
        for name in required {
            if !observed.contains(&fs::canonicalize(self.descriptor.sdk_prefix.join(name))?) {
                return Err(invalid("required critical SDK binding not observed"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::{symlink, DirBuilderExt, OpenOptionsExt};
    struct Fixture {
        root: PathBuf,
        path: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let mut entropy = [0; 16];
            getrandom::fill(&mut entropy).unwrap();
            let root = PathBuf::from("/dev/shm").join(format!(
                "mixyard-descriptor-{:x}",
                u128::from_ne_bytes(entropy)
            ));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            let prefix = root.join("sdk");
            fs::DirBuilder::new().mode(0o700).create(&prefix).unwrap();
            let mut names = vec![
                "bin/pipewire".to_string(),
                "lib/libpipewire-0.3.so.0".to_string(),
                "lib/spa-0.2/support/libspa-support.so".to_string(),
            ];
            names.extend(
                MODULES
                    .iter()
                    .map(|name| format!("lib/pipewire-0.3/libpipewire-module-{name}.so")),
            );
            let mut files = serde_json::Map::new();
            let mut total = 0;
            for name in names {
                let file = prefix.join(&name);
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(file.parent().unwrap())
                    .unwrap();
                let mut output = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(if name == "bin/pipewire" { 0o700 } else { 0o600 })
                    .open(&file)
                    .unwrap();
                use std::io::Write;
                output
                    .write_all(b"descriptor hash fixture, not an SDK")
                    .unwrap();
                drop(output);
                let metadata = fs::metadata(&file).unwrap();
                total += metadata.len();
                files.insert(name,json!({"sha256":digest(&file).unwrap(),"bytes":metadata.len(),"mode":metadata.mode()&0o7777,"buildId":null}));
            }
            let manifest = root.join("manifest.json");
            put(
                &manifest,
                &serde_json::to_vec(&json!({"files":files,"symlinks":{},"totalBytes":total}))
                    .unwrap(),
                0o600,
            );
            let companion = root.join("companion");
            put(&companion, b"descriptor fixture, never executed", 0o700);
            let path = root.join("descriptor.json");
            put(&path,&serde_json::to_vec(&json!({"schemaVersion":1,"scope":"owned-private-generated-v1","sdkPrefix":prefix,"prefixManifest":manifest,"prefixManifestSha256":digest(&root.join("manifest.json")).unwrap(),"companionExecutable":companion,"companionSha256":digest(&root.join("companion")).unwrap(),"sdkVersion":"1.4.8","sdkCommit":"8b43ed5c910e892d53de5a96aa01e9280a5b6650","generators":[{"sourceId":"generated","left":0.25,"right":-0.25,"markerPeriod":128,"frameLimit":0}],"limits":{"maxSources":4,"maxOutputs":4,"maxResidentBytes":1048576,"maxFrames":128}})).unwrap(),0o600);
            Self { root, path }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
    fn put(path: &Path, bytes: &[u8], mode: u32) {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)
            .unwrap();
        file.write_all(bytes).unwrap();
    }
    #[test]
    fn full_manifest_qualification_allows_four_outputs_but_never_claims_bindings() {
        let fixture = Fixture::new();
        let qualified = Qualified::load(&fixture.path).unwrap();
        assert_eq!(qualified.descriptor.limits.max_outputs, 4);
        assert!(qualified
            .observe_bindings(std::process::id(), true)
            .is_err());
        fs::write(
            fixture.root.join("sdk/lib/libpipewire-0.3.so.0"),
            b"tampered",
        )
        .unwrap();
        assert!(Qualified::load(&fixture.path).is_err());
    }
    #[test]
    fn unmanifested_objects_and_descriptor_symlinks_fail_closed() {
        let fixture = Fixture::new();
        put(
            &fixture.root.join("sdk/unqualified-plugin"),
            b"extra",
            0o600,
        );
        assert!(Qualified::load(&fixture.path).is_err());
        fs::remove_file(fixture.root.join("sdk/unqualified-plugin")).unwrap();
        let link = fixture.root.join("descriptor-link.json");
        symlink(&fixture.path, &link).unwrap();
        assert!(Qualified::load(&link).is_err());
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&fixture.path).unwrap()).unwrap();
        value["systemFallback"] = json!(true);
        fs::write(&fixture.path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(Qualified::load(&fixture.path).is_err());
    }
}
