// IVF search orchestrator. Mirrors sketchy_search_frauds in
// api/internal/ivf/kernel/kernel_amd64.c — quantize the query, pick the
// NPROBE closest centroids, scan those clusters, then bbox-repair any
// remaining cluster whose lower-bound distance still beats top-5.
//
// On x86_64 the AVX2 inner loop is used; otherwise the scalar fallback.
// Both paths produce bit-identical top-5 results — required for the grader's
// brute-force-k=5 detection score.

mod top5;
mod scalar;

#[cfg(target_arch = "x86_64")]
mod avx2;

pub use top5::Top5;

use crate::index::{Index, D, FIX_SCALE, K};

const IVF_CLUSTERS: usize = K;

#[derive(Default, Clone, Copy)]
pub struct SearchStats {
    pub scanned_clusters: u32,
    pub scanned_vectors: u32,
}

/// Returns the number of fraud labels in the top-5 nearest neighbors of `q_float`.
/// Range: 0..=5.
pub fn search_frauds(idx: &Index, q_float: &[f32; D], nprobe: u32) -> i32 {
    let mut t = Top5::new();
    let stats = search_into(idx, q_float, nprobe, &mut t);
    let _ = stats;
    t.frauds()
}

pub fn search_with_stats(idx: &Index, q_float: &[f32; D], nprobe: u32) -> (i32, SearchStats) {
    let mut t = Top5::new();
    let stats = search_into(idx, q_float, nprobe, &mut t);
    (t.frauds(), stats)
}

fn search_into(idx: &Index, q_float: &[f32; D], nprobe: u32, t: &mut Top5) -> SearchStats {
    let nprobe = nprobe.clamp(1, IVF_CLUSTERS as u32) as usize;

    // Quantize once: int16 for the int kernel, float-grid for centroid distance.
    let mut q = [0i16; D];
    let mut q_grid = [0f32; D];
    for j in 0..D {
        q[j] = quantize_fixed(q_float[j]);
        q_grid[j] = q[j] as f32 / FIX_SCALE;
    }

    // Pick the `nprobe` closest centroids.
    let mut best_c = [-1i32; IVF_CLUSTERS];
    let mut best_p = [f32::MAX; IVF_CLUSTERS];
    for c in 0..IVF_CLUSTERS {
        let dist = centroid_sqdist(&q_grid, &idx.centroids, c);
        insert_probe(c as i32, dist, &mut best_c, &mut best_p, nprobe);
    }

    let mut scanned_cluster = [false; IVF_CLUSTERS];
    let mut clusters: u32 = 0;
    let mut vectors: u32 = 0;

    for pi in 0..nprobe {
        let c = best_c[pi];
        if c < 0 {
            continue;
        }
        let cu = c as usize;
        let start = idx.offsets[cu] as i32;
        let end = idx.offsets[cu + 1] as i32;
        if end <= start {
            continue;
        }
        scanned_cluster[cu] = true;
        scan_range(idx, start, end, &q, t);
        clusters += 1;
        vectors += (end - start) as u32;
    }

    for c in 0..IVF_CLUSTERS {
        if scanned_cluster[c] {
            continue;
        }
        let start = idx.offsets[c] as i32;
        let end = idx.offsets[c + 1] as i32;
        if end <= start {
            continue;
        }
        if bbox_lower_bound(&q, &idx.bbox_min, &idx.bbox_max, c) <= t.worst_d {
            scan_range(idx, start, end, &q, t);
            clusters += 1;
            vectors += (end - start) as u32;
        }
    }

    SearchStats {
        scanned_clusters: clusters,
        scanned_vectors: vectors,
    }
}

#[inline]
fn scan_range(idx: &Index, start: i32, end: i32, q: &[i16; D], t: &mut Top5) {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 detected at runtime; function is gated to AVX2 ISA.
            unsafe {
                avx2::scan_range_avx2(
                    &idx.dims_buf,
                    idx.n,
                    start as usize,
                    end as usize,
                    q,
                    &idx.labels,
                    &idx.orig_ids,
                    t,
                );
            }
            return;
        }
    }
    scalar::scan_range_scalar(
        &idx.dims_buf,
        idx.n,
        start as usize,
        end as usize,
        q,
        &idx.labels,
        &idx.orig_ids,
        t,
    );
}

#[inline]
fn quantize_fixed(x: f32) -> i16 {
    let mut x = x;
    if x < -1.0 {
        x = -1.0;
    }
    if x > 1.0 {
        x = 1.0;
    }
    let mut scaled = x * FIX_SCALE;
    scaled += if scaled >= 0.0 { 0.5 } else { -0.5 };
    if scaled < -FIX_SCALE {
        scaled = -FIX_SCALE;
    }
    if scaled > FIX_SCALE {
        scaled = FIX_SCALE;
    }
    scaled as i16
}

#[inline]
fn centroid_sqdist(q: &[f32; D], centroids: &[f32], c: usize) -> f32 {
    let base = c * D;
    let mut s = 0.0f32;
    for j in 0..D {
        let d = q[j] - centroids[base + j];
        s += d * d;
    }
    s
}

#[inline]
fn insert_probe(
    cluster: i32,
    penalty: f32,
    best_c: &mut [i32; IVF_CLUSTERS],
    best_p: &mut [f32; IVF_CLUSTERS],
    nprobe: usize,
) {
    if penalty >= best_p[nprobe - 1] {
        return;
    }
    let mut pos = nprobe - 1;
    while pos > 0 && penalty < best_p[pos - 1] {
        pos -= 1;
    }
    let mut i = nprobe - 1;
    while i > pos {
        best_p[i] = best_p[i - 1];
        best_c[i] = best_c[i - 1];
        i -= 1;
    }
    best_p[pos] = penalty;
    best_c[pos] = cluster;
}

#[inline]
fn bbox_lower_bound(q: &[i16; D], bbox_min: &[i16], bbox_max: &[i16], c: usize) -> u64 {
    let base = c * D;
    let mut s: u64 = 0;
    for j in 0..D {
        let d: i32 = if q[j] < bbox_min[base + j] {
            bbox_min[base + j] as i32 - q[j] as i32
        } else if q[j] > bbox_max[base + j] {
            q[j] as i32 - bbox_max[base + j] as i32
        } else {
            0
        };
        s += (d as i64 * d as i64) as u64;
    }
    s
}
