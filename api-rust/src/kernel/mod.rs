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
    // The production Dockerfile pins -C target-cpu=haswell and -C target-feature=+avx2,
    // so on x86_64 builds AVX2 is statically guaranteed and the runtime detect is dead.
    // Gating with cfg(target_feature) lets LLVM eliminate the branch and inline the
    // AVX2 routine directly into the dispatcher.
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    {
        // SAFETY: target_feature = "avx2" is enabled at compile time.
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
    }
    #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
    {
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

#[cfg(all(test, target_arch = "x86_64", target_feature = "avx2"))]
mod parity_tests {
    use super::scalar::scan_range_scalar;
    use super::avx2::scan_range_avx2;
    use super::top5::Top5;

    // Build a deterministic synthetic dataset and confirm AVX2 and scalar
    // produce bit-identical Top5 state (best_d, best_l, best_id, worst_*).
    // The contest demands byte-identical output, so this catches any future
    // optimization that drifts the AVX2 path off the scalar oracle.
    fn synth(n: usize, seed: u32) -> (Vec<i16>, Vec<u8>, Vec<u32>) {
        let mut dims = vec![0i16; 14 * n];
        let mut labels = vec![0u8; n];
        let mut ids = vec![0u32; n];
        let mut s = seed;
        let mut rng = || {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            s
        };
        for j in 0..14 {
            for i in 0..n {
                let v = (rng() as i32 & 0x3FFF) - 0x2000;
                dims[j * n + i] = v as i16;
            }
        }
        for i in 0..n {
            labels[i] = (rng() & 1) as u8;
            ids[i] = rng();
        }
        (dims, labels, ids)
    }

    #[test]
    fn avx2_matches_scalar_full_range() {
        let n = 200;
        let (dims, labels, ids) = synth(n, 0xC0FFEE);
        let q: [i16; 14] = [100, -50, 200, 0, -1000, 500, 250, -333, 42, 7, -77, 999, -1, 0];

        let mut t_av = Top5::new();
        let mut t_sc = Top5::new();
        unsafe {
            scan_range_avx2(&dims, n, 0, n, &q, &labels, &ids, &mut t_av);
        }
        scan_range_scalar(&dims, n, 0, n, &q, &labels, &ids, &mut t_sc);
        assert_eq!(t_av.best_d, t_sc.best_d, "best_d mismatch");
        assert_eq!(t_av.best_id, t_sc.best_id, "best_id mismatch");
        assert_eq!(t_av.best_l, t_sc.best_l, "best_l mismatch");
        assert_eq!(t_av.worst_d, t_sc.worst_d, "worst_d mismatch");
        assert_eq!(t_av.worst_id, t_sc.worst_id, "worst_id mismatch");
    }

    #[test]
    fn avx2_matches_scalar_unaligned_tail() {
        // n not a multiple of 8 — exercises the scalar tail call inside AVX2.
        let n = 67;
        let (dims, labels, ids) = synth(n, 0xBEEF);
        let q: [i16; 14] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

        let mut t_av = Top5::new();
        let mut t_sc = Top5::new();
        unsafe {
            scan_range_avx2(&dims, n, 0, n, &q, &labels, &ids, &mut t_av);
        }
        scan_range_scalar(&dims, n, 0, n, &q, &labels, &ids, &mut t_sc);
        assert_eq!(t_av.best_d, t_sc.best_d);
        assert_eq!(t_av.best_id, t_sc.best_id);
        assert_eq!(t_av.best_l, t_sc.best_l);
    }

    #[test]
    fn avx2_matches_scalar_partial_range() {
        // start > 0 — exercises the start-offset path inside the AVX2 loop.
        let n = 256;
        let (dims, labels, ids) = synth(n, 0xDEAD);
        let q: [i16; 14] = [-500, 500, 0, 0, 1000, -1000, 0, 0, 0, 0, 0, 0, 0, 0];

        let mut t_av = Top5::new();
        let mut t_sc = Top5::new();
        unsafe {
            scan_range_avx2(&dims, n, 13, 137, &q, &labels, &ids, &mut t_av);
        }
        scan_range_scalar(&dims, n, 13, 137, &q, &labels, &ids, &mut t_sc);
        assert_eq!(t_av.best_d, t_sc.best_d);
        assert_eq!(t_av.best_id, t_sc.best_id);
        assert_eq!(t_av.best_l, t_sc.best_l);
    }
}
