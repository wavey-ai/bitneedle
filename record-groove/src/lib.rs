// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

#![doc = include_str!("../README.md")]

use anyhow::{bail, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

static BALANCED_CACHE: OnceLock<Mutex<BoundedCache<([u8; 3], u64), TonedConfig>>> =
    OnceLock::new();
static PALETTE_CACHE: OnceLock<Mutex<BoundedCache<TonedConfig, Arc<TonedPalette>>>> =
    OnceLock::new();

/// A process-wide cache with a ceiling on it.
///
/// Palettes are deterministic and immutable, so caching them is free as far
/// as correctness goes — but a palette at 16 bits per pixel is 65,536
/// colours in a `Vec` plus the same again in a lookup map, and a record with
/// several tone spans builds one per base tone. Left unbounded that is
/// megabytes of resident memory that never comes back, on a device that has
/// none to spare. Least-recently-used, capped: the working set of a single
/// record is a handful of palettes, and anything past that is a record
/// nobody is looking at any more.
struct BoundedCache<K, V> {
    entries: HashMap<K, (V, u64)>,
    capacity: usize,
    clock: u64,
}

impl<K: std::hash::Hash + Eq + Copy, V: Clone> BoundedCache<K, V> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity: capacity.max(1),
            clock: 0,
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        self.clock = self.clock.wrapping_add(1);
        let clock = self.clock;
        let (value, used) = self.entries.get_mut(key)?;
        *used = clock;
        Some(value.clone())
    }

    fn insert(&mut self, key: K, value: V) {
        self.clock = self.clock.wrapping_add(1);
        if self.entries.len() >= self.capacity && !self.entries.contains_key(&key) {
            if let Some(stalest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| *key)
            {
                self.entries.remove(&stalest);
            }
        }
        self.entries.insert(key, (value, self.clock));
    }
}

/// The cache's lock, recovered rather than propagated.
fn lock<T>(cache: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How many palettes stay resident. A record carries a handful of tone spans
/// at most, so this holds a whole record's worth and then some.
const PALETTE_CACHE_CAPACITY: usize = 8;
/// Balanced configurations are a few dozen bytes each, so this can be
/// generous — it exists to stop unbounded growth, not to save space.
const BALANCED_CACHE_CAPACITY: usize = 256;

/// Number of RGB pixels needed to carry `byte_length` bytes at 3 bytes per pixel.
pub fn pixel_count_for_byte_length(byte_length: usize) -> usize {
    byte_length.div_ceil(3)
}

/// Recovers the byte stream packed into the RGB channels of an RGBA buffer,
/// skipping fully transparent pixels. When `byte_length` is given the result
/// is truncated to that length.
pub fn rgba_to_bytes(rgba: &[u8], byte_length: Option<usize>) -> Result<Vec<u8>> {
    if rgba.len() % 4 != 0 {
        bail!("RGBA length must be divisible by 4");
    }

    let mut bytes = Vec::with_capacity((rgba.len() / 4) * 3);

    for chunk in rgba.chunks_exact(4) {
        if chunk[3] == 0 {
            continue;
        }

        bytes.extend_from_slice(&chunk[..3]);
    }

    if let Some(byte_length) = byte_length {
        if byte_length > bytes.len() {
            bail!("requested byte length exceeds decoded RGB payload");
        }

        bytes.truncate(byte_length);
    }

    Ok(bytes)
}

/// Packs a byte stream into opaque RGBA pixels, 3 bytes per pixel. The final
/// pixel is zero-padded when the length is not a multiple of 3.
pub fn bytes_to_rgba(bytes: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixel_count_for_byte_length(bytes.len()) * 4);

    for chunk in bytes.chunks(3) {
        let mut pixel = [0u8, 0, 0, 255];
        pixel[..chunk.len()].copy_from_slice(chunk);
        rgba.extend_from_slice(&pixel);
    }

    rgba
}

/// Packs a byte stream into opaque grayscale RGBA pixels, one byte per pixel
/// with R = G = B = byte.
pub fn bytes_to_grayscale_rgba(bytes: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(bytes.len() * 4);

    for &byte in bytes {
        rgba.extend_from_slice(&[byte, byte, byte, 255]);
    }

    rgba
}

/// Recovers the byte stream packed into grayscale RGBA pixels, skipping fully
/// transparent pixels. Fails if an opaque pixel is not grayscale (R = G = B).
/// When `byte_length` is given the result is truncated to that length.
pub fn grayscale_rgba_to_bytes(rgba: &[u8], byte_length: Option<usize>) -> Result<Vec<u8>> {
    if rgba.len() % 4 != 0 {
        bail!("RGBA length must be divisible by 4");
    }

    let mut bytes = Vec::with_capacity(rgba.len() / 4);

    for chunk in rgba.chunks_exact(4) {
        if chunk[3] == 0 {
            continue;
        }

        if chunk[0] != chunk[1] || chunk[1] != chunk[2] {
            bail!("pixel is not grayscale");
        }

        bytes.push(chunk[0]);
    }

    if let Some(byte_length) = byte_length {
        if byte_length > bytes.len() {
            bail!("requested byte length exceeds decoded grayscale payload");
        }

        bytes.truncate(byte_length);
    }

    Ok(bytes)
}

/// Copies the RGB channels of one pixel to another, leaving alpha untouched.
pub fn copy_rgb(
    source: &[u8],
    source_pixel_index: usize,
    target: &mut [u8],
    target_pixel_index: usize,
) {
    let source_offset = source_pixel_index * 4;
    let target_offset = target_pixel_index * 4;
    target[target_offset] = source[source_offset];
    target[target_offset + 1] = source[source_offset + 1];
    target[target_offset + 2] = source[source_offset + 2];
}

/// Rec. 709 luma of a pixel in an RGBA buffer, in the 0.0–255.0 range.
pub fn luma_rec709(data: &[u8], pixel_index: usize) -> f64 {
    let offset = pixel_index * 4;
    0.2126 * data[offset] as f64
        + 0.7152 * data[offset + 1] as f64
        + 0.0722 * data[offset + 2] as f64
}

fn rec709_luma_of(color: [u8; 3]) -> f64 {
    0.2126 * color[0] as f64 + 0.7152 * color[1] as f64 + 0.0722 * color[2] as f64
}

/// Rec. 709 chroma (Cb, Cr) of a colour, signed and centred on 0.
pub fn chroma_rec709(color: [u8; 3]) -> (f64, f64) {
    chroma_rec709_with_luma(color, rec709_luma_of(color))
}

/// [`chroma_rec709`] for a colour whose luma the caller already has. The
/// enumerations below compute luma to decide whether a colour is in the
/// window at all, so recomputing it here was a third of their work.
#[inline]
fn chroma_rec709_with_luma(color: [u8; 3], luma: f64) -> (f64, f64) {
    let cb = (color[2] as f64 - luma) / 1.8556;
    let cr = (color[0] as f64 - luma) / 1.5748;
    (cb, cr)
}

fn chroma_distance(a: [u8; 3], b: [u8; 3]) -> f64 {
    chroma_distance_from(a, rec709_luma_of(a), chroma_rec709(b))
}

/// Chroma distance from `color` to a tone whose chroma is already known —
/// the base tone is fixed for a whole enumeration, so its half of the
/// arithmetic is hoisted out. Same operations in the same order as
/// [`chroma_distance`], so the two agree bit for bit.
#[inline]
fn chroma_distance_from(color: [u8; 3], luma: f64, base_chroma: (f64, f64)) -> f64 {
    let (a_cb, a_cr) = chroma_rec709_with_luma(color, luma);
    let (b_cb, b_cr) = base_chroma;
    ((a_cb - b_cb).powi(2) + (a_cr - b_cr).powi(2)).sqrt()
}

/// How palette colours are ordered (and therefore which `2^bits_per_pixel`
/// subset of the iso-luma candidates carries the data). Both orderings are
/// deterministic, so the same config always rebuilds the same palette and the
/// byte stream decodes exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ToneOrdering {
    /// Closest RGB distance to the base tone first.
    #[default]
    BaseProximity,
    /// Closest Rec. 709 chroma (Cb/Cr) distance to the base tone first. This
    /// pulls the palette's average colour toward the base tone's hue instead
    /// of the iso-luma set's centroid (which skews green at high luma).
    ChromaProximity,
}

pub mod span;
pub use span::{decode_toned_spans, encode_toned_spans, ToneRequest, ToneSpan, TonedRender};

pub mod oklch;
pub use oklch::{
    adaptive_gap_tone_lightness, lighten_base_oklch, oklch_lightness, validate_gap_tone_lightness,
};

/// Configuration for a [`TonedPalette`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TonedConfig {
    /// Base tone whose luma every pixel preserves (within `luma_tolerance`).
    pub base: [u8; 3],
    /// Allowed deviation, in rounded Rec. 709 luma steps, around the base
    /// tone's luma. `0` keeps brightness exactly flat; larger values unlock
    /// more colours (capacity) at the cost of brightness drift.
    pub luma_tolerance: u8,
    /// Bits encoded per pixel. The palette must contain at least
    /// `2^bits_per_pixel` iso-luma colours, so this is bounded by
    /// `luma_tolerance`. Higher means smaller output (fewer pixels).
    pub bits_per_pixel: u32,
    /// Which candidate colours make up the palette; see [`ToneOrdering`].
    pub ordering: ToneOrdering,
}

impl TonedConfig {
    /// Configuration from a CSS-style hex base tone (`#RRGGBB`, `#RGB`, or the
    /// same without `#`).
    pub fn from_hex(base: &str, luma_tolerance: u8, bits_per_pixel: u32) -> Self {
        let hex = normalized_hex_color(Some(base));
        let parse =
            |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).unwrap_or(0);
        Self {
            base: [parse(1..3), parse(3..5), parse(5..7)],
            luma_tolerance,
            bits_per_pixel,
            ordering: ToneOrdering::default(),
        }
    }

    /// Picks the luma tolerance that best balances brightness drift against
    /// colour cast for `base`, given a size budget.
    ///
    /// `max_size_factor` caps output size relative to raw 24-bit RGB packing
    /// (e.g. `1.25` allows 25% more pixels) and fixes `bits_per_pixel` to the
    /// smallest count that fits the budget. Within that budget, widening the
    /// luma window adds candidate colours, letting [`ToneOrdering::ChromaProximity`]
    /// keep only colours whose hue is near the base tone — less colour cast,
    /// more brightness drift. This searches a tolerance ladder and minimises
    ///
    /// `cost = chroma error of the palette's mean colour + luma tolerance / 8`
    ///
    /// (both in 0–255-ish perceptual units; colour cast is much more visible
    /// than brightness grain, hence the down-weighted luma term — it also
    /// lets dark and very light base tones, whose luma windows clip against
    /// the gamut edge, widen far enough to find same-hue colours). The result
    /// is an ordinary
    /// [`TonedConfig`]: rebuilding a palette from it is deterministic, so
    /// encode and decode sides agree exactly.
    pub fn balanced(base: [u8; 3], max_size_factor: f64) -> Result<Self> {
        if !(max_size_factor >= 1.0) {
            bail!("max size factor must be at least 1.0");
        }

        let cache_key = (base, max_size_factor.to_bits());
        let cache = BALANCED_CACHE
            .get_or_init(|| Mutex::new(BoundedCache::with_capacity(BALANCED_CACHE_CAPACITY)));
        // A poisoned cache is still a cache: a panic elsewhere must not take
        // every later render down with it, least of all across the FFI
        // boundary this is called over.
        if let Some(config) = lock(cache).get(&cache_key) {
            return Ok(config);
        }

        let bits_per_pixel = ((24.0 / max_size_factor).ceil() as u32).clamp(1, 24);
        let needed = 1usize << bits_per_pixel;

        // Enumerate once at the widest tolerance we are willing to consider:
        // the first ladder rung with ~8x the needed colours (3 extra bits of
        // selection headroom), beyond which extra tolerance buys little tint.
        const LADDER: [u8; 12] = [0, 1, 2, 4, 8, 16, 32, 48, 64, 96, 128, 255];
        let max_tolerance = LADDER
            .into_iter()
            .find(|&tol| iso_luma_count(base, tol) >= needed.saturating_mul(8))
            .unwrap_or(255);

        let mut best: Option<(f64, u8)> = None;
        for (tol, mean) in balanced_candidate_means(base, max_tolerance, needed, &LADDER) {
            let cost = chroma_distance(mean, base) + tol as f64 / 8.0;
            if best.is_none_or(|(best_cost, _)| cost < best_cost) {
                best = Some((cost, tol));
            }
        }

        let Some((_, luma_tolerance)) = best else {
            bail!(
                "no luma tolerance up to ±{max_tolerance} yields the {needed} colours \
                 needed for {bits_per_pixel} bits per pixel"
            );
        };

        let config = Self {
            base,
            luma_tolerance,
            bits_per_pixel,
            ordering: ToneOrdering::ChromaProximity,
        };
        lock(cache).insert(cache_key, config);
        Ok(config)
    }
}

/// Number of 8-bit RGB colours whose rounded Rec. 709 luma is within
/// `luma_tolerance` of the base tone's, without materialising them.
fn iso_luma_count(base: [u8; 3], luma_tolerance: u8) -> usize {
    let (min, max) = iso_luma_window(base, luma_tolerance);
    fold_iso_luma_rows(
        || 0usize,
        |count, r| {
            for g in 0..=255u16 {
                let rg = red_green_luma(r, g);
                if let Some((low, high)) = iso_luma_blue_range(rg, min, max) {
                    *count += usize::from(high - low + 1);
                }
            }
        },
        |count, piece| *count += piece,
    )
}

/// The rounded-luma window `[min, max]` a tolerance opens around a base.
fn iso_luma_window(base: [u8; 3], luma_tolerance: u8) -> (f64, f64) {
    let target = rec709_luma_of(base).round();
    (
        target - luma_tolerance as f64,
        target + luma_tolerance as f64,
    )
}

/// The red and green share of a colour's luma. Blue is added on top in
/// [`blue_luma`], in the order [`rec709_luma_of`] evaluates it, so a luma
/// built in two steps here is the same `f64` as one built in one step there.
#[inline]
fn red_green_luma(r: u16, g: u16) -> f64 {
    0.2126 * r as f64 + 0.7152 * g as f64
}

#[inline]
fn blue_luma(rg: f64, b: u16) -> f64 {
    rg + 0.0722 * b as f64
}

/// The blue values `low..=high` at which a colour with red/green luma `rg`
/// has a rounded luma inside `[min, max]`, or `None` when there are none.
///
/// The estimate brackets the answer as before; within it, rounded luma is
/// monotone in blue (each step is a monotone floating-point operation), so
/// each end is found by bisection on exactly the test that used to be run
/// on every blue value. Same colours, eight probes instead of up to 256.
fn iso_luma_blue_range(rg: f64, min: f64, max: f64) -> Option<(u16, u16)> {
    let b_low = ((min - 0.5 - rg) / 0.0722).floor().max(0.0) as u16;
    let b_high = (((max + 0.5 - rg) / 0.0722).ceil().min(255.0) as u16).min(255);
    if b_low > b_high {
        return None;
    }
    let luma = |b: u16| blue_luma(rg, b).round();
    if luma(b_high) < min || luma(b_low) > max {
        return None;
    }

    // Smallest blue whose luma reaches `min`.
    let (mut lo, mut hi) = (b_low, b_high);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if luma(mid) >= min {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let first = lo;
    if luma(first) > max {
        return None;
    }

    // Largest blue whose luma still fits under `max`.
    let (mut lo, mut hi) = (first, b_high);
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if luma(mid) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Some((first, lo))
}

const PALETTE_SELECTION_BUCKETS: usize = 16_384;

#[derive(Clone, Copy)]
struct KeyedColor {
    key: f64,
    color: [u8; 3],
}

/// The per-colour state an enumeration needs to key candidates against the
/// base tone: the tone itself and its chroma, worked out once.
#[derive(Clone, Copy)]
struct SelectionBase {
    base: [u8; 3],
    chroma: (f64, f64),
    ordering: ToneOrdering,
}

impl SelectionBase {
    fn new(base: [u8; 3], ordering: ToneOrdering) -> Self {
        Self {
            base,
            chroma: chroma_rec709(base),
            ordering,
        }
    }

    #[inline]
    fn key(&self, color: [u8; 3], luma: f64) -> f64 {
        palette_selection_key(color, luma, self)
    }

    #[inline]
    fn bucket(&self, key: f64) -> usize {
        palette_selection_bucket(key, self.ordering)
    }
}

#[inline]
fn palette_selection_key(color: [u8; 3], luma: f64, selection: &SelectionBase) -> f64 {
    let base = selection.base;
    match selection.ordering {
        ToneOrdering::ChromaProximity => chroma_distance_from(color, luma, selection.chroma),
        ToneOrdering::BaseProximity => color
            .iter()
            .zip(base.iter())
            .map(|(&a, &b)| ((a as i32 - b as i32).pow(2)) as f64)
            .sum(),
    }
}

fn palette_selection_bucket(key: f64, ordering: ToneOrdering) -> usize {
    let maximum = match ordering {
        // Cb and Cr are each confined to roughly ±127.5, so the distance
        // between any two chroma pairs is safely below 512.
        ToneOrdering::ChromaProximity => 512.0,
        // Three squared 8-bit channel deltas.
        ToneOrdering::BaseProximity => 3.0 * 255.0 * 255.0,
    };
    ((key.clamp(0.0, maximum) / maximum) * (PALETTE_SELECTION_BUCKETS - 1) as f64)
        .floor() as usize
}

fn compare_keyed_colors(a: &KeyedColor, b: &KeyedColor) -> std::cmp::Ordering {
    a.key.total_cmp(&b.key).then(a.color.cmp(&b.color))
}

/// Calls `visit` with every colour in the window and its (unrounded) luma,
/// in enumeration order, on the calling thread.
fn visit_iso_luma_colors(
    base: [u8; 3],
    luma_tolerance: u8,
    mut visit: impl FnMut([u8; 3], f64),
) {
    let (min, max) = iso_luma_window(base, luma_tolerance);
    for r in 0..=255u16 {
        visit_iso_luma_row(r, min, max, &mut visit);
    }
}

/// One red row of the enumeration: every green, and for each the blue
/// range that lands in the window.
#[inline]
fn visit_iso_luma_row(r: u16, min: f64, max: f64, visit: &mut impl FnMut([u8; 3], f64)) {
    for g in 0..=255u16 {
        let rg = red_green_luma(r, g);
        let Some((low, high)) = iso_luma_blue_range(rg, min, max) else {
            continue;
        };
        for b in low..=high {
            visit([r as u8, g as u8, b as u8], blue_luma(rg, b));
        }
    }
}

/// How many threads share an enumeration. wasm32 has no threads to offer,
/// so there it is the caller's alone.
fn enumeration_workers() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
            .min(16)
    }
}

/// Folds the 256 red rows of an enumeration into worker-local state and
/// merges the pieces. Rows are handed out one at a time across the
/// available threads (a row's work varies with how much of the blue range
/// lands in the window, so a static split would leave cores idle). None of
/// this crate's folds care what order rows arrive in — they either add
/// integers or collect colours that are sorted by a total order afterwards
/// — so the answer is the one a single thread would give.
fn fold_iso_luma_rows<S: Send>(
    init: impl Fn() -> S + Sync,
    row: impl Fn(&mut S, u16) + Sync,
    mut merge: impl FnMut(&mut S, S),
) -> S {
    let workers = enumeration_workers();
    if workers <= 1 {
        let mut state = init();
        for r in 0..=255u16 {
            row(&mut state, r);
        }
        return state;
    }

    let next_row = std::sync::atomic::AtomicUsize::new(0);
    let mut pieces = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut state = init();
                    loop {
                        let r = next_row.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if r > 255 {
                            break state;
                        }
                        row(&mut state, r as u16);
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect::<Vec<_>>()
    });
    let mut state = pieces.remove(0);
    for piece in pieces {
        merge(&mut state, piece);
    }
    state
}

/// [`fold_iso_luma_rows`] over every colour in the window, with its luma.
fn fold_iso_luma_colors<S: Send>(
    base: [u8; 3],
    luma_tolerance: u8,
    init: impl Fn() -> S + Sync,
    visit: impl Fn(&mut S, [u8; 3], f64) + Sync,
    merge: impl FnMut(&mut S, S),
) -> S {
    let (min, max) = iso_luma_window(base, luma_tolerance);
    fold_iso_luma_rows(
        init,
        |state, r| visit_iso_luma_row(r, min, max, &mut |color, luma| visit(state, color, luma)),
        merge,
    )
}

/// Sorts `slices` independently, sharing them out across the available
/// threads. Each slice ends up as `sort_unstable_by` would leave it.
fn sort_slices_by<T: Send>(
    slices: Vec<&mut [T]>,
    compare: impl Fn(&T, &T) -> std::cmp::Ordering + Sync,
) {
    let workers = enumeration_workers();
    if workers <= 1 {
        for slice in slices {
            slice.sort_unstable_by(&compare);
        }
        return;
    }
    let slices = std::sync::Mutex::new(slices.into_iter());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let Some(slice) = lock(&slices).next() else {
                    break;
                };
                slice.sort_unstable_by(&compare);
            });
        }
    });
}

fn select_palette_colors(
    base: [u8; 3],
    luma_tolerance: u8,
    bits_per_pixel: u32,
    ordering: ToneOrdering,
) -> Result<Vec<[u8; 3]>> {
    let needed = 1usize << bits_per_pixel;
    let available = iso_luma_count(base, luma_tolerance);
    if available < needed {
        bail!(
            "only {} colours share the base tone's luma (±{}); {} bits per pixel needs {}",
            available,
            luma_tolerance,
            bits_per_pixel,
            needed
        );
    }

    let selection = SelectionBase::new(base, ordering);
    let histogram = fold_iso_luma_colors(
        base,
        luma_tolerance,
        || vec![0u32; PALETTE_SELECTION_BUCKETS],
        |histogram, color, luma| {
            histogram[selection.bucket(selection.key(color, luma))] += 1;
        },
        |histogram, piece| {
            for (total, part) in histogram.iter_mut().zip(piece) {
                *total += part;
            }
        },
    );
    let mut cumulative = 0usize;
    let cutoff_bucket = histogram
        .iter()
        .position(|&count| {
            cumulative += count as usize;
            cumulative >= needed
        })
        .expect("available palette colours must occupy a selection bucket");

    let candidates = fold_iso_luma_colors(
        base,
        luma_tolerance,
        Vec::new,
        |selected, color, luma| {
            let key = selection.key(color, luma);
            if selection.bucket(key) <= cutoff_bucket {
                selected.push(KeyedColor { key, color });
            }
        },
        |selected, piece| selected.extend(piece),
    );

    // The bucket is monotone in the key, so bucket order then key order is
    // the full key order: place each candidate in its bucket's run and sort
    // the runs separately (and in parallel), then cut at `needed`. The same
    // palette one big sort would give, in a fraction of the time.
    let mut starts = Vec::with_capacity(cutoff_bucket + 2);
    let mut offset = 0usize;
    for &count in &histogram[..=cutoff_bucket] {
        starts.push(offset);
        offset += count as usize;
    }
    starts.push(offset);
    let mut cursors = starts[..=cutoff_bucket].to_vec();
    let mut placed = vec![
        KeyedColor {
            key: 0.0,
            color: [0; 3]
        };
        cumulative
    ];
    for entry in candidates {
        let cursor = &mut cursors[selection.bucket(entry.key)];
        placed[*cursor] = entry;
        *cursor += 1;
    }
    let mut runs = Vec::with_capacity(cutoff_bucket + 1);
    let mut rest = placed.as_mut_slice();
    for bucket in 0..=cutoff_bucket {
        let (run, tail) = rest.split_at_mut(starts[bucket + 1] - starts[bucket]);
        rest = tail;
        if run.len() > 1 {
            runs.push(run);
        }
    }
    sort_slices_by(runs, compare_keyed_colors);
    placed.truncate(needed);
    Ok(placed.into_iter().map(|entry| entry.color).collect())
}

fn balanced_candidate_means(
    base: [u8; 3],
    max_tolerance: u8,
    needed: usize,
    ladder: &[u8],
) -> Vec<(u8, [u8; 3])> {
    let tolerances: Vec<u8> = ladder
        .iter()
        .copied()
        .filter(|&tolerance| tolerance <= max_tolerance)
        .collect();
    let selection = SelectionBase::new(base, ToneOrdering::ChromaProximity);
    let base_luma = rec709_luma_of(base).round();

    // count, R sum, G sum, B sum. A complete 24-bit cube still fits each
    // channel sum in u32: 16_777_216 * 255 < u32::MAX.
    //
    // Each colour is tallied once, under the narrowest rung that admits it;
    // the rungs are nested, so a running sum up the ladder afterwards gives
    // every rung its full histogram without touching each colour once per
    // rung.
    let mut histograms = fold_iso_luma_colors(
        base,
        max_tolerance,
        || {
            tolerances
                .iter()
                .map(|_| vec![[0u32; 4]; PALETTE_SELECTION_BUCKETS])
                .collect::<Vec<_>>()
        },
        |histograms, color, luma| {
            let bucket = selection.bucket(selection.key(color, luma));
            let luma_delta = (luma - base_luma).abs();
            let Some(rung) = tolerances
                .iter()
                .position(|&tolerance| luma_delta <= tolerance as f64 + 0.5)
            else {
                return;
            };
            let totals = &mut histograms[rung][bucket];
            totals[0] += 1;
            totals[1] += color[0] as u32;
            totals[2] += color[1] as u32;
            totals[3] += color[2] as u32;
        },
        |histograms, pieces| {
            for (histogram, piece) in histograms.iter_mut().zip(pieces) {
                for (totals, parts) in histogram.iter_mut().zip(piece) {
                    for (total, part) in totals.iter_mut().zip(parts) {
                        *total += part;
                    }
                }
            }
        },
    );
    for rung in 1..histograms.len() {
        let (narrower, wider) = histograms.split_at_mut(rung);
        for (totals, below) in wider[0].iter_mut().zip(&narrower[rung - 1]) {
            for (total, &part) in totals.iter_mut().zip(below) {
                *total += part;
            }
        }
    }

    struct Selection {
        tolerance: u8,
        cutoff_bucket: usize,
        remaining: usize,
        sums: [u64; 3],
    }

    let mut selections = Vec::new();
    for (index, &tolerance) in tolerances.iter().enumerate() {
        let available: usize = histograms[index]
            .iter()
            .map(|totals| totals[0] as usize)
            .sum();
        if available < needed {
            continue;
        }
        let mut count = 0usize;
        let mut sums = [0u64; 3];
        for (bucket, totals) in histograms[index].iter().enumerate() {
            let bucket_count = totals[0] as usize;
            if count + bucket_count >= needed {
                selections.push(Selection {
                    tolerance,
                    cutoff_bucket: bucket,
                    remaining: needed - count,
                    sums,
                });
                break;
            }
            count += bucket_count;
            for channel in 0..3 {
                sums[channel] += totals[channel + 1] as u64;
            }
        }
    }

    // The colours sitting in each selection's cutoff bucket, from which the
    // last `remaining` are taken in key order.
    let boundaries = fold_iso_luma_colors(
        base,
        max_tolerance,
        || selections.iter().map(|_| Vec::new()).collect::<Vec<_>>(),
        |boundaries, color, luma| {
            let key = selection.key(color, luma);
            let bucket = selection.bucket(key);
            let luma_delta = (luma - base_luma).abs();
            for (candidate, boundary) in selections.iter().zip(boundaries.iter_mut()) {
                if bucket == candidate.cutoff_bucket
                    && luma_delta <= candidate.tolerance as f64 + 0.5
                {
                    boundary.push(KeyedColor { key, color });
                }
            }
        },
        |boundaries, pieces| {
            for (boundary, piece) in boundaries.iter_mut().zip(pieces) {
                boundary.extend(piece);
            }
        },
    );

    selections
        .into_iter()
        .zip(boundaries)
        .map(|(mut selection, mut boundary)| {
            boundary.sort_unstable_by(compare_keyed_colors);
            for entry in boundary.iter().take(selection.remaining) {
                for channel in 0..3 {
                    selection.sums[channel] += entry.color[channel] as u64;
                }
            }
            let count = needed as f64;
            let mean = [
                (selection.sums[0] as f64 / count).round() as u8,
                (selection.sums[1] as f64 / count).round() as u8,
                (selection.sums[2] as f64 / count).round() as u8,
            ];
            (selection.tolerance, mean)
        })
        .collect()
}

/// A palette of colours sharing (within `luma_tolerance` integer steps) the
/// Rec. 709 luma of a base tone, ordered by closeness to that tone. Data is
/// carried as chroma drift: each pixel encodes `bits_per_pixel` bits as an
/// index into the palette, so brightness stays flat while hue/saturation
/// wander only as far as the requested capacity demands.
pub struct TonedPalette {
    config: TonedConfig,
    colors: Vec<[u8; 3]>,
    /// Reverse lookup, built on first use: each colour packed into the high
    /// bits of a `u64` with its palette index in the low bits, sorted, so a
    /// colour is found by bisection. At a million colours this sorts in a
    /// few milliseconds and takes 8 MB, where a hash map took several times
    /// as long to build and more room to hold.
    index_of: OnceLock<Vec<u64>>,
}

/// One reverse-index entry: the colour in the high 24 bits, the palette
/// index below.
#[inline]
fn reverse_index_entry(color: [u8; 3], index: u32) -> u64 {
    (u64::from(color[0]) << 56 | u64::from(color[1]) << 48 | u64::from(color[2]) << 40)
        | u64::from(index)
}

impl TonedPalette {
    /// Every 8-bit RGB colour whose rounded luma is within `luma_tolerance`
    /// of the base tone's, in enumeration order (unsorted).
    fn enumerate_iso_luma(base: [u8; 3], luma_tolerance: u8) -> Vec<[u8; 3]> {
        let mut colors = Vec::new();
        visit_iso_luma_colors(base, luma_tolerance, |color, _| colors.push(color));
        colors
    }

    /// Every 8-bit RGB colour whose rounded luma is within `luma_tolerance`
    /// of the base tone's, sorted by RGB distance to the base tone.
    pub fn candidates(base: [u8; 3], luma_tolerance: u8) -> Vec<[u8; 3]> {
        let mut colors = Self::enumerate_iso_luma(base, luma_tolerance);
        let distance = |c: &[u8; 3]| -> u32 {
            c.iter()
                .zip(base.iter())
                .map(|(&a, &b)| (a as i32 - b as i32).pow(2) as u32)
                .sum()
        };
        colors.sort_by_key(|c| (distance(c), c[0], c[1], c[2]));
        colors
    }

    pub fn new(base: [u8; 3], luma_tolerance: u8, bits_per_pixel: u32) -> Result<Self> {
        Self::from_config(TonedConfig {
            base,
            luma_tolerance,
            bits_per_pixel,
            ordering: ToneOrdering::default(),
        })
    }

    /// Palette tuned by [`TonedConfig::balanced`]: the best chroma/luminance
    /// balance for `base` within the given size budget.
    pub fn balanced(base: [u8; 3], max_size_factor: f64) -> Result<Self> {
        Self::from_config(TonedConfig::balanced(base, max_size_factor)?)
    }

    /// Process-wide cached palette for `config`. Palettes are deterministic
    /// and immutable, so encode, decode and metadata layers share one build
    /// per config instead of recomputing it.
    pub fn shared(config: TonedConfig) -> Result<Arc<Self>> {
        let cache = PALETTE_CACHE
            .get_or_init(|| Mutex::new(BoundedCache::with_capacity(PALETTE_CACHE_CAPACITY)));
        if let Some(palette) = lock(cache).get(&config) {
            return Ok(palette);
        }
        let palette = Arc::new(Self::from_config(config)?);
        lock(cache).insert(config, palette.clone());
        Ok(palette)
    }

    pub fn from_config(config: TonedConfig) -> Result<Self> {
        let TonedConfig {
            base,
            luma_tolerance,
            bits_per_pixel,
            ordering,
        } = config;

        if !(1..=24).contains(&bits_per_pixel) {
            bail!("bits per pixel must be between 1 and 24");
        }

        let colors = Self::select_colors(base, luma_tolerance, bits_per_pixel, ordering)?;

        Ok(Self {
            config,
            colors,
            index_of: OnceLock::new(),
        })
    }

    fn select_colors(
        base: [u8; 3],
        luma_tolerance: u8,
        bits_per_pixel: u32,
        ordering: ToneOrdering,
    ) -> Result<Vec<[u8; 3]>> {
        select_palette_colors(base, luma_tolerance, bits_per_pixel, ordering)
    }

    /// The configuration this palette was built from. Rebuilding from it is
    /// deterministic, so it is all a decoder needs.
    pub fn config(&self) -> TonedConfig {
        self.config
    }

    pub fn base(&self) -> [u8; 3] {
        self.config.base
    }

    pub fn bits_per_pixel(&self) -> u32 {
        self.config.bits_per_pixel
    }

    /// Mean colour of the palette — with uniform (compressed/encrypted) data
    /// this is the tint the rendered image averages to.
    pub fn mean_color(&self) -> [u8; 3] {
        let n = self.colors.len().max(1) as f64;
        let mut sum = [0f64; 3];
        for color in &self.colors {
            for (s, &ch) in sum.iter_mut().zip(color.iter()) {
                *s += ch as f64;
            }
        }
        [
            (sum[0] / n).round() as u8,
            (sum[1] / n).round() as u8,
            (sum[2] / n).round() as u8,
        ]
    }

    /// Palette colour at `index`.
    pub fn color(&self, index: usize) -> [u8; 3] {
        self.colors[index]
    }

    /// Palette index of an exact colour, if present.
    pub fn index_of(&self, color: [u8; 3]) -> Option<u32> {
        Self::lookup(self.reverse_index(), color)
    }

    fn reverse_index(&self) -> &[u64] {
        self.index_of.get_or_init(|| {
            let mut table: Vec<u64> = self
                .colors
                .iter()
                .enumerate()
                .map(|(index, &color)| reverse_index_entry(color, index as u32))
                .collect();
            table.sort_unstable();
            table
        })
    }

    #[inline]
    fn lookup(table: &[u64], color: [u8; 3]) -> Option<u32> {
        let probe = reverse_index_entry(color, 0);
        let entry = *table.get(table.partition_point(|&entry| entry < probe))?;
        (entry & !0xFF_FFFF_FFFF == probe).then_some(entry as u32)
    }

    /// Number of colours in the palette (`2^bits_per_pixel`).
    pub fn len(&self) -> usize {
        self.colors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// RGB distance from the base tone to the farthest palette entry — how
    /// far the chroma is allowed to drift.
    pub fn max_drift(&self) -> f64 {
        self.colors
            .iter()
            .map(|c| {
                c.iter()
                    .zip(self.config.base.iter())
                    .map(|(&a, &b)| (a as f64 - b as f64).powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .fold(0.0, f64::max)
    }

    /// Packs a byte stream into opaque toned pixels, `bits_per_pixel` bits
    /// per pixel. Trailing bits of the final pixel are zero-padded.
    pub fn bytes_to_rgba(&self, bytes: &[u8]) -> Vec<u8> {
        let bpp = self.config.bits_per_pixel;
        let mask = (1u32 << bpp) - 1;
        let pixel_count = (bytes.len() * 8).div_ceil(bpp as usize);
        let mut rgba = Vec::with_capacity(pixel_count * 4);

        let mut acc = 0u32;
        let mut acc_bits = 0u32;
        let emit = |index: u32, rgba: &mut Vec<u8>| {
            let color = self.colors[index as usize];
            rgba.extend_from_slice(&[color[0], color[1], color[2], 255]);
        };

        for &byte in bytes {
            acc = (acc << 8) | byte as u32;
            acc_bits += 8;
            while acc_bits >= bpp {
                emit((acc >> (acc_bits - bpp)) & mask, &mut rgba);
                acc_bits -= bpp;
                acc &= (1 << acc_bits) - 1;
            }
        }

        if acc_bits > 0 {
            emit((acc << (bpp - acc_bits)) & mask, &mut rgba);
        }

        rgba
    }

    /// Recovers the byte stream from toned pixels, skipping fully transparent
    /// pixels. When `byte_length` is given the result is truncated to that
    /// length.
    pub fn rgba_to_bytes(&self, rgba: &[u8], byte_length: Option<usize>) -> Result<Vec<u8>> {
        if rgba.len() % 4 != 0 {
            bail!("RGBA length must be divisible by 4");
        }

        let bpp = self.config.bits_per_pixel;
        let mut bytes = Vec::with_capacity(rgba.len() / 4 * bpp as usize / 8 + 1);
        let mut acc = 0u32;
        let mut acc_bits = 0u32;
        let index_of = self.reverse_index();

        for chunk in rgba.chunks_exact(4) {
            if chunk[3] == 0 {
                continue;
            }

            let Some(index) = Self::lookup(index_of, [chunk[0], chunk[1], chunk[2]]) else {
                bail!(
                    "pixel #{:02X}{:02X}{:02X} is not in the toned palette",
                    chunk[0],
                    chunk[1],
                    chunk[2]
                );
            };

            acc = (acc << bpp) | index;
            acc_bits += bpp;
            while acc_bits >= 8 {
                bytes.push((acc >> (acc_bits - 8)) as u8);
                acc_bits -= 8;
                acc &= (1 << acc_bits) - 1;
            }
        }

        if let Some(byte_length) = byte_length {
            if byte_length > bytes.len() {
                bail!("requested byte length exceeds decoded toned payload");
            }
            bytes.truncate(byte_length);
        }

        Ok(bytes)
    }
}

/// Side length of the smallest square that holds `pixel_count` pixels.
pub fn square_side_for_pixel_count(pixel_count: usize) -> usize {
    let mut side = (pixel_count as f64).sqrt() as usize;
    while side * side < pixel_count {
        side += 1;
    }
    side
}

/// Encodes an RGBA buffer as the smallest square PNG that fits its pixels,
/// padding the remainder of the square with fully transparent pixels (which
/// the byte decoders skip).
pub fn rgba_to_square_png(rgba: &[u8]) -> Result<Vec<u8>> {
    if rgba.len() % 4 != 0 {
        bail!("RGBA length must be divisible by 4");
    }

    let pixel_count = rgba.len() / 4;
    if pixel_count == 0 {
        bail!("cannot encode an empty RGBA buffer as PNG");
    }

    let side = square_side_for_pixel_count(pixel_count);
    let mut padded = rgba.to_vec();
    padded.resize(side * side * 4, 0);

    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, side as u32, side as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&padded)?;
    writer.finish()?;

    Ok(out)
}

/// Decodes a PNG produced by [`rgba_to_square_png`] back into its RGBA buffer
/// (including any transparent padding pixels).
pub fn square_png_to_rgba(png_bytes: &[u8]) -> Result<Vec<u8>> {
    let decoder = png::Decoder::new(png_bytes);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer)?;

    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        bail!("expected an 8-bit RGBA PNG");
    }

    buffer.truncate(info.buffer_size());
    Ok(buffer)
}

/// Normalizes a CSS-style hex colour (with or without `#`, 3 or 6 digits) to
/// uppercase `#RRGGBB`, falling back to white on invalid input.
pub fn normalized_hex_color(value: Option<&str>) -> String {
    let raw = value.unwrap_or("").trim().trim_start_matches('#');
    let expanded = if raw.len() == 3 {
        raw.chars().flat_map(|ch| [ch, ch]).collect::<String>()
    } else {
        raw.to_string()
    };
    let parsed = u32::from_str_radix(&expanded, 16).unwrap_or(0xff_ffff);
    format!(
        "#{:02X}{:02X}{:02X}",
        (parsed >> 16) & 0xff,
        (parsed >> 8) & 0xff,
        parsed & 0xff
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_round_trip_through_rgba() {
        let bytes = [1u8, 2, 3, 4, 5];
        let rgba = bytes_to_rgba(&bytes);
        assert_eq!(rgba.len(), 8);
        assert_eq!(rgba_to_bytes(&rgba, Some(bytes.len())).unwrap(), bytes);
    }

    #[test]
    fn rgba_to_bytes_skips_transparent_pixels() {
        let rgba = [1, 2, 3, 255, 9, 9, 9, 0, 4, 5, 6, 255];
        assert_eq!(rgba_to_bytes(&rgba, None).unwrap(), vec![1, 2, 3, 4, 5, 6]);
        assert!(rgba_to_bytes(&rgba, Some(7)).is_err());
    }

    #[test]
    fn bytes_round_trip_through_grayscale_rgba() {
        let bytes = [0u8, 0x5a, 0xff, 0x12];
        let rgba = bytes_to_grayscale_rgba(&bytes);
        assert_eq!(rgba.len(), bytes.len() * 4);
        assert_eq!(grayscale_rgba_to_bytes(&rgba, None).unwrap(), bytes);
    }

    #[test]
    fn grayscale_rgba_to_bytes_rejects_color_and_skips_transparent() {
        let rgba = [7, 7, 7, 255, 9, 9, 9, 0, 3, 3, 3, 255];
        assert_eq!(grayscale_rgba_to_bytes(&rgba, None).unwrap(), vec![7, 3]);
        assert_eq!(grayscale_rgba_to_bytes(&rgba, Some(1)).unwrap(), vec![7]);
        assert!(grayscale_rgba_to_bytes(&rgba, Some(3)).is_err());
        assert!(grayscale_rgba_to_bytes(&[1, 2, 3, 255], None).is_err());
    }

    #[test]
    fn westside_ecdc_rgb_vs_grayscale_sizes() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../goldenfiles/records/lori-asha-westside-single45-hq/lori-asha-westside-single45-hq.ecdc"
        ))
        .unwrap();

        let rgb = bytes_to_rgba(&bytes);
        let grayscale = bytes_to_grayscale_rgba(&bytes);

        println!("ecdc payload: {} bytes", bytes.len());
        println!(
            "rgb colour:   {} pixels, {} bytes RGBA",
            rgb.len() / 4,
            rgb.len()
        );
        println!(
            "grayscale:    {} pixels, {} bytes RGBA",
            grayscale.len() / 4,
            grayscale.len()
        );

        assert_eq!(rgb.len() / 4, pixel_count_for_byte_length(bytes.len()));
        assert_eq!(grayscale.len() / 4, bytes.len());
        assert_eq!(rgba_to_bytes(&rgb, Some(bytes.len())).unwrap(), bytes);
        assert_eq!(grayscale_rgba_to_bytes(&grayscale, None).unwrap(), bytes);

        let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/fixtures");
        std::fs::create_dir_all(&out_dir).unwrap();

        for (name, rgba) in [("rgb", &rgb), ("grayscale", &grayscale)] {
            let side = square_side_for_pixel_count(rgba.len() / 4);
            let png_bytes = rgba_to_square_png(rgba).unwrap();
            let path = out_dir.join(format!("lori-asha-westside-single45-hq.{name}.png"));
            std::fs::write(&path, &png_bytes).unwrap();
            println!(
                "{name} png:    {side}x{side}, {} bytes -> {}",
                png_bytes.len(),
                path.display()
            );

            let decoded = square_png_to_rgba(&png_bytes).unwrap();
            assert_eq!(
                &decoded[..rgba.len()],
                &rgba[..],
                "{name} png did not round-trip"
            );
            assert!(decoded[rgba.len()..].iter().all(|&b| b == 0));
        }
    }

    const PINK: [u8; 3] = [0xff, 0xc0, 0xcb];

    #[test]
    fn toned_palette_round_trips() {
        let palette = TonedPalette::new(PINK, 0, 8).unwrap();
        let bytes = [0u8, 0x5a, 0xff, 0x12, 0x34];
        let rgba = palette.bytes_to_rgba(&bytes);
        assert_eq!(rgba.len() / 4, 5); // 40 bits at 8 bits/pixel

        for chunk in rgba.chunks_exact(4) {
            let luma = rec709_luma_of([chunk[0], chunk[1], chunk[2]]).round();
            assert_eq!(luma, rec709_luma_of(PINK).round());
        }

        assert_eq!(
            palette.rgba_to_bytes(&rgba, Some(bytes.len())).unwrap(),
            bytes
        );
    }

    #[test]
    #[ignore] // slow: builds large iso-luma palettes; run with --ignored
    fn westside_ecdc_pink_toned_size() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../goldenfiles/records/lori-asha-westside-single45-hq/lori-asha-westside-single45-hq.ecdc"
        ))
        .unwrap();

        // ±16 luma yields ~2.2M iso-tone colours -> 21 bits/pixel (1.14x RGB).
        let tolerance = 16u8;
        let target_bits = 21;
        let palette = TonedPalette::new(PINK, tolerance, target_bits).unwrap();

        println!(
            "selected: ±{tolerance} luma, {} bits/pixel, max chroma drift {:.1} RGB units",
            palette.bits_per_pixel(),
            palette.max_drift()
        );

        let rgba = palette.bytes_to_rgba(&bytes);
        let rgb_pixels = pixel_count_for_byte_length(bytes.len());
        let toned_pixels = rgba.len() / 4;
        println!(
            "rgb colour: {rgb_pixels} pixels, pink toned: {toned_pixels} pixels ({:.2}x)",
            toned_pixels as f64 / rgb_pixels as f64
        );
        assert!(
            toned_pixels * 3 <= rgb_pixels * 4,
            "more than 1/3 larger than RGB"
        );

        let png_bytes = rgba_to_square_png(&rgba).unwrap();
        let side = square_side_for_pixel_count(toned_pixels);
        let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/fixtures");
        std::fs::create_dir_all(&out_dir).unwrap();
        let path = out_dir.join("lori-asha-westside-single45-hq.pink-toned.png");
        std::fs::write(&path, &png_bytes).unwrap();
        println!(
            "pink png:   {side}x{side}, {} bytes -> {}",
            png_bytes.len(),
            path.display()
        );

        let decoded = square_png_to_rgba(&png_bytes).unwrap();
        assert_eq!(
            palette.rgba_to_bytes(&decoded, Some(bytes.len())).unwrap(),
            bytes,
            "pink toned png did not round-trip"
        );
    }

    #[test]
    // Sweeps the full luma-tolerance range building palettes up to ~16M
    // colours; runs for well over a minute, so it is excluded from the
    // default suite. Run explicitly with `--ignored` when needed.
    #[ignore]
    fn pink_iso_luma_capacity_ceiling() {
        for tol in [0u8, 2, 4, 8, 16, 32, 64, 128, 255] {
            let n = TonedPalette::candidates(PINK, tol).len();
            println!(
                "±{tol:>3} luma: {n:>8} colours, max {:.1} bits/pixel, size {:.2}x RGB",
                (n as f64).log2().floor(),
                24.0 / (n as f64).log2().floor()
            );
        }
    }

    /// Deterministic pseudo-random payload, uniform like compressed/encrypted data.
    fn test_payload(len: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 56) as u8
            })
            .collect()
    }

    #[test]
    #[ignore] // slow: builds large iso-luma palettes; run with --ignored
    fn balanced_config_trades_luma_for_chroma() {
        let config = TonedConfig::balanced(PINK, 1.25).unwrap();
        assert_eq!(config.ordering, ToneOrdering::ChromaProximity);
        assert_eq!(config.bits_per_pixel, 20); // ceil(24 / 1.25)
        assert!(
            config.luma_tolerance > 0,
            "flat luma cannot fit 2^20 colours"
        );

        let balanced = TonedPalette::from_config(config).unwrap();
        let unbalanced = TonedPalette::new(PINK, config.luma_tolerance, 21).unwrap();
        let err = |p: &TonedPalette| chroma_distance(p.mean_color(), PINK);
        println!(
            "balanced: ±{} luma, mean {:?} (chroma err {:.1}); 21-bit base-proximity mean {:?} (chroma err {:.1})",
            config.luma_tolerance,
            balanced.mean_color(),
            err(&balanced),
            unbalanced.mean_color(),
            err(&unbalanced)
        );
        assert!(
            err(&balanced) < err(&unbalanced),
            "balanced palette should sit closer to the base tone's chroma"
        );
    }

    #[test]
    #[ignore] // slow: builds large iso-luma palettes; run with --ignored
    fn chroma_ordered_palette_round_trips_exactly() {
        let config = TonedConfig {
            base: PINK,
            luma_tolerance: 16,
            bits_per_pixel: 21,
            ordering: ToneOrdering::ChromaProximity,
        };
        let palette = TonedPalette::from_config(config).unwrap();
        let bytes = test_payload(4093); // prime length -> partial final pixel

        let rgba = palette.bytes_to_rgba(&bytes);
        assert_eq!(
            palette.rgba_to_bytes(&rgba, Some(bytes.len())).unwrap(),
            bytes
        );
    }

    #[test]
    #[ignore] // slow: builds large iso-luma palettes; run with --ignored
    fn balanced_palette_rebuilt_from_config_decodes_exactly() {
        let bytes = test_payload(10_007);
        let encoder = TonedPalette::balanced(PINK, 1.25).unwrap();
        let rgba = encoder.bytes_to_rgba(&bytes);

        // The decode side only knows the config; rebuilding must yield the
        // identical palette and recover the byte stream exactly.
        let decoder = TonedPalette::from_config(encoder.config()).unwrap();
        assert_eq!(
            decoder.rgba_to_bytes(&rgba, Some(bytes.len())).unwrap(),
            bytes
        );

        // And through PNG encode/decode, including transparent padding.
        let png_bytes = rgba_to_square_png(&rgba).unwrap();
        let decoded_rgba = square_png_to_rgba(&png_bytes).unwrap();
        assert_eq!(
            decoder
                .rgba_to_bytes(&decoded_rgba, Some(bytes.len()))
                .unwrap(),
            bytes
        );
    }

    #[test]
    #[ignore] // slow: builds large iso-luma palettes; run with --ignored
    fn balanced_palettes_round_trip_across_bases_and_budgets() {
        for base in [
            PINK,
            [0x20, 0x60, 0xc0],
            [0x10, 0x10, 0x10],
            [0xf0, 0xf0, 0xf0],
        ] {
            for max_size_factor in [1.2, 1.5, 3.0] {
                let palette = TonedPalette::balanced(base, max_size_factor).unwrap();
                assert!(24.0 / palette.bits_per_pixel() as f64 <= max_size_factor + 1e-9);

                let bytes = test_payload(257);
                let rgba = palette.bytes_to_rgba(&bytes);
                assert_eq!(
                    palette.rgba_to_bytes(&rgba, Some(bytes.len())).unwrap(),
                    bytes,
                    "round trip failed for base {base:?} at {max_size_factor}x"
                );
            }
        }
    }

    #[test]
    #[ignore] // slow: builds large iso-luma palettes; run with --ignored
    fn westside_ecdc_balanced_pink() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../goldenfiles/records/lori-asha-westside-single45-hq/lori-asha-westside-single45-hq.ecdc"
        ))
        .unwrap();

        let palette = TonedPalette::balanced(PINK, 1.25).unwrap();
        let config = palette.config();
        println!(
            "balanced: ±{} luma, {} bits/pixel, mean colour {:?}, max drift {:.1}",
            config.luma_tolerance,
            config.bits_per_pixel,
            palette.mean_color(),
            palette.max_drift()
        );

        let rgba = palette.bytes_to_rgba(&bytes);
        let png_bytes = rgba_to_square_png(&rgba).unwrap();
        let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/fixtures");
        std::fs::create_dir_all(&out_dir).unwrap();
        let path = out_dir.join("lori-asha-westside-single45-hq.pink-balanced.png");
        std::fs::write(&path, &png_bytes).unwrap();
        let side = square_side_for_pixel_count(rgba.len() / 4);
        println!(
            "balanced png: {side}x{side}, {} bytes -> {}",
            png_bytes.len(),
            path.display()
        );

        let decoded = square_png_to_rgba(&png_bytes).unwrap();
        let decoder = TonedPalette::from_config(config).unwrap();
        assert_eq!(
            decoder.rgba_to_bytes(&decoded, Some(bytes.len())).unwrap(),
            bytes,
            "balanced pink png did not round-trip"
        );
    }

    #[test]
    fn hex_colors_normalize() {
        assert_eq!(normalized_hex_color(Some("#abc")), "#AABBCC");
        assert_eq!(normalized_hex_color(Some("112233")), "#112233");
        assert_eq!(normalized_hex_color(None), "#FFFFFF");
        assert_eq!(normalized_hex_color(Some("nope")), "#FFFFFF");
    }
}
