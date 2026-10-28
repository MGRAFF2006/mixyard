use super::Filter;

/// RBJ/W3C coefficients, normalized a0=1, band-pass with unity center gain.
/// Shelves use alpha=sin(w)/(2Q), i.e. Q rather than a shelf-slope parameter.
pub(super) fn coefficients(
    filter: Filter,
    frequency: f64,
    q: f64,
    gain_db: f64,
    rate: u32,
) -> Result<[f64; 5], &'static str> {
    if !frequency.is_finite() || frequency <= 0.0 || frequency >= rate as f64 / 2.0 {
        return Err("frequencyHz");
    }
    if !q.is_finite() || q <= 0.0 {
        return Err("q");
    }
    if !gain_db.is_finite() || gain_db.abs() > 6000.0 {
        return Err("gainDb");
    }
    if !matches!(
        filter,
        Filter::Peaking | Filter::LowShelf | Filter::HighShelf
    ) && gain_db != 0.0
    {
        return Err("gainDb");
    }
    let w = 2.0 * std::f64::consts::PI * (frequency / rate as f64);
    let c = w.cos();
    let alpha = w.sin() / (2.0 * q);
    let a = 10f64.powf(gain_db / 40.0);
    let beta = 2.0 * a.sqrt() * alpha;
    let (b0, b1, b2, a0, a1, a2) = match filter {
        Filter::LowPass => (
            (1.0 - c) / 2.0,
            1.0 - c,
            (1.0 - c) / 2.0,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        ),
        Filter::HighPass => (
            (1.0 + c) / 2.0,
            -(1.0 + c),
            (1.0 + c) / 2.0,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        ),
        Filter::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
        Filter::Notch => (1.0, -2.0 * c, 1.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
        Filter::AllPass => (
            1.0 - alpha,
            -2.0 * c,
            1.0 + alpha,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        ),
        Filter::Peaking => (
            1.0 + alpha * a,
            -2.0 * c,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * c,
            1.0 - alpha / a,
        ),
        Filter::LowShelf => (
            a * ((a + 1.0) - (a - 1.0) * c + beta),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - beta),
            (a + 1.0) + (a - 1.0) * c + beta,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - beta,
        ),
        Filter::HighShelf => (
            a * ((a + 1.0) + (a - 1.0) * c + beta),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - beta),
            (a + 1.0) - (a - 1.0) * c + beta,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - beta,
        ),
    };
    let result = [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0];
    if result.iter().any(|v| !v.is_finite()) || a0 == 0.0 {
        return Err("nonfiniteCoefficients");
    }
    // Strict real second-order Jury conditions: reject numerically marginal or
    // unstable coefficients instead of promising a filter at unrepresentable Q/f.
    if result[4].abs() >= 1.0
        || 1.0 + result[3] + result[4] <= 0.0
        || 1.0 - result[3] + result[4] <= 0.0
    {
        return Err("unstableCoefficients");
    }
    Ok(result)
}
