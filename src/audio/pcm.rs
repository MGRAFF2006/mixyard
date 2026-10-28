//! Checked packed signed PCM codecs using caller-owned storage. Integer PCM
//! widens exactly to normalized Float64 with no Float32 intermediate. These
//! helpers neither negotiate a native format nor perform DSP/quantization.

/// Packed two's-complement samples; S24 uses exactly three bytes, not a
/// 24-in-32 container. Byte order is explicit and independent of the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignedPcm {
    S16Le,
    S16Be,
    S24Le,
    S24Be,
    S32Le,
    S32Be,
}

impl SignedPcm {
    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::S16Le | Self::S16Be => 2,
            Self::S24Le | Self::S24Be => 3,
            Self::S32Le | Self::S32Be => 4,
        }
    }

    fn little_endian(self) -> bool {
        matches!(self, Self::S16Le | Self::S24Le | Self::S32Le)
    }

    fn scale(self) -> f64 {
        (1u64 << (self.bytes_per_sample() * 8 - 1)) as f64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmError {
    Length,
    NonFinite {
        sample: usize,
    },
    OutOfRange {
        sample: usize,
    },
    /// Encoding would discard fractional integer bits. Caller must explicitly
    /// choose and apply any desired quantization/dither before exact encoding.
    NotRepresentable {
        sample: usize,
    },
}

/// Decode N packed samples to N Float64 samples in [-1, 1). The positive
/// endpoint is 1 - 2^-(bits-1). Every S16/S24/S32 value is exactly representable.
/// Lengths must match exactly; all destination samples are zero on error.
/// No allocation, I/O or locks; sample/channel ordering is unchanged.
pub fn decode(format: SignedPcm, bytes: &[u8], samples: &mut [f64]) -> Result<(), PcmError> {
    samples.fill(0.0);
    let width = format.bytes_per_sample();
    if samples.len().checked_mul(width) != Some(bytes.len()) {
        return Err(PcmError::Length);
    }
    for (packed, dest) in bytes.chunks_exact(width).zip(samples) {
        let mut word = 0u32;
        for (index, byte) in packed.iter().enumerate() {
            let shift = if format.little_endian() {
                index
            } else {
                width - 1 - index
            };
            word |= (*byte as u32) << (shift * 8);
        }
        let shift = 32 - width * 8;
        let signed = ((word << shift) as i32) >> shift;
        *dest = signed as f64 / format.scale();
    }
    Ok(())
}

/// Encode only exactly representable normalized Float64 values. No rounding,
/// clipping, saturation or normalization is implicit; -0 encodes as integer 0.
/// Nonfinite, out-of-range and fractional-LSB values reject the entire block,
/// leaving all destination bytes zero. Empty matching slices are accepted.
/// No allocation, I/O or locks. Destination conversion is caller-selected.
pub fn encode_exact(format: SignedPcm, samples: &[f64], bytes: &mut [u8]) -> Result<(), PcmError> {
    bytes.fill(0);
    let width = format.bytes_per_sample();
    if samples.len().checked_mul(width) != Some(bytes.len()) {
        return Err(PcmError::Length);
    }
    let scale = format.scale();
    for (sample, value) in samples.iter().enumerate() {
        if !value.is_finite() {
            return Err(PcmError::NonFinite { sample });
        }
        let integer = value * scale;
        if integer < -scale || integer > scale - 1.0 {
            return Err(PcmError::OutOfRange { sample });
        }
        if integer.fract() != 0.0 {
            return Err(PcmError::NotRepresentable { sample });
        }
    }
    for (value, packed) in samples.iter().zip(bytes.chunks_exact_mut(width)) {
        let word = (value * scale) as i32 as u32;
        for (index, byte) in packed.iter_mut().enumerate() {
            let shift = if format.little_endian() {
                index
            } else {
                width - 1 - index
            };
            *byte = (word >> (shift * 8)) as u8;
        }
    }
    Ok(())
}
