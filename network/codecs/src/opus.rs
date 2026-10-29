use super::{Error, Format, OpusParameters, Result, Sample, require};
use std::ffi::{CStr, c_char, c_int, c_void};

#[link(name = "opus")]
unsafe extern "C" {
    fn opus_get_version_string() -> *const c_char;
    fn opus_encoder_get_size(channels: c_int) -> c_int;
    fn opus_decoder_get_size(channels: c_int) -> c_int;
    fn opus_encoder_init(
        state: *mut c_void,
        rate: c_int,
        channels: c_int,
        application: c_int,
    ) -> c_int;
    fn opus_decoder_init(state: *mut c_void, rate: c_int, channels: c_int) -> c_int;
    fn opus_encoder_ctl(state: *mut c_void, request: c_int, ...) -> c_int;
    fn opus_encode_float(
        state: *mut c_void,
        input: *const f32,
        frames: c_int,
        output: *mut u8,
        capacity: c_int,
    ) -> c_int;
    fn opus_decode_float(
        state: *mut c_void,
        input: *const u8,
        len: c_int,
        output: *mut f32,
        frames: c_int,
        fec: c_int,
    ) -> c_int;
    fn opus_packet_get_nb_samples(input: *const u8, len: c_int, rate: c_int) -> c_int;
    fn opus_packet_get_nb_channels(input: *const u8) -> c_int;
}
pub(crate) fn version() -> Option<&'static str> {
    // SAFETY: libopus documents a static NUL-terminated version string.
    let pointer = unsafe { opus_get_version_string() };
    if pointer.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(pointer) }.to_str().ok()
    }
}
fn checked(n: c_int) -> Result<usize> {
    if n >= 0 {
        Ok(n as usize)
    } else {
        Err(Error::Library("opus"))
    }
}
pub(crate) struct Engine {
    encoder: Vec<u64>,
    decoder: Vec<u64>,
    lookahead: u32,
}
impl Engine {
    pub fn new(f: &Format, p: OpusParameters) -> Result<Self> {
        let channels = f.channels.len() as i32;
        // SAFETY: negotiated standard mono/stereo. The documented position-independent
        // states reside in exclusive, initialized u64 storage, with at least 8-byte alignment.
        let enc_bytes = checked(unsafe { opus_encoder_get_size(channels) })?;
        let dec_bytes = checked(unsafe { opus_decoder_get_size(channels) })?;
        require(
            enc_bytes > 0 && enc_bytes <= 1024 * 1024 && dec_bytes > 0 && dec_bytes <= 1024 * 1024,
            "Opus context budget",
        )?;
        let mut e = Self {
            encoder: vec![0; enc_bytes.div_ceil(8)],
            decoder: vec![0; dec_bytes.div_ceil(8)],
            lookahead: 0,
        };
        unsafe {
            checked(opus_encoder_init(
                e.encoder.as_mut_ptr().cast(),
                f.rate as i32,
                channels,
                2049,
            ))?; // AUDIO
            checked(opus_decoder_init(
                e.decoder.as_mut_ptr().cast(),
                f.rate as i32,
                channels,
            ))?;
            for (request, value) in [
                (4002, p.bitrate as i32),
                (4010, i32::from(p.complexity)),
                (4006, i32::from(p.vbr)),
                (4012, 0),
                (4016, 0),
            ] {
                checked(opus_encoder_ctl(
                    e.encoder.as_mut_ptr().cast(),
                    request,
                    value,
                ))?;
            }
            // Force negotiated mono/stereo instead of automatic stereo -> mono coding.
            checked(opus_encoder_ctl(
                e.encoder.as_mut_ptr().cast(),
                4022,
                channels,
            ))?;
            let mut actual_bitrate = 0i32;
            checked(opus_encoder_ctl(
                e.encoder.as_mut_ptr().cast(),
                4003,
                &mut actual_bitrate as *mut i32,
            ))?;
            require(
                actual_bitrate == p.bitrate as i32,
                "Opus library changed requested target bitrate",
            )?;
            let mut lookahead = 0i32;
            checked(opus_encoder_ctl(
                e.encoder.as_mut_ptr().cast(),
                4027,
                &mut lookahead as *mut i32,
            ))?;
            require(lookahead >= 0, "Opus lookahead")?;
            e.lookahead = lookahead as u32;
        }
        Ok(e)
    }
    pub fn encode(&mut self, f: &Format, input: &[u8]) -> Result<Vec<u8>> {
        let samples = to_f32(f.sample, input)?;
        let mut output = vec![0; super::OPUS_MAX_PACKET_BYTES];
        // SAFETY: admitted frame count/rate/channels, aligned samples, bounded packet output.
        let len = checked(unsafe {
            opus_encode_float(
                self.encoder.as_mut_ptr().cast(),
                samples.as_ptr(),
                f.frames as i32,
                output.as_mut_ptr(),
                output.len() as i32,
            )
        })?;
        require(len > 0, "empty Opus packet")?;
        output.truncate(len);
        Ok(output)
    }
    pub fn decode(&mut self, f: &Format, input: &[u8]) -> Result<Vec<u8>> {
        require(
            !input.is_empty() && input.len() <= super::OPUS_MAX_PACKET_BYTES,
            "Opus packet work budget",
        )?;
        // SAFETY: nonempty bounded packet. Inspect duration/layout before output allocation.
        let frames = checked(unsafe {
            opus_packet_get_nb_samples(input.as_ptr(), input.len() as i32, f.rate as i32)
        })?;
        require(frames == f.frames as usize, "Opus packet duration mismatch")?;
        let channels = checked(unsafe { opus_packet_get_nb_channels(input.as_ptr()) })?;
        require(channels == f.channels.len(), "Opus packet channel mismatch")?;
        let mut samples = vec![0f32; frames * channels];
        let decoded = checked(unsafe {
            opus_decode_float(
                self.decoder.as_mut_ptr().cast(),
                input.as_ptr(),
                input.len() as i32,
                samples.as_mut_ptr(),
                frames as i32,
                0,
            )
        })?;
        require(decoded == frames, "Opus decoded frame mismatch")?;
        from_f32(f.sample, &samples)
    }
    pub fn context_bytes(&self) -> usize {
        (self.encoder.len() + self.decoder.len()) * 8
    }
    pub fn lookahead_frames(&self) -> u32 {
        self.lookahead
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.encoder.fill(0);
        self.decoder.fill(0);
    }
}

pub(crate) fn to_f32(sample: Sample, input: &[u8]) -> Result<Vec<f32>> {
    let mut output = Vec::with_capacity(input.len() / sample.bytes());
    for bytes in input.chunks_exact(sample.bytes()) {
        let x = match sample {
            Sample::S16Le => f64::from(i16::from_le_bytes(bytes.try_into().unwrap())) / 32768.0,
            Sample::S24Le => {
                f64::from(i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8) / 8388608.0
            }
            Sample::S32Le => {
                f64::from(i32::from_le_bytes(bytes.try_into().unwrap())) / 2147483648.0
            }
            Sample::F32Le => f64::from(f32::from_le_bytes(bytes.try_into().unwrap())),
            Sample::F64Le => f64::from_le_bytes(bytes.try_into().unwrap()),
        };
        require(
            x.is_finite() && (-1.0..=1.0).contains(&x),
            "Opus conversion requires finite normalized input",
        )?;
        output.push(x as f32);
    }
    Ok(output)
}
pub(crate) fn from_f32(sample: Sample, input: &[f32]) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len() * sample.bytes());
    for &x in input {
        require(x.is_finite(), "nonfinite Opus decoder output")?;
        let x = f64::from(x).clamp(-1.0, 1.0);
        match sample {
            Sample::S16Le => {
                output.extend_from_slice(&((x * 32768.0).round() as i16).to_le_bytes())
            }
            Sample::S24Le => {
                let value = (x * 8388608.0).round().clamp(-8388608.0, 8388607.0) as i32;
                output.extend_from_slice(&value.to_le_bytes()[..3]);
            }
            Sample::S32Le => {
                output.extend_from_slice(&((x * 2147483648.0).round() as i32).to_le_bytes())
            }
            Sample::F32Le => output.extend_from_slice(&(x as f32).to_le_bytes()),
            Sample::F64Le => output.extend_from_slice(&x.to_le_bytes()),
        }
    }
    Ok(output)
}
