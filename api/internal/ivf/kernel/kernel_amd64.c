/*
 * sketchy_scan_range — IVF cluster scan with deterministic top-5 update.
 *
 * Two implementations live here:
 *
 *   - AVX2 (active when __AVX2__ is defined; we compile with -march=haswell -mavx2):
 *     loads 8 int16 lanes per dim, broadcasts q[j] as int32, computes
 *     (v - q)^2 per lane in int32, accumulates into two int64 vectors.
 *     One full 14-dim distance per ~14 iterations of trivial SIMD ops.
 *
 *   - Scalar fallback: per-vector loop with early exit when the partial
 *     distance already exceeds the current worst — typically prunes 70+%
 *     of records cheaply.
 *
 * Both paths share the same top-5 update rule and bit-for-bit produce the
 * same output for the same input.
 */

#include "kernel_amd64.h"

#include <float.h>
#include <string.h>

#ifdef __AVX2__
#include <immintrin.h>
#endif

#define SKETCHY_DIM 14
#define SKETCHY_IVF_CLUSTERS 256
#define SKETCHY_FIX_SCALE 10000.0f

static inline int is_better_pair(uint64_t da, uint32_t ia, uint64_t db, uint32_t ib) {
    return da < db || (da == db && ia < ib);
}

static inline int is_worse_pair(uint64_t da, uint32_t ia, uint64_t db, uint32_t ib) {
    return da > db || (da == db && ia > ib);
}

static inline int find_worst5(const uint64_t d[5], const uint32_t id[5]) {
    int w = 0;
    if (is_worse_pair(d[1], id[1], d[w], id[w])) w = 1;
    if (is_worse_pair(d[2], id[2], d[w], id[w])) w = 2;
    if (is_worse_pair(d[3], id[3], d[w], id[w])) w = 3;
    if (is_worse_pair(d[4], id[4], d[w], id[w])) w = 4;
    return w;
}

static inline void try_insert(uint64_t d, uint8_t label, uint32_t orig_id, sketchy_top5_t *t) {
    if (!is_better_pair(d, orig_id, t->worst_d, t->worst_id)) return;
    int w = t->worst;
    t->best_d[w] = d;
    t->best_l[w] = label;
    t->best_id[w] = orig_id;
    int nw = find_worst5(t->best_d, t->best_id);
    t->worst = nw;
    t->worst_d = t->best_d[nw];
    t->worst_id = t->best_id[nw];
}

static inline uint64_t sqdiff_i16(int16_t a, int16_t b) {
    int32_t d = (int32_t)a - (int32_t)b;
    return (uint64_t)((int64_t)d * (int64_t)d);
}

static inline int16_t quantize_fixed(float x) {
    if (x < -1.0f) x = -1.0f;
    if (x > 1.0f) x = 1.0f;

    float scaled = x * SKETCHY_FIX_SCALE;
    scaled += scaled >= 0.0f ? 0.5f : -0.5f;

    if (scaled < -SKETCHY_FIX_SCALE) scaled = -SKETCHY_FIX_SCALE;
    if (scaled > SKETCHY_FIX_SCALE) scaled = SKETCHY_FIX_SCALE;

    return (int16_t)scaled;
}

static inline void top5_reset(sketchy_top5_t *t) {
    for (int i = 0; i < 5; i++) {
        t->best_d[i] = UINT64_MAX;
        t->best_id[i] = UINT32_MAX;
        t->best_l[i] = 0;
    }
    t->worst = 0;
    t->worst_d = UINT64_MAX;
    t->worst_id = UINT32_MAX;
}

static inline int top5_frauds(const sketchy_top5_t *t) {
    return (t->best_l[0] == 1) + (t->best_l[1] == 1) + (t->best_l[2] == 1) +
           (t->best_l[3] == 1) + (t->best_l[4] == 1);
}

static inline float centroid_sqdist(const float q[SKETCHY_DIM], const float *centroids, int c) {
    const float *cent = centroids + (size_t)c * SKETCHY_DIM;
    float s = 0.0f;
    for (int j = 0; j < SKETCHY_DIM; j++) {
        float d = q[j] - cent[j];
        s += d * d;
    }
    return s;
}

static inline void insert_probe_cluster(int cluster, float penalty, int *best_c, float *best_p, int nprobe) {
    if (penalty >= best_p[nprobe - 1]) return;
    int pos = nprobe - 1;
    while (pos > 0 && penalty < best_p[pos - 1]) pos--;
    for (int i = nprobe - 1; i > pos; i--) {
        best_p[i] = best_p[i - 1];
        best_c[i] = best_c[i - 1];
    }
    best_p[pos] = penalty;
    best_c[pos] = cluster;
}

static inline uint64_t bbox_lower_bound(const int16_t q[SKETCHY_DIM], const int16_t *bbox_min, const int16_t *bbox_max, int c) {
    const int16_t *mn = bbox_min + (size_t)c * SKETCHY_DIM;
    const int16_t *mx = bbox_max + (size_t)c * SKETCHY_DIM;
    uint64_t s = 0;
    for (int j = 0; j < SKETCHY_DIM; j++) {
        int32_t d = 0;
        if (q[j] < mn[j]) d = (int32_t)mn[j] - (int32_t)q[j];
        else if (q[j] > mx[j]) d = (int32_t)q[j] - (int32_t)mx[j];
        s += (uint64_t)((int64_t)d * (int64_t)d);
    }
    return s;
}

static void scan_range_scalar(
    const int16_t *dims_buf, int n_per_dim,
    int start, int end,
    const int16_t q[14],
    const uint8_t *labels,
    const uint32_t *ids,
    sketchy_top5_t *t)
{
    /* Per-dim slice pointers — one row each. */
    const int16_t *d0  = dims_buf + 0  * n_per_dim;
    const int16_t *d1  = dims_buf + 1  * n_per_dim;
    const int16_t *d2  = dims_buf + 2  * n_per_dim;
    const int16_t *d3  = dims_buf + 3  * n_per_dim;
    const int16_t *d4  = dims_buf + 4  * n_per_dim;
    const int16_t *d5  = dims_buf + 5  * n_per_dim;
    const int16_t *d6  = dims_buf + 6  * n_per_dim;
    const int16_t *d7  = dims_buf + 7  * n_per_dim;
    const int16_t *d8  = dims_buf + 8  * n_per_dim;
    const int16_t *d9  = dims_buf + 9  * n_per_dim;
    const int16_t *d10 = dims_buf + 10 * n_per_dim;
    const int16_t *d11 = dims_buf + 11 * n_per_dim;
    const int16_t *d12 = dims_buf + 12 * n_per_dim;
    const int16_t *d13 = dims_buf + 13 * n_per_dim;

    const int16_t q0=q[0], q1=q[1], q2=q[2], q3=q[3], q4=q[4], q5=q[5], q6=q[6];
    const int16_t q7=q[7], q8=q[8], q9=q[9], q10=q[10], q11=q[11], q12=q[12], q13=q[13];

    /* Order dims roughly by selectivity — sentinel-bearing dims (5, 6) first
     * tend to discriminate fastest because they're either equal (-10000 == -10000)
     * or maximally distant. Hot-pruning order matches the reference.
     */
    for (int i = start; i < end; i++) {
        uint64_t dist = 0;
        dist += sqdiff_i16(q5,  d5[i]);  if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q6,  d6[i]);  if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q2,  d2[i]);  if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q0,  d0[i]);  if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q7,  d7[i]);  if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q8,  d8[i]);  if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q11, d11[i]); if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q12, d12[i]); if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q9,  d9[i]);  if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q10, d10[i]); if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q1,  d1[i]);  if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q13, d13[i]); if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q3,  d3[i]);  if (dist > t->worst_d) continue;
        dist += sqdiff_i16(q4,  d4[i]);
        uint32_t oid = ids[i];
        if (is_better_pair(dist, oid, t->worst_d, t->worst_id)) {
            try_insert(dist, labels[i], oid, t);
        }
    }
}

#ifdef __AVX2__
/*
 * Accumulate squared diff for one dim into two int64 lanes.
 * lo64 holds lanes 0..3, hi64 holds lanes 4..7 of the int32 squared diffs
 * widened to int64 so accumulation cannot overflow.
 */
static inline void acc_dim(__m256i *lo64, __m256i *hi64, __m256i q32, const int16_t *ptr, int i) {
    __m128i raw   = _mm_loadu_si128((const __m128i *)(ptr + i)); /* 8 × int16 */
    __m256i v32   = _mm256_cvtepi16_epi32(raw);                  /* 8 × int32 */
    __m256i diff  = _mm256_sub_epi32(v32, q32);
    __m256i sq32  = _mm256_mullo_epi32(diff, diff);              /* 8 × int32 squared */
    __m128i lo128 = _mm256_castsi256_si128(sq32);
    __m128i hi128 = _mm256_extracti128_si256(sq32, 1);
    *lo64 = _mm256_add_epi64(*lo64, _mm256_cvtepi32_epi64(lo128));
    *hi64 = _mm256_add_epi64(*hi64, _mm256_cvtepi32_epi64(hi128));
}

static void scan_range_avx2(
    const int16_t *dims_buf, int n_per_dim,
    int start, int end,
    const int16_t q[14],
    const uint8_t *labels,
    const uint32_t *ids,
    sketchy_top5_t *t)
{
    const int16_t *d0  = dims_buf + 0  * n_per_dim;
    const int16_t *d1  = dims_buf + 1  * n_per_dim;
    const int16_t *d2  = dims_buf + 2  * n_per_dim;
    const int16_t *d3  = dims_buf + 3  * n_per_dim;
    const int16_t *d4  = dims_buf + 4  * n_per_dim;
    const int16_t *d5  = dims_buf + 5  * n_per_dim;
    const int16_t *d6  = dims_buf + 6  * n_per_dim;
    const int16_t *d7  = dims_buf + 7  * n_per_dim;
    const int16_t *d8  = dims_buf + 8  * n_per_dim;
    const int16_t *d9  = dims_buf + 9  * n_per_dim;
    const int16_t *d10 = dims_buf + 10 * n_per_dim;
    const int16_t *d11 = dims_buf + 11 * n_per_dim;
    const int16_t *d12 = dims_buf + 12 * n_per_dim;
    const int16_t *d13 = dims_buf + 13 * n_per_dim;

    const __m256i Q0  = _mm256_set1_epi32((int)q[0]);
    const __m256i Q1  = _mm256_set1_epi32((int)q[1]);
    const __m256i Q2  = _mm256_set1_epi32((int)q[2]);
    const __m256i Q3  = _mm256_set1_epi32((int)q[3]);
    const __m256i Q4  = _mm256_set1_epi32((int)q[4]);
    const __m256i Q5  = _mm256_set1_epi32((int)q[5]);
    const __m256i Q6  = _mm256_set1_epi32((int)q[6]);
    const __m256i Q7  = _mm256_set1_epi32((int)q[7]);
    const __m256i Q8  = _mm256_set1_epi32((int)q[8]);
    const __m256i Q9  = _mm256_set1_epi32((int)q[9]);
    const __m256i Q10 = _mm256_set1_epi32((int)q[10]);
    const __m256i Q11 = _mm256_set1_epi32((int)q[11]);
    const __m256i Q12 = _mm256_set1_epi32((int)q[12]);
    const __m256i Q13 = _mm256_set1_epi32((int)q[13]);

    uint64_t tmp_lo[4] __attribute__((aligned(32)));
    uint64_t tmp_hi[4] __attribute__((aligned(32)));

    int i = start;
    int limit = end - ((end - start) & 7); /* round to multiple of 8 */
    for (; i < limit; i += 8) {
        __m256i lo = _mm256_setzero_si256();
        __m256i hi = _mm256_setzero_si256();
        /* Same selectivity-friendly order as the scalar path. AVX2 cannot early-exit
         * mid-vector but the order still matches so the result is bit-identical.
         */
        acc_dim(&lo, &hi, Q5,  d5,  i);
        acc_dim(&lo, &hi, Q6,  d6,  i);
        acc_dim(&lo, &hi, Q2,  d2,  i);
        acc_dim(&lo, &hi, Q0,  d0,  i);
        acc_dim(&lo, &hi, Q7,  d7,  i);
        acc_dim(&lo, &hi, Q8,  d8,  i);
        acc_dim(&lo, &hi, Q11, d11, i);
        acc_dim(&lo, &hi, Q12, d12, i);
        acc_dim(&lo, &hi, Q9,  d9,  i);
        acc_dim(&lo, &hi, Q10, d10, i);
        acc_dim(&lo, &hi, Q1,  d1,  i);
        acc_dim(&lo, &hi, Q13, d13, i);
        acc_dim(&lo, &hi, Q3,  d3,  i);
        acc_dim(&lo, &hi, Q4,  d4,  i);

        _mm256_store_si256((__m256i *)tmp_lo, lo);
        _mm256_store_si256((__m256i *)tmp_hi, hi);
        for (int lane = 0; lane < 4; lane++) {
            uint64_t dd = tmp_lo[lane];
            uint32_t oid = ids[i + lane];
            if (is_better_pair(dd, oid, t->worst_d, t->worst_id)) {
                try_insert(dd, labels[i + lane], oid, t);
            }
        }
        for (int lane = 0; lane < 4; lane++) {
            uint64_t dd = tmp_hi[lane];
            uint32_t oid = ids[i + 4 + lane];
            if (is_better_pair(dd, oid, t->worst_d, t->worst_id)) {
                try_insert(dd, labels[i + 4 + lane], oid, t);
            }
        }
    }
    if (i < end) {
        scan_range_scalar(dims_buf, n_per_dim, i, end, q, labels, ids, t);
    }
}
#endif /* __AVX2__ */

void sketchy_scan_range(
    const int16_t *dims_buf,
    int n_per_dim,
    int start,
    int end,
    const int16_t q[14],
    const uint8_t *labels,
    const uint32_t *ids,
    sketchy_top5_t *t)
{
#ifdef __AVX2__
    scan_range_avx2(dims_buf, n_per_dim, start, end, q, labels, ids, t);
#else
    scan_range_scalar(dims_buf, n_per_dim, start, end, q, labels, ids, t);
#endif
}

int sketchy_search_frauds(
    const int16_t *dims_buf,
    int n_per_dim,
    const float *centroids,
    const int16_t *bbox_min,
    const int16_t *bbox_max,
    const uint32_t *offsets,
    const float q_float[14],
    int nprobe,
    const uint8_t *labels,
    const uint32_t *ids,
    uint32_t *scanned_clusters,
    uint32_t *scanned_vectors)
{
    if (nprobe < 1) nprobe = 1;
    if (nprobe > SKETCHY_IVF_CLUSTERS) nprobe = SKETCHY_IVF_CLUSTERS;

    int16_t q[SKETCHY_DIM];
    float q_grid[SKETCHY_DIM];
    for (int j = 0; j < SKETCHY_DIM; j++) {
        q[j] = quantize_fixed(q_float[j]);
        q_grid[j] = (float)q[j] / SKETCHY_FIX_SCALE;
    }

    int best_c[SKETCHY_IVF_CLUSTERS];
    float best_p[SKETCHY_IVF_CLUSTERS];
    for (int i = 0; i < nprobe; i++) {
        best_c[i] = -1;
        best_p[i] = FLT_MAX;
    }
    for (int c = 0; c < SKETCHY_IVF_CLUSTERS; c++) {
        insert_probe_cluster(c, centroid_sqdist(q_grid, centroids, c), best_c, best_p, nprobe);
    }

    sketchy_top5_t top;
    top5_reset(&top);

    uint8_t scanned_cluster[SKETCHY_IVF_CLUSTERS];
    memset(scanned_cluster, 0, sizeof(scanned_cluster));
    uint32_t clusters = 0;
    uint32_t vectors = 0;

    for (int pi = 0; pi < nprobe; pi++) {
        int c = best_c[pi];
        if (c < 0) continue;
        int start = (int)offsets[c];
        int end = (int)offsets[c + 1];
        if (end <= start) continue;
        scanned_cluster[c] = 1;
        sketchy_scan_range(dims_buf, n_per_dim, start, end, q, labels, ids, &top);
        clusters++;
        vectors += (uint32_t)(end - start);
    }

    for (int c = 0; c < SKETCHY_IVF_CLUSTERS; c++) {
        if (scanned_cluster[c]) continue;
        int start = (int)offsets[c];
        int end = (int)offsets[c + 1];
        if (end <= start) continue;
        if (bbox_lower_bound(q, bbox_min, bbox_max, c) <= top.worst_d) {
            sketchy_scan_range(dims_buf, n_per_dim, start, end, q, labels, ids, &top);
            clusters++;
            vectors += (uint32_t)(end - start);
        }
    }

    if (scanned_clusters) *scanned_clusters = clusters;
    if (scanned_vectors) *scanned_vectors = vectors;
    return top5_frauds(&top);
}
