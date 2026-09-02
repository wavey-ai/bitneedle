// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

#![doc = include_str!("../README.md")]

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LabelCutoutMode {
    Spindle,
    Dink,
    Dink45,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelProfileGeometry {
    pub record_profile: &'static str,
    pub label_radius: i32,
    pub spindle_hole_radius: i32,
    pub dink_radius: Option<i32>,
    pub default_cutout_mode: LabelCutoutMode,
    pub default_cutout_radius: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelCutoutGeometry {
    pub record_profile: &'static str,
    pub mode: LabelCutoutMode,
    pub radius: i32,
    pub transparent: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelCutoutStyleRing {
    pub radius: f64,
    pub width: f64,
    pub alpha: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelCutoutStyleInput {
    pub radius: f64,
    pub clear_radius: f64,
    pub cutout: bool,
    #[serde(default)]
    pub reposition_text: bool,
    pub shadow_alpha: f64,
    pub rings: Vec<LabelCutoutStyleRing>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedLabelCutoutStyle {
    pub radius: f64,
    pub clear_radius: f64,
    pub cutout: bool,
    pub transparent_cutout_mode: LabelCutoutMode,
    pub transparent_cutout_radius: f64,
    pub shadow_alpha: f64,
    pub rings: Vec<LabelCutoutStyleRing>,
}

pub fn label_profile_geometry(record_profile: &str) -> Result<LabelProfileGeometry> {
    let geometry = record_core::describe_record_profile(record_profile)?;
    // `describe_record_profile` has already accepted the name, so this only
    // needs a `'static` copy of it. Matching each profile by hand meant a new
    // carrier broke every label — and broke `known_label_profile_geometries`
    // wholesale, since that maps over the same list this refused.
    let record_profile = record_core::known_record_profile_names()
        .iter()
        .copied()
        .find(|name| *name == geometry.record_profile)
        .ok_or_else(|| anyhow!("unknown record profile: {}", geometry.record_profile))?;
    Ok(LabelProfileGeometry {
        record_profile,
        label_radius: geometry.label_radius,
        spindle_hole_radius: geometry.spindle_hole_radius,
        dink_radius: geometry.dink_radius,
        default_cutout_mode: LabelCutoutMode::Spindle,
        default_cutout_radius: geometry.spindle_hole_radius,
    })
}

pub fn known_label_profile_geometries() -> Result<Vec<LabelProfileGeometry>> {
    record_core::known_record_profile_names()
        .iter()
        .map(|profile| label_profile_geometry(profile))
        .collect()
}

pub fn label_cutout_geometry(
    record_profile: &str,
    mode: LabelCutoutMode,
    transparent: bool,
) -> Result<LabelCutoutGeometry> {
    let profile = label_profile_geometry(record_profile)?;
    let radius = match mode {
        LabelCutoutMode::Spindle => profile.spindle_hole_radius,
        LabelCutoutMode::Dink | LabelCutoutMode::Dink45 => {
            profile.dink_radius.ok_or_else(|| {
                anyhow::anyhow!(
                    "record profile {} has no dink cutout",
                    profile.record_profile
                )
            })?
        }
    };

    Ok(LabelCutoutGeometry {
        record_profile: profile.record_profile,
        mode,
        radius,
        transparent,
    })
}

fn require_positive(value: f64, field: &str) -> Result<f64> {
    if !value.is_finite() || value <= 0.0 {
        bail!("label cutout style field {field} must be a positive finite number");
    }
    Ok(value)
}

fn require_unit_interval(value: f64, field: &str) -> Result<f64> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        bail!("label cutout style field {field} must be between 0 and 1");
    }
    Ok(value)
}

fn validated_cutout_style(style: LabelCutoutStyleInput) -> Result<LabelCutoutStyleInput> {
    let radius = require_positive(style.radius, "radius")?;
    let clear_radius = require_positive(style.clear_radius, "clearRadius")?;
    let shadow_alpha = require_unit_interval(style.shadow_alpha, "shadowAlpha")?;
    let rings = style
        .rings
        .into_iter()
        .enumerate()
        .map(|(index, ring)| {
            Ok(LabelCutoutStyleRing {
                radius: require_positive(ring.radius, &format!("rings[{index}].radius"))?,
                width: require_positive(ring.width, &format!("rings[{index}].width"))?,
                alpha: require_unit_interval(ring.alpha, &format!("rings[{index}].alpha"))?,
                color: ring.color,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if rings.is_empty() && shadow_alpha <= 0.0 {
        bail!("label cutout style must include rings or shadowAlpha");
    }

    Ok(LabelCutoutStyleInput {
        radius,
        clear_radius,
        cutout: style.cutout,
        reposition_text: style.reposition_text,
        shadow_alpha,
        rings,
    })
}

pub fn resolve_label_cutout_style(
    record_profile: &str,
    style: LabelCutoutStyleInput,
) -> Result<Option<ResolvedLabelCutoutStyle>> {
    let profile = label_profile_geometry(record_profile)?;
    if profile.record_profile != "single45" {
        return Ok(None);
    }

    let style = validated_cutout_style(style)?;
    let dink_radius = profile.dink_radius.ok_or_else(|| {
        anyhow::anyhow!(
            "record profile {} has no dink cutout",
            profile.record_profile
        )
    })?;
    let scale = f64::from(dink_radius) / style.radius;
    let rings = style
        .rings
        .into_iter()
        .map(|ring| LabelCutoutStyleRing {
            radius: ring.radius * scale,
            width: ring.width * scale,
            alpha: ring.alpha,
            color: ring.color,
        })
        .collect();

    Ok(Some(ResolvedLabelCutoutStyle {
        radius: f64::from(dink_radius),
        clear_radius: style.clear_radius * scale,
        cutout: style.cutout,
        transparent_cutout_mode: if style.cutout {
            if style.reposition_text {
                LabelCutoutMode::Dink45
            } else {
                LabelCutoutMode::Dink
            }
        } else {
            LabelCutoutMode::Spindle
        },
        transparent_cutout_radius: if style.cutout {
            f64::from(dink_radius)
        } else {
            f64::from(profile.spindle_hole_radius)
        },
        shadow_alpha: style.shadow_alpha,
        rings,
    }))
}

pub fn known_label_profile_geometries_json() -> Result<String> {
    serde_json::to_string(&known_label_profile_geometries()?).map_err(Into::into)
}

pub fn label_profile_geometry_json(record_profile: &str) -> Result<String> {
    serde_json::to_string(&label_profile_geometry(record_profile)?).map_err(Into::into)
}

pub fn resolve_label_cutout_style_json(record_profile: &str, style_json: &str) -> Result<String> {
    let trimmed = style_json.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok("null".to_string());
    }
    let style = serde_json::from_str::<LabelCutoutStyleInput>(trimmed)?;
    serde_json::to_string(&resolve_label_cutout_style(record_profile, style)?).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single45_has_small_spindle_and_large_dink() {
        let profile = label_profile_geometry("single45").unwrap();

        assert_eq!(profile.label_radius, 151);
        assert_eq!(profile.spindle_hole_radius, 12);
        assert_eq!(profile.dink_radius, Some(63));
        assert_eq!(profile.default_cutout_radius, profile.spindle_hole_radius);
    }

    #[test]
    fn lp_has_standard_spindle_and_no_dink() {
        let profile = label_profile_geometry("lp").unwrap();

        assert_eq!(profile.label_radius, 95);
        assert_eq!(profile.spindle_hole_radius, 7);
        assert_eq!(profile.dink_radius, None);
    }

    #[test]
    fn dink_cutout_is_only_available_for_single45() {
        let cutout = label_cutout_geometry("single45", LabelCutoutMode::Dink, true).unwrap();
        assert_eq!(cutout.radius, 63);
        assert!(cutout.transparent);

        assert!(label_cutout_geometry("lp", LabelCutoutMode::Dink, true).is_err());
    }

    #[test]
    fn resolves_single45_cutout_style_against_dink_geometry() {
        let style = LabelCutoutStyleInput {
            radius: 63.0,
            clear_radius: 70.0,
            cutout: true,
            reposition_text: false,
            shadow_alpha: 0.1,
            rings: vec![LabelCutoutStyleRing {
                radius: 63.0,
                width: 1.2,
                alpha: 0.22,
                color: None,
            }],
        };

        let resolved = resolve_label_cutout_style("single45", style)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.radius, 63.0);
        assert_eq!(resolved.transparent_cutout_mode, LabelCutoutMode::Dink);
        assert_eq!(resolved.transparent_cutout_radius, 63.0);
        assert_eq!(resolved.rings[0].radius, 63.0);
    }

    #[test]
    fn resolves_single45_cutout_style_with_reposition_as_dink45() {
        let style = LabelCutoutStyleInput {
            radius: 63.0,
            clear_radius: 70.0,
            cutout: true,
            reposition_text: true,
            shadow_alpha: 0.1,
            rings: vec![LabelCutoutStyleRing {
                radius: 63.0,
                width: 1.2,
                alpha: 0.22,
                color: None,
            }],
        };

        let resolved = resolve_label_cutout_style("single45", style)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.transparent_cutout_mode, LabelCutoutMode::Dink45);
        assert_eq!(resolved.transparent_cutout_radius, 63.0);
    }

    #[test]
    fn visual_single45_cutout_style_keeps_small_spindle_transparency() {
        let style = LabelCutoutStyleInput {
            radius: 63.0,
            clear_radius: 70.0,
            cutout: false,
            reposition_text: false,
            shadow_alpha: 0.1,
            rings: vec![LabelCutoutStyleRing {
                radius: 63.0,
                width: 1.2,
                alpha: 0.22,
                color: None,
            }],
        };

        let resolved = resolve_label_cutout_style("single45", style)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.transparent_cutout_mode, LabelCutoutMode::Spindle);
        assert_eq!(resolved.transparent_cutout_radius, 12.0);
    }

    #[test]
    fn lp_ignores_45_cutout_style() {
        let style = LabelCutoutStyleInput {
            radius: 63.0,
            clear_radius: 70.0,
            cutout: true,
            reposition_text: false,
            shadow_alpha: 0.1,
            rings: vec![LabelCutoutStyleRing {
                radius: 63.0,
                width: 1.2,
                alpha: 0.22,
                color: None,
            }],
        };

        assert!(resolve_label_cutout_style("lp", style).unwrap().is_none());
    }

    #[test]
    fn rejects_incomplete_cutout_style() {
        let err = resolve_label_cutout_style_json(
            "single45",
            r#"{"radius":63,"clearRadius":70,"shadowAlpha":0.1,"rings":[]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("missing field `cutout`"));
    }
}
