//! ADR-0032 — referenced coordinates: **one** descriptor for "how a stored value or index maps to a
//! physical or world quantity". The format had this concept scattered across two ad-hoc shapes — the
//! array's `rescale_slope`/`rescale_intercept` (sample → physical) and [`WorldFrame`] (voxel → world).
//! This module names the general pattern they are both instances of: a `(transform, unit, frame)`
//! descriptor whose `transform` is one of a small closed taxonomy.
//!
//! **Store, don't compute** (one of the six invariants): the mapping is *data* carried in the manifest,
//! never re-derived at read time. **Feature-by-presence**: absence of a descriptor means "stored value
//! *is* the quantity" (i.e. [`Transform::Identity`]), not "unknown".
//!
//! The existing [`crate::block::array::ArraySpec`] keeps its convenience accessors (`to_physical`,
//! `world_frame`); this module is the unifying *type* those instances project into, with lossless
//! [`Transform::from_rescale`] / [`Transform::from_world_frame`] bridges so there is a single taxonomy
//! to reason about, serialize, and extend (e.g. the irregular-axis [`Transform::Lookup`] case).

use serde::{Deserialize, Serialize};

use crate::block::array::WorldFrame;

/// The transform taxonomy (ADR-0032): the closed set of ways stored numbers become physical
/// quantities. Tagged by `kind` so it is self-describing on the wire and extends additively.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Transform {
    /// The stored value already *is* the physical value (no-op). The feature-by-presence default.
    Identity,
    /// 1-D affine: `physical = stored * slope + intercept`. The array `rescale_*` case (CT → HU,
    /// PET → Bq/mL). Invertible iff `slope != 0`.
    #[serde(rename = "affine_1d")]
    Affine1d { slope: f64, intercept: f64 },
    /// N-D affine: a homogeneous index→world matrix of `dims` rows × `dims+1` columns, **row-major**
    /// (`[R | t]`, implicit last row `[0…0 1]`). The [`WorldFrame`] voxel→world case.
    #[serde(rename = "affine_nd")]
    AffineNd { dims: u8, matrix: Vec<f64> },
    /// Irregular axis: an explicit lookup table mapping integer index → physical value (e.g. variable
    /// PET frame mid-times, non-uniform energy bins). The escape hatch when no closed-form affine fits.
    Lookup { values: Vec<f64> },
}

impl Transform {
    /// Bridge the array rescale pair into the taxonomy: both `None` → [`Self::Identity`]; otherwise a
    /// 1-D affine with the fd5 defaults (slope 1, intercept 0). Lossless and total.
    pub fn from_rescale(slope: Option<f64>, intercept: Option<f64>) -> Self {
        match (slope, intercept) {
            (None, None) => Transform::Identity,
            (s, i) => Transform::Affine1d {
                slope: s.unwrap_or(1.0),
                intercept: i.unwrap_or(0.0),
            },
        }
    }

    /// Bridge a [`WorldFrame`] into the taxonomy as the 3-D affine instance (its `affine` is the
    /// row-major `[R | t]`). The convention/unit/frame travel on the enclosing [`Referenced`].
    pub fn from_world_frame(frame: &WorldFrame) -> Self {
        Transform::AffineNd {
            dims: 3,
            matrix: frame.affine.to_vec(),
        }
    }

    /// Map a single stored scalar — or, for [`Self::Lookup`], an integer index — to its physical value.
    /// Returns `None` for [`Self::AffineNd`] (use [`Self::apply_point`]) and for an out-of-range or
    /// non-integral `Lookup` index.
    pub fn apply_scalar(&self, stored: f64) -> Option<f64> {
        match self {
            Transform::Identity => Some(stored),
            Transform::Affine1d { slope, intercept } => Some(stored * slope + intercept),
            Transform::Lookup { values } => (stored.fract() == 0.0 && stored >= 0.0)
                .then(|| values.get(stored as usize).copied())
                .flatten(),
            Transform::AffineNd { .. } => None,
        }
    }

    /// Invert a 1-D mapping (physical → stored) for the scalar taxonomy members. `None` for
    /// [`Self::AffineNd`] and for a non-invertible affine (`slope == 0`); [`Self::Lookup`] inverts by
    /// exact-match search (the smallest index whose value equals `physical`).
    pub fn invert_scalar(&self, physical: f64) -> Option<f64> {
        match self {
            Transform::Identity => Some(physical),
            Transform::Affine1d { slope, intercept } => {
                (*slope != 0.0).then(|| (physical - intercept) / slope)
            }
            Transform::Lookup { values } => {
                values.iter().position(|&v| v == physical).map(|i| i as f64)
            }
            Transform::AffineNd { .. } => None,
        }
    }

    /// Apply an N-D affine to a homogeneous index point (length `dims`), returning the world point.
    /// [`Self::Identity`] passes the point through unchanged. `None` for the 1-D scalar members and on
    /// a dimension/shape mismatch.
    pub fn apply_point(&self, index: &[f64]) -> Option<Vec<f64>> {
        match self {
            Transform::Identity => Some(index.to_vec()),
            Transform::AffineNd { dims, matrix } => {
                let n = *dims as usize;
                if index.len() != n || matrix.len() != n * (n + 1) {
                    return None;
                }
                let mut out = vec![0.0f64; n];
                for (r, slot) in out.iter_mut().enumerate() {
                    let base = r * (n + 1);
                    let mut acc = matrix[base + n]; // translation column
                    for (c, &x) in index.iter().enumerate() {
                        acc += matrix[base + c] * x;
                    }
                    *slot = acc;
                }
                Some(out)
            }
            _ => None,
        }
    }
}

/// ADR-0032 §Status-note: the **pinned** closed vocabulary of named reference frames (an Accepted
/// precondition — frames are interoperable codes, not free text). A descriptor's `frame` must be one of
/// these or an `"atlas:<id>"` prefix: `physical` (rescaled intensity / quantity), `epoch` (elapsed time
/// since a recorded instant), and the ADR-0030 spatial spaces `patient` / `scanner` / `aligned`. Units
/// are **UCUM** unless [`Referenced::vocabulary`] names another controlled vocabulary (the §3 escape).
pub const CANONICAL_FRAMES: &[&str] = &["physical", "epoch", "patient", "scanner", "aligned"];

/// ADR-0032 referenced-coordinate descriptor: a `transform` plus the `unit` of its output quantity and
/// the named `frame` it lands in. One type for sample-value rescaling, spatial referencing, and
/// irregular axes alike — so referencing is described, serialized, and extended in a single place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Referenced {
    pub transform: Transform,
    /// UCUM unit of the transform's *output* (e.g. `"HU"`, `"Bq/mL"`, `"mm"`, `"s"`). When the quantity
    /// has no UCUM unit, leave this as the bare domain code and name the controlled vocabulary in
    /// [`Self::vocabulary`] (the §3 escape).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// ADR-0032 §3 **vocabulary escape**: the controlled vocabulary the `unit` code is drawn from when it
    /// is *not* UCUM (reusing the fd5 `_vocabulary`/`_code` convention — e.g. `"DICOM"`, `"SNOMED"`,
    /// `"UCUM"` made explicit). `None` ⇒ `unit` is a plain UCUM symbol. This keeps non-physical/coded
    /// quantities (modality codes, categorical axes) expressible without a separate type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<String>,
    /// Named reference frame the output lands in (e.g. `"physical"`, `"patient"`, `"scanner"`,
    /// `"atlas:<id>"`, `"epoch"`). `None` for a bare value rescale with no spatial frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
}

impl Referenced {
    /// The identity descriptor: stored value *is* the quantity, in `unit` (no frame).
    pub fn identity(unit: Option<String>) -> Self {
        Referenced {
            transform: Transform::Identity,
            unit,
            vocabulary: None,
            frame: None,
        }
    }

    /// Build the sample-value descriptor from an array's rescale pair + unit (the `affine_1d` instance,
    /// frame `"physical"` when a non-identity rescale is present).
    pub fn from_rescale(slope: Option<f64>, intercept: Option<f64>, unit: Option<String>) -> Self {
        let transform = Transform::from_rescale(slope, intercept);
        let frame = (!matches!(transform, Transform::Identity)).then(|| "physical".to_string());
        Referenced {
            transform,
            unit,
            vocabulary: None,
            frame,
        }
    }

    /// Build the spatial descriptor from a [`WorldFrame`] (the `affine_nd` instance), carrying its
    /// world unit and named space across verbatim.
    pub fn from_world_frame(frame: &WorldFrame) -> Self {
        Referenced {
            transform: Transform::from_world_frame(frame),
            unit: Some(frame.unit.clone()),
            vocabulary: None,
            frame: Some(frame.space.clone()),
        }
    }

    /// ADR-0032 §5/§6 **regular time-axis** instance: frame index → elapsed seconds since the frame's
    /// `epoch`, `seconds = index * step_s + start_s`. A 1-D affine; unit `"s"`, frame `"epoch"`. The
    /// common fixed-cadence dynamic series (e.g. uniform PET frames, a regular gate).
    pub fn time_regular(start_s: f64, step_s: f64) -> Self {
        Referenced {
            transform: Transform::Affine1d {
                slope: step_s,
                intercept: start_s,
            },
            unit: Some("s".into()),
            vocabulary: None,
            frame: Some("epoch".into()),
        }
    }

    /// ADR-0032 §5/§6 **irregular time-axis** instance: frame index → an explicit mid-time table (s)
    /// since `epoch`, for variable-duration frames (the classic non-uniform PET protocol). A lookup;
    /// unit `"s"`, frame `"epoch"`. Store-don't-compute — the per-frame times are data, not re-derived.
    pub fn time_irregular(mid_times_s: Vec<f64>) -> Self {
        Referenced {
            transform: Transform::Lookup {
                values: mid_times_s,
            },
            unit: Some("s".into()),
            vocabulary: None,
            frame: Some("epoch".into()),
        }
    }

    /// ADR-0032 §6 high-rate **integer-tick** event timestamps: an event's integer tick count → elapsed
    /// seconds since `epoch`, `seconds = tick * period_s + start_s`. Storing integer ticks + a scale
    /// (rather than float seconds) avoids float-accumulation drift over long high-rate streams — e.g.
    /// ps-resolution coincidence timestamps in listmode PET. A 1-D affine; unit `"s"`, frame `"epoch"`.
    /// The integer tick is the *stored* value; the period carries the resolution.
    pub fn time_ticks(period_s: f64, start_s: f64) -> Self {
        Referenced {
            transform: Transform::Affine1d {
                slope: period_s,
                intercept: start_s,
            },
            unit: Some("s".into()),
            vocabulary: None,
            frame: Some("epoch".into()),
        }
    }

    /// Builder: attach the §3 vocabulary escape naming the controlled vocabulary `unit` is drawn from.
    pub fn with_vocabulary(mut self, vocabulary: &str) -> Self {
        self.vocabulary = Some(vocabulary.into());
        self
    }

    /// Whether `frame` is drawn from the pinned [`CANONICAL_FRAMES`] vocabulary (or is an `"atlas:<id>"`
    /// reference, or absent — an unframed value rescale is legal). The ADR-0032 promotion precondition
    /// that frames stay interoperable codes rather than free text.
    pub fn frame_is_canonical(&self) -> bool {
        match self.frame.as_deref() {
            None => true,
            Some(f) => CANONICAL_FRAMES.contains(&f) || f.starts_with("atlas:"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_passthrough_both_directions() {
        let t = Transform::Identity;
        assert_eq!(t.apply_scalar(42.0), Some(42.0));
        assert_eq!(t.invert_scalar(42.0), Some(42.0));
        assert_eq!(t.apply_point(&[1.0, 2.0, 3.0]), Some(vec![1.0, 2.0, 3.0]));
    }

    #[test]
    fn affine_1d_roundtrips_and_matches_rescale_semantics() {
        // CT rescale: HU = stored*1 + (-1024). from_rescale must agree with ArraySpec::to_physical.
        let t = Transform::from_rescale(Some(1.0), Some(-1024.0));
        assert_eq!(
            t,
            Transform::Affine1d {
                slope: 1.0,
                intercept: -1024.0
            }
        );
        let hu = t.apply_scalar(900.0).unwrap();
        assert_eq!(hu, -124.0);
        // exact inverse recovers the stored value
        assert_eq!(t.invert_scalar(hu), Some(900.0));
        // both-None rescale collapses to Identity (feature-by-presence)
        assert_eq!(Transform::from_rescale(None, None), Transform::Identity);
        // a zero-slope affine is non-invertible
        assert_eq!(
            Transform::Affine1d {
                slope: 0.0,
                intercept: 5.0
            }
            .invert_scalar(5.0),
            None
        );
    }

    #[test]
    fn lookup_indexes_irregular_axis_and_inverts_by_match() {
        // variable PET frame mid-times (s) — an irregular axis no affine can express
        let t = Transform::Lookup {
            values: vec![15.0, 45.0, 90.0, 165.0],
        };
        assert_eq!(t.apply_scalar(2.0), Some(90.0));
        assert_eq!(t.apply_scalar(4.0), None); // out of range
        assert_eq!(t.apply_scalar(1.5), None); // non-integral index
        assert_eq!(t.invert_scalar(165.0), Some(3.0));
        assert_eq!(t.invert_scalar(1.0), None); // no such frame time
    }

    #[test]
    fn affine_nd_from_world_frame_maps_voxel_to_world() {
        // 2 mm iso, origin (-100,-100,-50), LPS — voxel (10,20,5) → world.
        let wf = WorldFrame {
            affine: [
                2.0, 0.0, 0.0, -100.0, //
                0.0, 2.0, 0.0, -100.0, //
                0.0, 0.0, 2.0, -50.0,
            ],
            convention: "LPS".into(),
            unit: "mm".into(),
            space: "patient".into(),
        };
        let r = Referenced::from_world_frame(&wf);
        assert_eq!(r.unit.as_deref(), Some("mm"));
        assert_eq!(r.frame.as_deref(), Some("patient"));
        let world = r.transform.apply_point(&[10.0, 20.0, 5.0]).unwrap();
        assert_eq!(world, vec![-80.0, -60.0, -40.0]);
        // scalar application is undefined for an N-D affine
        assert_eq!(r.transform.apply_scalar(1.0), None);
        // shape mismatch is rejected, not silently wrong
        assert_eq!(r.transform.apply_point(&[1.0, 2.0]), None);
    }

    #[test]
    fn referenced_from_rescale_tags_physical_frame_only_when_non_identity() {
        let id = Referenced::from_rescale(None, None, Some("HU".into()));
        assert_eq!(id.transform, Transform::Identity);
        assert_eq!(id.frame, None);
        let r = Referenced::from_rescale(Some(2.0), Some(0.0), Some("Bq/mL".into()));
        assert_eq!(r.frame.as_deref(), Some("physical"));
        assert_eq!(r.unit.as_deref(), Some("Bq/mL"));
    }

    #[test]
    fn regular_time_axis_maps_frame_index_to_elapsed_seconds() {
        // 30 s frames starting at t=0: frame 0 → 0 s, frame 4 → 120 s; inverse recovers the index.
        let t = Referenced::time_regular(0.0, 30.0);
        assert_eq!(t.unit.as_deref(), Some("s"));
        assert_eq!(t.frame.as_deref(), Some("epoch"));
        assert_eq!(t.transform.apply_scalar(0.0), Some(0.0));
        assert_eq!(t.transform.apply_scalar(4.0), Some(120.0));
        assert_eq!(t.transform.invert_scalar(120.0), Some(4.0));
        // a non-zero start offsets the epoch
        assert_eq!(
            Referenced::time_regular(15.0, 30.0)
                .transform
                .apply_scalar(0.0),
            Some(15.0)
        );
    }

    #[test]
    fn irregular_time_axis_indexes_variable_frame_midtimes() {
        // a classic non-uniform PET protocol: short early frames, long late frames
        let t = Referenced::time_irregular(vec![5.0, 15.0, 30.0, 60.0, 150.0]);
        assert_eq!(t.unit.as_deref(), Some("s"));
        assert_eq!(t.frame.as_deref(), Some("epoch"));
        assert_eq!(t.transform.apply_scalar(3.0), Some(60.0));
        assert_eq!(t.transform.apply_scalar(5.0), None); // past the last frame
        assert_eq!(t.transform.invert_scalar(150.0), Some(4.0));
    }

    #[test]
    fn every_constructor_lands_in_a_pinned_frame() {
        // every built-in instance must use the pinned vocabulary (ADR-0032 promotion precondition).
        for r in [
            Referenced::from_rescale(Some(1.0), Some(-1024.0), Some("HU".into())), // physical
            Referenced::time_regular(0.0, 30.0),                                   // epoch
            Referenced::time_irregular(vec![5.0, 15.0]),                           // epoch
            Referenced::time_ticks(1e-12, 0.0),                                    // epoch
            Referenced::identity(Some("HU".into())),                               // no frame
        ] {
            assert!(
                r.frame_is_canonical(),
                "{:?} must use a pinned frame",
                r.frame
            );
        }
        // a spatial descriptor's frame comes from WorldFrame.space — patient/scanner/aligned/atlas are pinned.
        let wf = WorldFrame {
            affine: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            convention: "LPS".into(),
            unit: "mm".into(),
            space: "atlas:mni152".into(),
        };
        assert!(Referenced::from_world_frame(&wf).frame_is_canonical());
        // free-text frames are rejected
        let bogus = Referenced {
            transform: Transform::Identity,
            unit: None,
            vocabulary: None,
            frame: Some("whatever".into()),
        };
        assert!(!bogus.frame_is_canonical());
    }

    #[test]
    fn integer_tick_event_times_avoid_float_drift() {
        // ps-resolution coincidence ticks: integer tick 1_000_000 at 1 ps period → 1 µs since epoch.
        let t = Referenced::time_ticks(1e-12, 0.0);
        assert_eq!(t.unit.as_deref(), Some("s"));
        assert_eq!(t.frame.as_deref(), Some("epoch"));
        // the stored value is an exact integer; only the scale carries resolution.
        assert_eq!(t.transform.apply_scalar(1_000_000.0), Some(1e-6));
        // a start offset shifts the epoch origin.
        assert_eq!(
            Referenced::time_ticks(1e-9, 5.0)
                .transform
                .apply_scalar(0.0),
            Some(5.0)
        );
    }

    #[test]
    fn vocabulary_escape_carries_non_ucum_codes() {
        // a coded (non-UCUM) quantity: the unit is a domain code, the vocab names its source
        let r = Referenced::identity(Some("CT".into())).with_vocabulary("DICOM");
        assert_eq!(r.unit.as_deref(), Some("CT"));
        assert_eq!(r.vocabulary.as_deref(), Some("DICOM"));
        // plain UCUM units leave the escape empty (skipped on the wire)
        let u = Referenced::from_rescale(Some(1.0), Some(0.0), Some("HU".into()));
        assert_eq!(u.vocabulary, None);
        let j = serde_json::to_value(&u).unwrap();
        assert!(
            j.get("vocabulary").is_none(),
            "UCUM unit omits the vocabulary escape"
        );
        // the escape roundtrips when present
        let back: Referenced = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn descriptor_serializes_self_describing_by_kind() {
        let r = Referenced::from_rescale(Some(1.0), Some(-1024.0), Some("HU".into()));
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["transform"]["kind"], "affine_1d");
        assert_eq!(j["transform"]["slope"], 1.0);
        assert_eq!(j["unit"], "HU");
        // roundtrip
        let back: Referenced = serde_json::from_value(j).unwrap();
        assert_eq!(back, r);
    }
}
