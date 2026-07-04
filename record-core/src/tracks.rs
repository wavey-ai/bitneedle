//! Track metadata as revolution ranges (final compact format, section 2.5).
//!
//! A track spans one or more consecutive revolutions. There is no separate
//! payload-entry mapping: the track range *is* the revolution range, and for
//! ECDC entries `revolution_index == payload_entry_index`.
//!
//! Inter-track silence is not an implicit "uncovered revolution" sentinel: it
//! is an explicit [`TrackGapRange`], a programme classification distinct from
//! `Track`. The union of track ranges and track-gap ranges must cover every
//! playable revolution exactly once; a playable revolution covered by neither
//! (or by both) is a malformed programme and is rejected.

use anyhow::{bail, Result};

/// One track: a title and the consecutive range of revolutions it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackRange {
    pub title: String,
    pub first_revolution_index: u64,
    pub revolution_count: u64,
}

impl TrackRange {
    pub fn last_revolution_index(&self) -> u64 {
        self.first_revolution_index + self.revolution_count - 1
    }
}

/// One explicit inter-track gap: a consecutive range of revolutions that is
/// real, independently decodable audio (typically ambient near-silence) but
/// is intentionally not part of any track. `after_track_index` is the
/// 0-based index into the track list of the track this gap immediately
/// follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackGapRange {
    pub first_revolution_index: u64,
    pub revolution_count: u64,
    pub after_track_index: usize,
}

impl TrackGapRange {
    pub fn last_revolution_index(&self) -> u64 {
        self.first_revolution_index + self.revolution_count - 1
    }
}

/// Validate a complete set of track ranges (plus explicit track-gap ranges)
/// against section 2.5's rules.
///
/// `is_playable_revolution[i]` is `true` when payload entry `i` is an ECDC
/// (playable) revolution; non-ECDC auxiliary entries must be `false` and
/// must not be covered by any track or track-gap range. Tracks and gaps must
/// each be in-bounds and non-overlapping with every other track and gap, and
/// must never cover an auxiliary entry.
///
/// Every playable revolution must be covered by exactly one track or track
/// gap. A playable revolution covered by neither, or by more than one span,
/// is rejected — there is no implicit "uncovered means gap" fallback.
pub fn validate_track_ranges(
    tracks: &[TrackRange],
    track_gaps: &[TrackGapRange],
    is_playable_revolution: &[bool],
) -> Result<()> {
    if tracks.is_empty() {
        bail!("at least one track is required");
    }

    let mut covered_by = vec![None; is_playable_revolution.len()];
    let mut previous_end: Option<u64> = None;

    for (track_index, track) in tracks.iter().enumerate() {
        if track.revolution_count == 0 {
            bail!(
                "track {track_index} (\"{}\") has zero revolution count",
                track.title
            );
        }

        let end = track
            .first_revolution_index
            .checked_add(track.revolution_count)
            .ok_or_else(|| anyhow::anyhow!("track {track_index} revolution range overflows"))?;

        if end as usize > is_playable_revolution.len() {
            bail!(
                "track {track_index} (\"{}\") revolution range is out of bounds",
                track.title
            );
        }

        if let Some(previous_end) = previous_end {
            if track.first_revolution_index < previous_end {
                bail!(
                    "track {track_index} (\"{}\") overlaps the previous track's revolution range",
                    track.title
                );
            }
        }

        for revolution_index in track.first_revolution_index..end {
            if !is_playable_revolution[revolution_index as usize] {
                bail!(
                    "track {track_index} (\"{}\") covers non-ECDC auxiliary revolution {revolution_index}",
                    track.title
                );
            }
            covered_by[revolution_index as usize] = Some(track_index);
        }

        previous_end = Some(end);
    }

    for (gap_index, gap) in track_gaps.iter().enumerate() {
        if gap.revolution_count == 0 {
            bail!("track gap {gap_index} has zero revolution count");
        }

        if gap.after_track_index >= tracks.len() {
            bail!(
                "track gap {gap_index} after_track_index {} is out of range for {} tracks",
                gap.after_track_index,
                tracks.len()
            );
        }

        let end = gap
            .first_revolution_index
            .checked_add(gap.revolution_count)
            .ok_or_else(|| anyhow::anyhow!("track gap {gap_index} revolution range overflows"))?;

        if end as usize > is_playable_revolution.len() {
            bail!("track gap {gap_index} revolution range is out of bounds");
        }

        for revolution_index in gap.first_revolution_index..end {
            if !is_playable_revolution[revolution_index as usize] {
                bail!(
                    "track gap {gap_index} covers non-ECDC auxiliary revolution {revolution_index}"
                );
            }
            if let Some(existing) = covered_by[revolution_index as usize] {
                if existing < tracks.len() {
                    bail!(
                        "revolution {revolution_index} is covered by both track {existing} and track gap {gap_index}"
                    );
                }
                bail!(
                    "revolution {revolution_index} is covered by more than one track gap ({} and {gap_index})",
                    existing - tracks.len()
                );
            }
            covered_by[revolution_index as usize] = Some(tracks.len() + gap_index);
        }
    }

    for (revolution_index, (playable, owner)) in is_playable_revolution
        .iter()
        .zip(covered_by.iter())
        .enumerate()
    {
        if *playable && owner.is_none() {
            bail!("revolution {revolution_index} is not covered by any track or track gap");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, first: u64, count: u64) -> TrackRange {
        TrackRange {
            title: title.to_string(),
            first_revolution_index: first,
            revolution_count: count,
        }
    }

    fn gap(first: u64, count: u64, after_track_index: usize) -> TrackGapRange {
        TrackGapRange {
            first_revolution_index: first,
            revolution_count: count,
            after_track_index,
        }
    }

    #[test]
    fn one_track_spanning_one_revolution() {
        let tracks = [track("Side A", 0, 1)];
        validate_track_ranges(&tracks, &[], &[true]).unwrap();
    }

    #[test]
    fn one_track_spanning_multiple_revolutions() {
        let tracks = [track("Side A", 0, 3)];
        validate_track_ranges(&tracks, &[], &[true, true, true]).unwrap();
    }

    #[test]
    fn multiple_adjacent_track_ranges() {
        let tracks = [track("Track 1", 0, 2), track("Track 2", 2, 1)];
        validate_track_ranges(&tracks, &[], &[true, true, true]).unwrap();
    }

    #[test]
    fn zero_revolution_count_rejected() {
        let tracks = [track("Track 1", 0, 0)];
        let err = validate_track_ranges(&tracks, &[], &[true]).unwrap_err();
        assert!(err.to_string().contains("zero revolution count"));
    }

    #[test]
    fn out_of_range_first_revolution_rejected() {
        let tracks = [track("Track 1", 5, 1)];
        let err = validate_track_ranges(&tracks, &[], &[true, true]).unwrap_err();
        assert!(err.to_string().contains("out of bounds"));
    }

    #[test]
    fn overlapping_ranges_rejected() {
        let tracks = [track("Track 1", 0, 2), track("Track 2", 1, 2)];
        let err = validate_track_ranges(&tracks, &[], &[true, true, true]).unwrap_err();
        assert!(err.to_string().contains("overlaps"));
    }

    #[test]
    fn uncovered_playable_revolution_is_rejected_without_an_explicit_gap() {
        // Entry 1 is a playable ECDC revolution that no track or gap covers:
        // there is no implicit "uncovered means gap" fallback any more.
        let tracks = [track("Track 1", 0, 1)];
        let err = validate_track_ranges(&tracks, &[], &[true, true]).unwrap_err();
        assert!(err
            .to_string()
            .contains("not covered by any track or track gap"));
    }

    #[test]
    fn explicit_track_gap_covers_the_uncovered_revolution() {
        let tracks = [track("Track 1", 0, 1), track("Track 2", 2, 1)];
        let gaps = [gap(1, 1, 0)];
        validate_track_ranges(&tracks, &gaps, &[true, true, true]).unwrap();
    }

    #[test]
    fn track_gap_overlapping_a_track_is_rejected() {
        let tracks = [track("Track 1", 0, 2)];
        let gaps = [gap(1, 1, 0)];
        let err = validate_track_ranges(&tracks, &gaps, &[true, true]).unwrap_err();
        assert!(err.to_string().contains("covered by both"));
    }

    #[test]
    fn track_gap_with_out_of_range_after_track_index_is_rejected() {
        let tracks = [track("Track 1", 0, 1)];
        let gaps = [gap(1, 1, 1)];
        let err = validate_track_ranges(&tracks, &gaps, &[true, true]).unwrap_err();
        assert!(err.to_string().contains("after_track_index"));
    }

    #[test]
    fn auxiliary_entry_inside_range_rejected() {
        let tracks = [track("Track 1", 0, 2)];
        let err = validate_track_ranges(&tracks, &[], &[true, false]).unwrap_err();
        assert!(err.to_string().contains("non-ECDC auxiliary"));
    }

    #[test]
    fn auxiliary_entry_outside_range_is_fine() {
        let tracks = [track("Track 1", 0, 1)];
        validate_track_ranges(&tracks, &[], &[true, false]).unwrap();
    }
}
