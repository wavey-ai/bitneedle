// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

//! Perceptual (OKLCH) lightening of an sRGB base tone.
//!
//! Canonical Bitneedle OKLab/OKLCH conversion for carrier authoring and rendering.
//!
//! This Rust implementation is the public reference for the sRGB <-> OKLab <->
//! OKLCH transforms used by Bitneedle toned carriers. See Björn Ottosson's
//! OKLab reference for the conversion matrices.

use anyhow::{Context, Result};

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn linear_srgb_to_oklab(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let l = 0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b;
    let m = 0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b;
    let s = 0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    (
        0.210_454_255_3 * l_ + 0.793_617_785_0 * m_ - 0.004_072_046_8 * s_,
        1.977_998_495_1 * l_ - 2.428_592_205_0 * m_ + 0.450_593_709_9 * s_,
        0.025_904_037_1 * l_ + 0.782_771_766_2 * m_ - 0.808_675_766_0 * s_,
    )
}

fn oklab_to_linear_srgb(l: f64, a: f64, b: f64) -> (f64, f64, f64) {
    let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s_ = l - 0.089_484_177_5 * a - 1.291_485_548_0 * b;

    let l3 = l_ * l_ * l_;
    let m3 = m_ * m_ * m_;
    let s3 = s_ * s_ * s_;

    (
        4.076_741_662_1 * l3 - 3.307_711_591_3 * m3 + 0.230_969_929_2 * s3,
        -1.268_438_004_6 * l3 + 2.609_757_401_1 * m3 - 0.341_319_396_5 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_614_7 * m3 + 1.707_614_701_0 * s3,
    )
}

/// sRGB byte triple -> OKLCH (lightness, chroma, hue in radians).
fn srgb_to_oklch(base: [u8; 3]) -> (f64, f64, f64) {
    let r = srgb_to_linear(f64::from(base[0]) / 255.0);
    let g = srgb_to_linear(f64::from(base[1]) / 255.0);
    let b = srgb_to_linear(f64::from(base[2]) / 255.0);

    let (l, a, ob) = linear_srgb_to_oklab(r, g, b);
    let chroma = a.hypot(ob);
    let hue = ob.atan2(a);
    (l, chroma, hue)
}

/// OKLCH -> sRGB byte triple, or `None` if the colour falls outside the sRGB
/// gamut (any channel outside `[0.0, 1.0]` in linear space before rounding).
fn oklch_to_srgb_in_gamut(lightness: f64, chroma: f64, hue: f64) -> Option<[u8; 3]> {
    let a = chroma * hue.cos();
    let b = chroma * hue.sin();
    let (r, g, bl) = oklab_to_linear_srgb(lightness, a, b);

    let in_unit = |c: f64| (-1e-6..=1.0 + 1e-6).contains(&c);
    if !in_unit(r) || !in_unit(g) || !in_unit(bl) {
        return None;
    }

    let to_byte = |c: f64| {
        (linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Some([to_byte(r), to_byte(g), to_byte(bl)])
}

/// Lighten an sRGB base tone perceptually, moving its OKLCH lightness a
/// fraction `amount` of the remaining distance to white while preserving
/// hue and (where possible) chroma:
///
/// `gap_l = base_l + (1.0 - base_l) * amount`
///
/// `amount` must be finite and within `[0.0, 1.0]`; `0.0` returns the base
/// tone unchanged (modulo OKLCH round-trip rounding) and `1.0` targets the
/// white limit before gamut mapping. If the target lightness/hue cannot be
/// reproduced in the sRGB gamut at the original chroma, chroma is reduced
/// deterministically (hue is never rotated) until a valid colour is found;
/// the achromatic (`chroma = 0`) point at any lightness in `[0, 1]` is
/// always in gamut, so this always succeeds.
pub fn lighten_base_oklch(base: [u8; 3], amount: f64) -> Result<[u8; 3]> {
    validate_gap_tone_lightness(amount)?;

    let (base_l, chroma, hue) = srgb_to_oklch(base);
    let gap_l = (base_l + (1.0 - base_l) * amount).clamp(0.0, 1.0);

    let mut candidate_chroma = chroma;
    for _ in 0..32 {
        if let Some(rgb) = oklch_to_srgb_in_gamut(gap_l, candidate_chroma, hue) {
            return Ok(rgb);
        }
        candidate_chroma *= 0.9;
    }

    oklch_to_srgb_in_gamut(gap_l, 0.0, hue)
        .context("could not gamut-map TrackGap tone even at zero chroma")
}

/// Validate a `gapToneLightness` amount: finite and within `[0.0, 1.0]`.
pub fn validate_gap_tone_lightness(amount: f64) -> Result<()> {
    if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
        anyhow::bail!("gapToneLightness must be finite and within [0.0, 1.0], got {amount}");
    }
    Ok(())
}

/// OKLCH lightness of an sRGB base tone, in `[0.0, 1.0]`.
pub fn oklch_lightness(base: [u8; 3]) -> f64 {
    srgb_to_oklch(base).0
}

/// Below this OKLCH base lightness, [`adaptive_gap_tone_lightness`] applies
/// `predominant_amount` unchanged: this is the lightness (and darker) where a
/// flat fractional amount already reads clearly as a lighter gap.
pub const ADAPTIVE_REFERENCE_BASE_LIGHTNESS: f64 = 0.5;

/// A flat `gap_l = base_l + (1.0 - base_l) * amount` shrinks in *absolute*
/// lightness as `base_l` approaches white — there's simply less "room to
/// white" left — so a fixed amount that reads as clearly lighter on a
/// mid/dark base tone can become perceptually invisible on a light one.
///
/// This keeps `predominant_amount` (the configured `gapToneLightness`, e.g.
/// the default `0.2`) for any base at or below
/// [`ADAPTIVE_REFERENCE_BASE_LIGHTNESS`], and scales the amount up for
/// lighter bases so the *absolute* lightness gap stays at least what it
/// would have been at the reference lightness — i.e. it goes lighter (not
/// darker) as the track tone itself gets lighter, capped at `1.0` (the
/// white limit).
pub fn adaptive_gap_tone_lightness(base_lightness: f64, predominant_amount: f64) -> Result<f64> {
    validate_gap_tone_lightness(predominant_amount)?;
    let base_lightness = base_lightness.clamp(0.0, 1.0);

    if base_lightness <= ADAPTIVE_REFERENCE_BASE_LIGHTNESS {
        return Ok(predominant_amount);
    }

    let target_delta = (1.0 - ADAPTIVE_REFERENCE_BASE_LIGHTNESS) * predominant_amount;
    let remaining_room = (1.0 - base_lightness).max(1e-6);
    Ok((target_delta / remaining_room).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_amount_matches_predominant_at_and_below_reference() {
        assert_eq!(
            adaptive_gap_tone_lightness(ADAPTIVE_REFERENCE_BASE_LIGHTNESS, 0.35).unwrap(),
            0.35,
        );
        assert_eq!(adaptive_gap_tone_lightness(0.1, 0.35).unwrap(), 0.35);
        assert_eq!(adaptive_gap_tone_lightness(0.0, 0.35).unwrap(), 0.35);
    }

    #[test]
    fn adaptive_amount_increases_for_lighter_bases() {
        let at_reference = adaptive_gap_tone_lightness(0.5, 0.35).unwrap();
        let lighter = adaptive_gap_tone_lightness(0.7, 0.35).unwrap();
        let lighter_still = adaptive_gap_tone_lightness(0.9, 0.35).unwrap();
        assert!(lighter > at_reference);
        assert!(lighter_still > lighter);
        assert!(lighter_still <= 1.0);
    }

    #[test]
    fn adaptive_amount_preserves_absolute_lightness_delta_above_reference() {
        let predominant = 0.35;
        let target_delta = (1.0 - ADAPTIVE_REFERENCE_BASE_LIGHTNESS) * predominant;
        for base_l in [0.55, 0.6, 0.7, 0.8] {
            let amount = adaptive_gap_tone_lightness(base_l, predominant).unwrap();
            let delta = (1.0 - base_l) * amount;
            assert!(
                (delta - target_delta).abs() < 1e-9,
                "base_l={base_l} delta={delta}"
            );
        }
    }

    #[test]
    fn adaptive_amount_caps_at_one_for_very_light_bases() {
        let amount = adaptive_gap_tone_lightness(0.999, 0.35).unwrap();
        assert!(amount <= 1.0);
    }

    #[test]
    fn adaptive_amount_rejects_invalid_predominant_amount() {
        assert!(adaptive_gap_tone_lightness(0.5, 1.5).is_err());
        assert!(adaptive_gap_tone_lightness(0.5, f64::NAN).is_err());
    }

    #[test]
    fn zero_amount_returns_base_tone_unchanged() {
        let base = [0x40, 0x80, 0xA0];
        let lightened = lighten_base_oklch(base, 0.0).unwrap();
        // Round-trip through OKLCH and back may move each channel by a
        // rounding step, but never more than one 8-bit step.
        for (a, b) in base.iter().zip(lightened.iter()) {
            assert!(
                (i32::from(*a) - i32::from(*b)).abs() <= 1,
                "{base:?} vs {lightened:?}"
            );
        }
    }

    #[test]
    fn half_amount_moves_lightness_partway_to_white() {
        let base = [0x40, 0x20, 0x20];
        let (base_l, _, _) = srgb_to_oklch(base);
        let lightened = lighten_base_oklch(base, 0.5).unwrap();
        let (lightened_l, _, _) = srgb_to_oklch(lightened);
        assert!(lightened_l > base_l);
        assert!(lightened_l < 1.0);
        // Should land close to the analytic target (allowing for any chroma
        // reduction needed to stay in gamut, which can't change L itself).
        let target_l = base_l + (1.0 - base_l) * 0.5;
        assert!(
            (lightened_l - target_l).abs() < 0.02,
            "{lightened_l} vs {target_l}"
        );
    }

    #[test]
    fn full_amount_reaches_white_limit() {
        let base = [0x40, 0x20, 0x20];
        let lightened = lighten_base_oklch(base, 1.0).unwrap();
        let (l, _, _) = srgb_to_oklch(lightened);
        assert!(l > 0.95, "expected near-white lightness, got {l}");
    }

    #[test]
    fn invalid_amounts_are_rejected() {
        assert!(lighten_base_oklch([0, 0, 0], f64::NAN).is_err());
        assert!(lighten_base_oklch([0, 0, 0], f64::INFINITY).is_err());
        assert!(lighten_base_oklch([0, 0, 0], -0.01).is_err());
        assert!(lighten_base_oklch([0, 0, 0], 1.01).is_err());
    }

    #[test]
    fn hue_is_stable() {
        let base = [0x30, 0x90, 0x40];
        let (_, _, base_hue) = srgb_to_oklch(base);
        let lightened = lighten_base_oklch(base, 0.5).unwrap();
        let (_, lightened_chroma, lightened_hue) = srgb_to_oklch(lightened);
        if lightened_chroma > 1e-4 {
            let diff = (lightened_hue - base_hue).abs();
            assert!(diff < 0.05 || (diff - std::f64::consts::TAU).abs() < 0.05);
        }
    }

    #[test]
    fn is_deterministic() {
        let base = [0x55, 0x11, 0x99];
        let a = lighten_base_oklch(base, 0.5).unwrap();
        let b = lighten_base_oklch(base, 0.5).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn chroma_reduction_is_only_applied_when_needed() {
        // A near-achromatic base never needs chroma reduction to gamut-map.
        let base = [0x80, 0x80, 0x80];
        let (_, base_chroma, _) = srgb_to_oklch(base);
        let lightened = lighten_base_oklch(base, 0.5).unwrap();
        let (_, lightened_chroma, _) = srgb_to_oklch(lightened);
        assert!(lightened_chroma <= base_chroma + 1e-3);
    }
}
