// AVX2 SIMD scan loop. 1:1 port of scan_range_avx2 in
// api/internal/ivf/kernel/kernel_amd64.c. Loads 8 int16 lanes per dim,
// widens to int32, broadcasts q[j], computes (v - q)^2, accumulates into
// two int64 vectors. Same dim order as scalar — bit-identical results.

#![cfg(target_arch = "x86_64")]

use std::arch::x86_64::*;

use super::scalar::scan_range_scalar;
use super::top5::{is_better, Top5};

#[target_feature(enable = "avx2")]
pub unsafe fn scan_range_avx2(
    dims_buf: &[i16],
    n_per_dim: usize,
    start: usize,
    end: usize,
    q: &[i16; 14],
    labels: &[u8],
    ids: &[u32],
    t: &mut Top5,
) {
    let d_ptr = dims_buf.as_ptr();
    let d0 = d_ptr.add(0 * n_per_dim);
    let d1 = d_ptr.add(1 * n_per_dim);
    let d2 = d_ptr.add(2 * n_per_dim);
    let d3 = d_ptr.add(3 * n_per_dim);
    let d4 = d_ptr.add(4 * n_per_dim);
    let d5 = d_ptr.add(5 * n_per_dim);
    let d6 = d_ptr.add(6 * n_per_dim);
    let d7 = d_ptr.add(7 * n_per_dim);
    let d8 = d_ptr.add(8 * n_per_dim);
    let d9 = d_ptr.add(9 * n_per_dim);
    let d10 = d_ptr.add(10 * n_per_dim);
    let d11 = d_ptr.add(11 * n_per_dim);
    let d12 = d_ptr.add(12 * n_per_dim);
    let d13 = d_ptr.add(13 * n_per_dim);

    let q0 = _mm256_set1_epi32(q[0] as i32);
    let q1 = _mm256_set1_epi32(q[1] as i32);
    let q2 = _mm256_set1_epi32(q[2] as i32);
    let q3 = _mm256_set1_epi32(q[3] as i32);
    let q4 = _mm256_set1_epi32(q[4] as i32);
    let q5 = _mm256_set1_epi32(q[5] as i32);
    let q6 = _mm256_set1_epi32(q[6] as i32);
    let q7 = _mm256_set1_epi32(q[7] as i32);
    let q8 = _mm256_set1_epi32(q[8] as i32);
    let q9 = _mm256_set1_epi32(q[9] as i32);
    let q10 = _mm256_set1_epi32(q[10] as i32);
    let q11 = _mm256_set1_epi32(q[11] as i32);
    let q12 = _mm256_set1_epi32(q[12] as i32);
    let q13 = _mm256_set1_epi32(q[13] as i32);

    let mut tmp_lo = [0u64; 4];
    let mut tmp_hi = [0u64; 4];

    // Prefetch distance, in i16 elements ahead of the current load.
    // 32 i16 = 1 cache line (64 B). We step by 8 per iter, so every fourth
    // iteration crosses into a fresh line — issuing one PREFETCHT0 per dim
    // per iter over-prefetches but keeps the L1 fill ahead of demand for
    // every dim stream (Haswell's HW prefetcher only tracks ~8 streams; we
    // have 14, so software prefetch covers the gap).
    const PREFETCH_AHEAD: usize = 32;

    let mut i = start;
    let limit = end - ((end - start) & 7);
    while i < limit {
        let pi = i + PREFETCH_AHEAD;
        _mm_prefetch(d5.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d6.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d2.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d0.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d7.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d8.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d11.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d12.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d9.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d10.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d1.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d13.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d3.add(pi) as *const i8, _MM_HINT_T0);
        _mm_prefetch(d4.add(pi) as *const i8, _MM_HINT_T0);

        let mut lo = _mm256_setzero_si256();
        let mut hi = _mm256_setzero_si256();
        // Selectivity-friendly order matching scalar: 5,6,2,0,7,8,11,12,9,10,1,13,3,4.
        acc_dim(&mut lo, &mut hi, q5, d5, i);
        acc_dim(&mut lo, &mut hi, q6, d6, i);
        acc_dim(&mut lo, &mut hi, q2, d2, i);
        acc_dim(&mut lo, &mut hi, q0, d0, i);
        acc_dim(&mut lo, &mut hi, q7, d7, i);
        acc_dim(&mut lo, &mut hi, q8, d8, i);
        acc_dim(&mut lo, &mut hi, q11, d11, i);
        acc_dim(&mut lo, &mut hi, q12, d12, i);
        acc_dim(&mut lo, &mut hi, q9, d9, i);
        acc_dim(&mut lo, &mut hi, q10, d10, i);
        acc_dim(&mut lo, &mut hi, q1, d1, i);
        acc_dim(&mut lo, &mut hi, q13, d13, i);
        acc_dim(&mut lo, &mut hi, q3, d3, i);
        acc_dim(&mut lo, &mut hi, q4, d4, i);

        _mm256_storeu_si256(tmp_lo.as_mut_ptr() as *mut __m256i, lo);
        _mm256_storeu_si256(tmp_hi.as_mut_ptr() as *mut __m256i, hi);

        for lane in 0..4 {
            let dd = tmp_lo[lane];
            let oid = *ids.get_unchecked(i + lane);
            if is_better(dd, oid, t.worst_d, t.worst_id) {
                t.try_insert(dd, *labels.get_unchecked(i + lane), oid);
            }
        }
        for lane in 0..4 {
            let dd = tmp_hi[lane];
            let oid = *ids.get_unchecked(i + 4 + lane);
            if is_better(dd, oid, t.worst_d, t.worst_id) {
                t.try_insert(dd, *labels.get_unchecked(i + 4 + lane), oid);
            }
        }
        i += 8;
    }
    if i < end {
        scan_range_scalar(dims_buf, n_per_dim, i, end, q, labels, ids, t);
    }
}

#[target_feature(enable = "avx2")]
#[inline]
unsafe fn acc_dim(lo64: &mut __m256i, hi64: &mut __m256i, q32: __m256i, ptr: *const i16, i: usize) {
    let raw = _mm_loadu_si128(ptr.add(i) as *const __m128i);
    let v32 = _mm256_cvtepi16_epi32(raw);
    let diff = _mm256_sub_epi32(v32, q32);
    let sq32 = _mm256_mullo_epi32(diff, diff);
    let lo128 = _mm256_castsi256_si128(sq32);
    let hi128 = _mm256_extracti128_si256(sq32, 1);
    *lo64 = _mm256_add_epi64(*lo64, _mm256_cvtepi32_epi64(lo128));
    *hi64 = _mm256_add_epi64(*hi64, _mm256_cvtepi32_epi64(hi128));
}
