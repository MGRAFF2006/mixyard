use super::{Error, Result, require};
use std::ffi::{c_int, c_uint, c_void};
use std::ptr::NonNull;

// Stable libzstd >= 1.4 APIs. Single-pass decode uses caller-sized output and
// no internal window buffer; we additionally reject oversized frame windows.
unsafe extern "C" {
    fn ZSTD_versionNumber() -> c_uint;
    fn ZSTD_createCCtx() -> *mut c_void;
    fn ZSTD_freeCCtx(ctx: *mut c_void) -> usize;
    fn ZSTD_createDCtx() -> *mut c_void;
    fn ZSTD_freeDCtx(ctx: *mut c_void) -> usize;
    fn ZSTD_CCtx_setParameter(ctx: *mut c_void, param: c_int, value: c_int) -> usize;
    fn ZSTD_compress2(
        ctx: *mut c_void,
        out: *mut u8,
        capacity: usize,
        input: *const u8,
        len: usize,
    ) -> usize;
    fn ZSTD_decompressDCtx(
        ctx: *mut c_void,
        out: *mut u8,
        capacity: usize,
        input: *const u8,
        len: usize,
    ) -> usize;
    fn ZSTD_getFrameContentSize(input: *const u8, len: usize) -> u64;
    fn ZSTD_findFrameCompressedSize(input: *const u8, len: usize) -> usize;
    fn ZSTD_isError(code: usize) -> c_uint;
    fn ZSTD_sizeof_CCtx(ctx: *const c_void) -> usize;
    fn ZSTD_sizeof_DCtx(ctx: *const c_void) -> usize;
}
pub(crate) fn version() -> u32 {
    // SAFETY: no-argument version query from the linked runtime library.
    unsafe { ZSTD_versionNumber() }
}
fn checked(n: usize) -> Result<usize> {
    // SAFETY: this function only classifies a size_t return value.
    if unsafe { ZSTD_isError(n) } == 0 {
        Ok(n)
    } else {
        Err(Error::Library("zstd"))
    }
}
pub(crate) struct Engine {
    encoder: NonNull<c_void>,
    decoder: NonNull<c_void>,
}
// SAFETY: contexts have one owner, no thread-local affinity, and every operation
// mutating either context requires exclusive ownership of this engine.
unsafe impl Send for Engine {}
impl Engine {
    pub fn new(level: u8) -> Result<Self> {
        // SAFETY: constructors take no arguments; each pointer has one RAII owner.
        let encoder =
            NonNull::new(unsafe { ZSTD_createCCtx() }).ok_or(Error::Library("zstd allocation"))?;
        let Some(decoder) = NonNull::new(unsafe { ZSTD_createDCtx() }) else {
            unsafe {
                ZSTD_freeCCtx(encoder.as_ptr());
            }
            return Err(Error::Library("zstd allocation"));
        };
        let engine = Self { encoder, decoder };
        // Stable parameters: compression level, window log, content-size, checksum,
        // dictionary ID, workers. Bound input+level+window; no remote parameters.
        for (param, value) in [
            (100, i32::from(level)),
            (101, 18),
            (200, 1),
            (201, 1),
            (202, 0),
            (400, 0),
        ] {
            checked(unsafe { ZSTD_CCtx_setParameter(engine.encoder.as_ptr(), param, value) })?;
        }
        Ok(engine)
    }
    pub fn encode(&mut self, input: &[u8], capacity: usize) -> Result<Vec<u8>> {
        let mut out = vec![0; capacity];
        // SAFETY: exclusive live context, input slice, initialized capacity-sized output.
        let len = checked(unsafe {
            ZSTD_compress2(
                self.encoder.as_ptr(),
                out.as_mut_ptr(),
                out.len(),
                input.as_ptr(),
                input.len(),
            )
        })?;
        out.truncate(len);
        Ok(out)
    }
    pub fn decode(&mut self, input: &[u8], expected: usize) -> Result<Vec<u8>> {
        validate_frame(input, expected)?;
        let mut out = vec![0; expected];
        // SAFETY: header and exact output bound validated before allocation. Single
        // pass decode does not allocate a frame-advertised window or dictionary.
        let len = checked(unsafe {
            ZSTD_decompressDCtx(
                self.decoder.as_ptr(),
                out.as_mut_ptr(),
                out.len(),
                input.as_ptr(),
                input.len(),
            )
        })?;
        require(len == expected, "zstd output length")?;
        Ok(out)
    }
    pub fn context_bytes(&self) -> usize {
        // SAFETY: both owned contexts remain live for this shared observation.
        unsafe { ZSTD_sizeof_CCtx(self.encoder.as_ptr()) + ZSTD_sizeof_DCtx(self.decoder.as_ptr()) }
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: each context freed once, after exclusive worker use ends.
        unsafe {
            ZSTD_freeCCtx(self.encoder.as_ptr());
            ZSTD_freeDCtx(self.decoder.as_ptr());
        }
    }
}

fn validate_frame(input: &[u8], expected: usize) -> Result<()> {
    require(
        input.len() >= 6 && input[..4] == [0x28, 0xb5, 0x2f, 0xfd],
        "zstd magic/header",
    )?;
    let descriptor = input[4];
    require(
        descriptor & 0x1b == 0 && descriptor & 4 != 0,
        "zstd dictionary/reserved/checksum policy",
    )?;
    // Reject unknown size, malformed header, and expansion beyond admitted bytes.
    let content = unsafe { ZSTD_getFrameContentSize(input.as_ptr(), input.len()) };
    require(
        content == expected as u64,
        "zstd exact known content size required",
    )?;
    let window = if descriptor & 0x20 != 0 {
        content
    } else {
        let byte = input[5];
        let base = 1u64 << (10 + (byte >> 3));
        base + (base / 8) * u64::from(byte & 7)
    };
    require(
        window <= super::MAX_BLOCK_BYTES as u64,
        "zstd window budget",
    )?;
    let frame_size = checked(unsafe { ZSTD_findFrameCompressedSize(input.as_ptr(), input.len()) })?;
    require(
        frame_size == input.len(),
        "zstd concatenated/trailing frame rejected",
    )
}
