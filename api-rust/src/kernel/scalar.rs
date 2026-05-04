// Scalar fallback for the IVF inner loop. Used on non-x86 hosts (macOS dev
// on Apple Silicon) and as a correctness oracle for the AVX2 path.
//
// Dimension order (5,6,2,0,7,8,11,12,9,10,1,13,3,4) matches the Go and C
// implementations — required for byte-identical top-5 results.

use super::top5::{is_better, Top5};

pub fn scan_range_scalar(
    dims_buf: &[i16],
    n_per_dim: usize,
    start: usize,
    end: usize,
    q: &[i16; 14],
    labels: &[u8],
    ids: &[u32],
    t: &mut Top5,
) {
    let d0 = &dims_buf[0 * n_per_dim..1 * n_per_dim];
    let d1 = &dims_buf[1 * n_per_dim..2 * n_per_dim];
    let d2 = &dims_buf[2 * n_per_dim..3 * n_per_dim];
    let d3 = &dims_buf[3 * n_per_dim..4 * n_per_dim];
    let d4 = &dims_buf[4 * n_per_dim..5 * n_per_dim];
    let d5 = &dims_buf[5 * n_per_dim..6 * n_per_dim];
    let d6 = &dims_buf[6 * n_per_dim..7 * n_per_dim];
    let d7 = &dims_buf[7 * n_per_dim..8 * n_per_dim];
    let d8 = &dims_buf[8 * n_per_dim..9 * n_per_dim];
    let d9 = &dims_buf[9 * n_per_dim..10 * n_per_dim];
    let d10 = &dims_buf[10 * n_per_dim..11 * n_per_dim];
    let d11 = &dims_buf[11 * n_per_dim..12 * n_per_dim];
    let d12 = &dims_buf[12 * n_per_dim..13 * n_per_dim];
    let d13 = &dims_buf[13 * n_per_dim..14 * n_per_dim];

    let q0 = q[0];
    let q1 = q[1];
    let q2 = q[2];
    let q3 = q[3];
    let q4 = q[4];
    let q5 = q[5];
    let q6 = q[6];
    let q7 = q[7];
    let q8 = q[8];
    let q9 = q[9];
    let q10 = q[10];
    let q11 = q[11];
    let q12 = q[12];
    let q13 = q[13];

    for i in start..end {
        let mut dist: u64 = 0;
        dist += sqd(q5, d5[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q6, d6[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q2, d2[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q0, d0[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q7, d7[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q8, d8[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q11, d11[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q12, d12[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q9, d9[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q10, d10[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q1, d1[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q13, d13[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q3, d3[i]);
        if dist > t.worst_d {
            continue;
        }
        dist += sqd(q4, d4[i]);
        let oid = ids[i];
        if is_better(dist, oid, t.worst_d, t.worst_id) {
            t.try_insert(dist, labels[i], oid);
        }
    }
}

#[inline]
fn sqd(a: i16, b: i16) -> u64 {
    let d = a as i32 - b as i32;
    (d as i64 * d as i64) as u64
}
