#ifndef SKETCHY_KERNEL_H
#define SKETCHY_KERNEL_H

#include <stddef.h>
#include <stdint.h>

/*
 * sketchy_top5_t mirrors the Go struct kernel.Top5 byte-for-byte.
 * Total size: 88 bytes, alignment 8.
 *
 * Update rule (tie-break): candidate (d, id) beats (d', id') iff
 *   d < d'  OR  (d == d' AND id < id').
 * This matches the deterministic ordering used by the Rinha grader.
 */
typedef struct {
    uint64_t best_d[5];   /* offset 0,  size 40 */
    uint32_t best_id[5];  /* offset 40, size 20 */
    uint8_t  best_l[5];   /* offset 60, size 5  */
    uint8_t  _pad[3];     /* offset 65, size 3  */
    int32_t  worst;       /* offset 68, size 4  */
    uint64_t worst_d;     /* offset 72, size 8  */
    uint32_t worst_id;    /* offset 80, size 4  */
    uint32_t _pad2;       /* offset 84, size 4  */
} sketchy_top5_t;

/*
 * sketchy_scan_range scans vectors [start, end) in cluster-sorted order from
 * a column-major int16 buffer dims_buf, where dims_buf[j*n_per_dim + i] is
 * the j-th dim of vector i.
 *
 * q[14] is the quantized int16 query vector.
 * labels[i] in {0,1}: 1 = fraud.
 * ids[i] is the original record index in references.json.gz (for tie-break).
 *
 * top5 is updated in place.
 */
void sketchy_scan_range(
    const int16_t *dims_buf,
    int n_per_dim,
    int start,
    int end,
    const int16_t q[14],
    const uint8_t *labels,
    const uint32_t *ids,
    sketchy_top5_t *top5);

#endif
