package ivf

import (
	"math"
	"sort"

	"github.com/rinha2026/sketchy/api/internal/ivf/kernel"
)

// SearchOpts holds the runtime knobs for IVF search.
//
// NProbe controls how many of the 256 clusters are scanned in the initial
// pass — picked from the closest centroids. The reference solution uses 1.
// The bounding-box repair pass after the initial scan ensures the top-5 is
// still exact (or matches the brute-force grader's tie-break exactly).
type SearchOpts struct {
	NProbe int // 1..256; default 1
}

// Search returns the number of fraud labels in the top-5 nearest neighbors
// of qFloat against the index. Range: 0..5. Used by the API to pick the
// pre-built response: score = frauds*0.2, approved = frauds<3.
func (idx *Index) Search(qFloat *[D]float32, opts SearchOpts) int {
	nprobe := opts.NProbe
	if nprobe < 1 {
		nprobe = 1
	}
	if nprobe > K {
		nprobe = K
	}

	// Quantize the query and derive a "grid-space" float copy that matches
	// the quantization rounding exactly. Using qGrid for centroid distance
	// (instead of qFloat directly) avoids subtle drift between the cluster-pick
	// step and the vector-scan step.
	var qInt [D]int16
	var qGrid [D]float32
	for j := 0; j < D; j++ {
		qInt[j] = quantize(qFloat[j])
		qGrid[j] = float32(qInt[j]) / FixScale
	}

	// Centroid distance for all 256, then partial-sort to keep the top NPROBE.
	type cluster struct {
		idx     int
		penalty float32
	}
	probes := make([]cluster, K)
	for c := 0; c < K; c++ {
		probes[c] = cluster{c, centroidSqDist(qGrid[:], idx.Centroids[c*D:(c+1)*D])}
	}
	sort.Slice(probes, func(i, j int) bool { return probes[i].penalty < probes[j].penalty })

	var top kernel.Top5
	top.Reset()

	scanned := [K]bool{}
	for p := 0; p < nprobe; p++ {
		c := probes[p].idx
		start, end := idx.ClusterStart[c], idx.ClusterEnd[c]
		if end <= start {
			scanned[c] = true
			continue
		}
		kernel.ScanRange(idx.DimsBuf, idx.N, start, end, qInt, idx.Labels, idx.OrigIDs, &top)
		scanned[c] = true
	}

	// Bounding-box repair: sweep the remaining clusters and scan any whose
	// bbox lower-bound to the query is <= current worst. This is a strict
	// safety net — if the top-5 is already correct, the bbox check rejects
	// every cluster cheaply (14 ops). When a cluster's bbox could still
	// improve the top-5, we go in and scan it.
	for c := 0; c < K; c++ {
		if scanned[c] {
			continue
		}
		start, end := idx.ClusterStart[c], idx.ClusterEnd[c]
		if end <= start {
			continue
		}
		if bboxLowerBound(qInt[:], idx.BBoxMin[c*D:(c+1)*D], idx.BBoxMax[c*D:(c+1)*D]) <= top.WorstD {
			kernel.ScanRange(idx.DimsBuf, idx.N, start, end, qInt, idx.Labels, idx.OrigIDs, &top)
		}
	}

	return top.Frauds()
}

// centroidSqDist computes squared Euclidean distance between query and a
// centroid, both in float-grid space. 14 dims, scalar — cheap, called 256x
// per query, total ~3.6K float ops.
func centroidSqDist(q, centroid []float32) float32 {
	var s float32
	for j := 0; j < D; j++ {
		d := q[j] - centroid[j]
		s += d * d
	}
	return s
}

// bboxLowerBound is the minimum possible squared distance from a query
// (quantized int16) to any vector inside a cluster, given the cluster's
// per-dim min/max bounding box. If the query value is inside [mn, mx] for
// dim j, that dim contributes 0 to the lower bound; otherwise it contributes
// the squared distance to the nearest face of the box on that dim.
func bboxLowerBound(q []int16, mn, mx []int16) uint64 {
	var s uint64
	for j := 0; j < D; j++ {
		var d int32
		if q[j] < mn[j] {
			d = int32(mn[j]) - int32(q[j])
		} else if q[j] > mx[j] {
			d = int32(q[j]) - int32(mx[j])
		}
		s += uint64(int64(d) * int64(d))
	}
	return s
}

// quantize mirrors the build-time quantization in cmd/preprocess: maps
// [-1, 1] floats to int16 [-10000, 10000] using nearest-int rounding.
// Sentinel -1.0 (no last_transaction) becomes -10000 — naturally distant
// from real [0,1] values, naturally close to other -10000 sentinels.
func quantize(x float32) int16 {
	if x < -1 {
		x = -1
	} else if x > 1 {
		x = 1
	}
	scaled := x * FixScale
	if scaled >= 0 {
		scaled += 0.5
	} else {
		scaled -= 0.5
	}
	if scaled < -FixScale {
		scaled = -FixScale
	} else if scaled > FixScale {
		scaled = FixScale
	}
	if scaled > math.MaxInt16 {
		scaled = math.MaxInt16
	} else if scaled < math.MinInt16 {
		scaled = math.MinInt16
	}
	return int16(scaled)
}
