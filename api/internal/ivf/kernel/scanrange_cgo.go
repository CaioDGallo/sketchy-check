//go:build cgo && amd64

package kernel

/*
#cgo CFLAGS: -O3 -march=haswell -mtune=haswell -mavx2 -fomit-frame-pointer -ffast-math -DNDEBUG
#include "kernel_amd64.h"
*/
import "C"

import "unsafe"

// ScanRange scans dims_buf[start:end] (in cluster-sorted column-major layout)
// against the quantized query q, updating top in place.
//
// Preconditions:
//
//	len(dimsBuf)  == 14 * nPerDim
//	len(labels)   == nPerDim
//	len(ids)      == nPerDim
//	0 <= start <= end <= nPerDim
//
// AVX2 implementation; build tag scopes this file to cgo+amd64 only.
func ScanRange(
	dimsBuf []int16, nPerDim, start, end int,
	q [14]int16,
	labels []uint8, ids []uint32,
	top *Top5,
) {
	if start >= end {
		return
	}
	C.sketchy_scan_range(
		(*C.int16_t)(unsafe.Pointer(&dimsBuf[0])),
		C.int(nPerDim),
		C.int(start), C.int(end),
		(*C.int16_t)(unsafe.Pointer(&q[0])),
		(*C.uint8_t)(unsafe.Pointer(&labels[0])),
		(*C.uint32_t)(unsafe.Pointer(&ids[0])),
		(*C.sketchy_top5_t)(unsafe.Pointer(top)),
	)
}
