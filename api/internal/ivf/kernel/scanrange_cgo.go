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

// Search runs the complete IVF request search in C: query quantization,
// centroid selection, bbox repair, AVX2/scalar scans, and deterministic top-5.
func Search(
	dimsBuf []int16, nPerDim int,
	centroids []float32,
	bboxMin, bboxMax []int16,
	offsets []uint32,
	qFloat *[14]float32,
	nprobe int,
	labels []uint8, ids []uint32,
) int {
	if nPerDim == 0 {
		return 0
	}
	return int(C.sketchy_search_frauds(
		(*C.int16_t)(unsafe.Pointer(&dimsBuf[0])),
		C.int(nPerDim),
		(*C.float)(unsafe.Pointer(&centroids[0])),
		(*C.int16_t)(unsafe.Pointer(&bboxMin[0])),
		(*C.int16_t)(unsafe.Pointer(&bboxMax[0])),
		(*C.uint32_t)(unsafe.Pointer(&offsets[0])),
		(*C.float)(unsafe.Pointer(&qFloat[0])),
		C.int(nprobe),
		(*C.uint8_t)(unsafe.Pointer(&labels[0])),
		(*C.uint32_t)(unsafe.Pointer(&ids[0])),
		(*C.uint32_t)(nil),
		(*C.uint32_t)(nil),
	))
}

// SearchStats is the profiling variant of Search. It pays the same single C
// crossing but asks the C path to expose scanned cluster/vector counts.
func SearchStats(
	dimsBuf []int16, nPerDim int,
	centroids []float32,
	bboxMin, bboxMax []int16,
	offsets []uint32,
	qFloat *[14]float32,
	nprobe int,
	labels []uint8, ids []uint32,
) (frauds int, scannedClusters, scannedVectors uint32) {
	if nPerDim == 0 {
		return 0, 0, 0
	}
	var clusters C.uint32_t
	var vectors C.uint32_t
	frauds = int(C.sketchy_search_frauds(
		(*C.int16_t)(unsafe.Pointer(&dimsBuf[0])),
		C.int(nPerDim),
		(*C.float)(unsafe.Pointer(&centroids[0])),
		(*C.int16_t)(unsafe.Pointer(&bboxMin[0])),
		(*C.int16_t)(unsafe.Pointer(&bboxMax[0])),
		(*C.uint32_t)(unsafe.Pointer(&offsets[0])),
		(*C.float)(unsafe.Pointer(&qFloat[0])),
		C.int(nprobe),
		(*C.uint8_t)(unsafe.Pointer(&labels[0])),
		(*C.uint32_t)(unsafe.Pointer(&ids[0])),
		&clusters,
		&vectors,
	))
	return frauds, uint32(clusters), uint32(vectors)
}
