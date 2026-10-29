//! Consented owned-file PCM persistence. Every API here is off the audio callback.
//! No capture, resampling, background task, automatic recovery or runtime activation.
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAGIC: &[u8; 8] = b"MYREC001";
const HEADER: usize = 16;
const RECORD: usize = 20;
const MAX_METADATA: usize = 16 * 1024;
pub const MAX_BLOCK_BYTES: usize = 1024 * 1024;
static NEXT_AUTHORITY: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Encoding {
    S16Le = 1,
    S16Be,
    S24Le,
    S24Be,
    S32Le,
    S32Be,
    F32Le,
    F32Be,
    F64Le,
    F64Be,
}
impl Encoding {
    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::S16Le | Self::S16Be => 2,
            Self::S24Le | Self::S24Be => 3,
            Self::S32Le | Self::S32Be | Self::F32Le | Self::F32Be => 4,
            Self::F64Le | Self::F64Be => 8,
        }
    }
    fn decode(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::S16Le),
            2 => Ok(Self::S16Be),
            3 => Ok(Self::S24Le),
            4 => Ok(Self::S24Be),
            5 => Ok(Self::S32Le),
            6 => Ok(Self::S32Be),
            7 => Ok(Self::F32Le),
            8 => Ok(Self::F32Be),
            9 => Ok(Self::F64Le),
            10 => Ok(Self::F64Be),
            _ => Err(Error::Invalid("unknown PCM encoding")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Format {
    pub sample_rate_hz: u32,
    pub channels: Vec<String>,
    pub encoding: Encoding,
}
impl Format {
    fn validate(&self) -> Result<()> {
        if self.sample_rate_hz == 0 || self.channels.is_empty() || self.channels.len() > 64 {
            return Err(Error::Invalid(
                "rate must be positive; require 1..64 channels",
            ));
        }
        for (index, channel) in self.channels.iter().enumerate() {
            if !valid_label(channel)
                || self.channels[..index]
                    .iter()
                    .any(|previous| previous == channel)
            {
                return Err(Error::Invalid(
                    "channel labels must be unique, 1..128 bytes",
                ));
            }
        }
        Ok(())
    }
    fn frame_bytes(&self) -> usize {
        self.channels.len() * self.encoding.bytes_per_sample()
    }
}

/// Host-observed immutable source/timeline binding. Rebind/revoke before dispatch
/// when any value changes. Generation and clock are identities, not permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub source_id: String,
    pub revision: u64,
    pub generation: u64,
    pub clock_id: u64,
}
impl Context {
    fn validate(&self) -> Result<()> {
        if !valid_label(&self.source_id)
            || self.revision == 0
            || self.generation == 0
            || self.clock_id == 0
        {
            return Err(Error::Invalid(
                "require exact source and nonzero revision/generation/clock",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub context: Context,
    pub format: Format,
}

/// Explicit finite resource admission. No queue is allocated or owned here.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_block_bytes: usize,
    pub max_file_bytes: u64,
    pub max_blocks: u32,
}
impl Limits {
    fn validate(self) -> Result<()> {
        if self.max_block_bytes == 0
            || self.max_block_bytes > MAX_BLOCK_BYTES
            || self.max_file_bytes < (HEADER + RECORD) as u64
            || self.max_blocks == 0
        {
            return Err(Error::Invalid(
                "invalid file/block limits (block ceiling 1 MiB)",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    Consent,
    Closed,
    Limit,
    Gap,
    Io(io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "invalid recording: {reason}"),
            Self::Consent => f.write_str("recording consent revoked, stale or mismatched"),
            Self::Closed => f.write_str("recording closed or failed; explicit recovery required"),
            Self::Limit => f.write_str("recording resource limit reached"),
            Self::Gap => f.write_str("recording frame gap or replay"),
            Self::Io(error) => write!(f, "recording I/O: {error}"),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// Local control-owner capability. It cannot be serialized or cloned. Revoke and
/// append use exclusive/shared Rust borrows: serialize them on the owning worker.
pub struct Authority {
    id: u64,
    epoch: u64,
    active: bool,
    context: Context,
}
pub struct Consent {
    authority: u64,
    epoch: u64,
    destination: PathBuf,
    metadata: Metadata,
    limits: Limits,
}
impl Authority {
    pub fn new(context: Context) -> Result<Self> {
        context.validate()?;
        // Retain compatibility with Rust versions before the try_update rename.
        #[allow(deprecated)]
        let id = NEXT_AUTHORITY
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Error::Consent)?;
        Ok(Self {
            id,
            epoch: 0,
            active: false,
            context,
        })
    }
    /// Explicit user-approved scope; each grant invalidates any preceding grant.
    /// Caller owns UI/authentication and verifies actual source provenance.
    pub fn grant(
        &mut self,
        destination: PathBuf,
        format: Format,
        limits: Limits,
    ) -> Result<Consent> {
        self.revoke();
        validate_destination(&destination)?;
        format.validate()?;
        limits.validate()?;
        self.epoch = self.epoch.checked_add(1).ok_or(Error::Consent)?;
        self.active = true;
        Ok(Consent {
            authority: self.id,
            epoch: self.epoch,
            destination,
            metadata: Metadata {
                context: self.context.clone(),
                format,
            },
            limits,
        })
    }
    pub fn revoke(&mut self) {
        self.active = false;
    }
    /// Rebinding never re-arms. A new explicit grant is required afterwards.
    pub fn rebind(&mut self, context: Context) -> Result<()> {
        self.revoke();
        context.validate()?;
        self.context = context;
        Ok(())
    }
    fn permits(&self, consent: &Consent) -> bool {
        self.active
            && self.id == consent.authority
            && self.epoch == consent.epoch
            && self.context == consent.metadata.context
    }
}

/// New exclusive file only. append/finish may block on disk and MUST run off
/// callback. Drop closes the descriptor without finalizing or arming anything.
pub struct Recorder {
    file: File,
    consent: Consent,
    frames: u64,
    blocks: u32,
    bytes: u64,
    failed: bool,
}
impl Recorder {
    pub fn create(authority: &Authority, consent: Consent) -> Result<Self> {
        if !authority.permits(&consent) {
            return Err(Error::Consent);
        }
        let header = encode_metadata(&consent.metadata)?;
        if header.len() as u64 + RECORD as u64 > consent.limits.max_file_bytes {
            return Err(Error::Limit);
        }
        let mut file = create_owned(&consent.destination)?;
        file.write_all(&header)?;
        file.sync_all()?;
        Ok(Self {
            file,
            consent,
            frames: 0,
            blocks: 0,
            bytes: header.len() as u64,
            failed: false,
        })
    }
    /// Blocks start at frame zero and must remain contiguous. Rejecting a block
    /// is terminal, so a caller cannot hide a recording hole with a later write.
    /// Raw bytes are persisted verbatim, including IEEE754 payload/sign bits.
    pub fn append(
        &mut self,
        authority: &Authority,
        context: &Context,
        format: &Format,
        first_frame: u64,
        payload: &[u8],
    ) -> Result<()> {
        if self.failed {
            return Err(Error::Closed);
        }
        self.failed = true;
        if !authority.permits(&self.consent)
            || *context != self.consent.metadata.context
            || *format != self.consent.metadata.format
        {
            return Err(Error::Consent);
        }
        if first_frame != self.frames {
            return Err(Error::Gap);
        }
        let limits = self.consent.limits;
        if payload.is_empty() || payload.len() % format.frame_bytes() != 0 {
            return Err(Error::Invalid(
                "require complete nonempty interleaved frames",
            ));
        }
        if payload.len() > limits.max_block_bytes || self.blocks == limits.max_blocks {
            return Err(Error::Limit);
        }
        let bytes = self
            .bytes
            .checked_add(RECORD as u64 + payload.len() as u64)
            .filter(|value| {
                value
                    .checked_add(RECORD as u64)
                    .is_some_and(|total| total <= limits.max_file_bytes)
            })
            .ok_or(Error::Limit)?;
        let frames = self
            .frames
            .checked_add((payload.len() / format.frame_bytes()) as u64)
            .ok_or(Error::Limit)?;
        self.file.write_all(&record_header(
            b"DATA",
            first_frame,
            payload.len(),
            crc32(payload),
        ))?;
        self.file.write_all(payload)?;
        self.file.sync_data()?;
        self.bytes = bytes;
        self.frames = frames;
        self.blocks += 1;
        self.failed = false;
        Ok(())
    }
    /// Closing may finalize already persisted bytes after consent revocation; it
    /// cannot append PCM. Failed writers require explicit archive recovery.
    pub fn finish(mut self) -> Result<Report> {
        if self.failed {
            return Err(Error::Closed);
        }
        self.file
            .write_all(&record_header(b"DONE", self.frames, 0, 0))?;
        self.file.sync_all()?;
        Ok(Report {
            metadata: self.consent.metadata,
            frames: self.frames,
            blocks: self.blocks,
            recoverable_bytes: self.bytes,
            ending: Ending::Complete,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Complete,
    Interrupted,
    Corrupt,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub metadata: Metadata,
    pub frames: u64,
    pub blocks: u32,
    /// Header plus validated DATA records; excludes the final DONE marker.
    pub recoverable_bytes: u64,
    pub ending: Ending,
}

/// Read-only bounded inspection, starting at offset zero. File must be exclusively
/// controlled by the caller while scanning. No recovery or capture is automatic.
pub fn inspect(source: &mut File, limits: Limits) -> Result<Report> {
    scan(source, limits, None)
}

/// Explicit salvage to a NEW file. Returns archive information only; never
/// returns a Recorder/Consent, reopens a source, or changes an audio graph.
pub fn recover(source: &mut File, destination: &Path, limits: Limits) -> Result<Report> {
    validate_destination(destination)?;
    // Validate metadata and admission before creating any destination.
    read_metadata(source, limits)?;
    let mut output = create_owned(destination)?;
    let report = scan(source, limits, Some(&mut output))?;
    output.write_all(&record_header(b"DONE", report.frames, 0, 0))?;
    output.sync_all()?;
    Ok(report)
}

fn scan(source: &mut File, limits: Limits, mut output: Option<&mut File>) -> Result<Report> {
    let (metadata, header, file_len) = read_metadata(source, limits)?;
    if let Some(dest) = output.as_mut() {
        dest.write_all(&header)?;
    }
    let mut report = Report {
        metadata,
        frames: 0,
        blocks: 0,
        recoverable_bytes: header.len() as u64,
        ending: Ending::Interrupted,
    };
    let mut payload = vec![0; limits.max_block_bytes];
    let mut consumed = header.len() as u64;
    loop {
        let remaining = file_len - consumed;
        if remaining < RECORD as u64 {
            break;
        }
        let mut raw = [0; RECORD];
        source.read_exact(&mut raw)?;
        consumed += RECORD as u64;
        let first = u64::from_le_bytes(raw[4..12].try_into().unwrap());
        let len = u32::from_le_bytes(raw[12..16].try_into().unwrap()) as usize;
        let crc = u32::from_le_bytes(raw[16..20].try_into().unwrap());
        if &raw[..4] == b"DONE" && first == report.frames && len == 0 && crc == 0 {
            report.ending = if consumed == file_len {
                Ending::Complete
            } else {
                Ending::Corrupt
            };
            break;
        }
        if &raw[..4] != b"DATA"
            || first != report.frames
            || len == 0
            || len % report.metadata.format.frame_bytes() != 0
        {
            report.ending = Ending::Corrupt;
            break;
        }
        if len > limits.max_block_bytes || report.blocks == limits.max_blocks {
            return Err(Error::Limit);
        }
        if len as u64 > file_len - consumed {
            break;
        }
        source.read_exact(&mut payload[..len])?;
        consumed += len as u64;
        if crc32(&payload[..len]) != crc {
            report.ending = Ending::Corrupt;
            break;
        }
        // Reserve room for a new DONE even if the source has no valid footer.
        if consumed
            .checked_add(RECORD as u64)
            .is_none_or(|n| n > limits.max_file_bytes)
        {
            return Err(Error::Limit);
        }
        let frames = report
            .frames
            .checked_add((len / report.metadata.format.frame_bytes()) as u64)
            .ok_or(Error::Limit)?;
        if let Some(dest) = output.as_mut() {
            dest.write_all(&raw)?;
            dest.write_all(&payload[..len])?;
        }
        report.frames = frames;
        report.blocks += 1;
        report.recoverable_bytes = consumed;
    }
    Ok(report)
}

fn valid_label(label: &str) -> bool {
    !label.is_empty() && label.len() <= 128 && !label.chars().any(char::is_control)
}
fn validate_destination(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(Error::Invalid(
            "destination must be an absolute normalized file path",
        ));
    }
    Ok(())
}
fn create_owned(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::Invalid("require a regular file"));
    }
    Ok(file)
}
fn record_header(tag: &[u8; 4], first: u64, len: usize, crc: u32) -> [u8; RECORD] {
    let mut raw = [0; RECORD];
    raw[..4].copy_from_slice(tag);
    raw[4..12].copy_from_slice(&first.to_le_bytes());
    raw[12..16].copy_from_slice(&(len as u32).to_le_bytes());
    raw[16..20].copy_from_slice(&crc.to_le_bytes());
    raw
}
fn encode_metadata(metadata: &Metadata) -> Result<Vec<u8>> {
    metadata.context.validate()?;
    metadata.format.validate()?;
    let mut body = Vec::new();
    body.extend_from_slice(&metadata.format.sample_rate_hz.to_le_bytes());
    body.push(metadata.format.encoding as u8);
    body.push(metadata.format.channels.len() as u8);
    for channel in &metadata.format.channels {
        put_label(&mut body, channel);
    }
    put_label(&mut body, &metadata.context.source_id);
    for value in [
        metadata.context.revision,
        metadata.context.generation,
        metadata.context.clock_id,
    ] {
        body.extend_from_slice(&value.to_le_bytes());
    }
    let mut header = Vec::with_capacity(HEADER + body.len());
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&(body.len() as u32).to_le_bytes());
    header.extend_from_slice(&crc32(&body).to_le_bytes());
    header.extend_from_slice(&body);
    Ok(header)
}
fn put_label(bytes: &mut Vec<u8>, label: &str) {
    bytes.push(label.len() as u8);
    bytes.extend_from_slice(label.as_bytes());
}
fn read_metadata(source: &mut File, limits: Limits) -> Result<(Metadata, Vec<u8>, u64)> {
    limits.validate()?;
    let info = source.metadata()?;
    if !info.is_file() {
        return Err(Error::Invalid("require a regular source file"));
    }
    if info.len() > limits.max_file_bytes {
        return Err(Error::Limit);
    }
    source.seek(SeekFrom::Start(0))?;
    let mut fixed = [0; HEADER];
    source.read_exact(&mut fixed)?;
    if &fixed[..8] != MAGIC {
        return Err(Error::Invalid("unknown recording container"));
    }
    let len = u32::from_le_bytes(fixed[8..12].try_into().unwrap()) as usize;
    if len > MAX_METADATA
        || (HEADER + len + RECORD) as u64 > limits.max_file_bytes
        || (HEADER + len) as u64 > info.len()
    {
        return Err(Error::Invalid("invalid metadata length"));
    }
    let mut body = vec![0; len];
    source.read_exact(&mut body)?;
    if crc32(&body) != u32::from_le_bytes(fixed[12..16].try_into().unwrap()) {
        return Err(Error::Invalid("metadata checksum mismatch"));
    }
    let mut cursor = body.as_slice();
    let sample_rate_hz = u32::from_le_bytes(take(&mut cursor, 4)?.try_into().unwrap());
    let encoding = Encoding::decode(take(&mut cursor, 1)?[0])?;
    let count = take(&mut cursor, 1)?[0] as usize;
    if !(1..=64).contains(&count) {
        return Err(Error::Invalid("invalid channel count"));
    }
    let mut channels = Vec::with_capacity(count);
    for _ in 0..count {
        channels.push(label(&mut cursor)?);
    }
    let source_id = label(&mut cursor)?;
    let revision = u64::from_le_bytes(take(&mut cursor, 8)?.try_into().unwrap());
    let generation = u64::from_le_bytes(take(&mut cursor, 8)?.try_into().unwrap());
    let clock_id = u64::from_le_bytes(take(&mut cursor, 8)?.try_into().unwrap());
    if !cursor.is_empty() {
        return Err(Error::Invalid("unexpected metadata fields"));
    }
    let metadata = Metadata {
        context: Context {
            source_id,
            revision,
            generation,
            clock_id,
        },
        format: Format {
            sample_rate_hz,
            channels,
            encoding,
        },
    };
    metadata.context.validate()?;
    metadata.format.validate()?;
    let mut header = fixed.to_vec();
    header.extend_from_slice(&body);
    Ok((metadata, header, info.len()))
}
fn take<'a>(cursor: &mut &'a [u8], count: usize) -> Result<&'a [u8]> {
    if cursor.len() < count {
        return Err(Error::Invalid("truncated metadata"));
    }
    let (head, tail) = cursor.split_at(count);
    *cursor = tail;
    Ok(head)
}
fn label(cursor: &mut &[u8]) -> Result<String> {
    let count = take(cursor, 1)?[0] as usize;
    let bytes = take(cursor, count)?;
    let value = std::str::from_utf8(bytes).map_err(|_| Error::Invalid("non-UTF8 label"))?;
    if !valid_label(value) {
        return Err(Error::Invalid("invalid label"));
    }
    Ok(value.to_owned())
}
/// CRC-32/ISO-HDLC detects accidental corruption; it is not authentication.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
