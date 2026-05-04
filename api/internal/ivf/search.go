package ivf

import (
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

type SearchStats struct {
	ScannedClusters uint32
	ScannedVectors  uint32
}

// Search returns the number of fraud labels in the top-5 nearest neighbors
// of qFloat against the index. Range: 0..5. Used by the API to pick the
// pre-built response: score = frauds*0.2, approved = frauds<3.
func (idx *Index) Search(qFloat *[D]float32, opts SearchOpts) int {
	return kernel.Search(idx.DimsBuf, idx.N, idx.Centroids, idx.BBoxMin, idx.BBoxMax, idx.Offsets[:], qFloat, normalizeNProbe(opts.NProbe), idx.Labels, idx.OrigIDs)
}

func (idx *Index) SearchWithStats(qFloat *[D]float32, opts SearchOpts) (int, SearchStats) {
	frauds, clusters, vectors := kernel.SearchStats(idx.DimsBuf, idx.N, idx.Centroids, idx.BBoxMin, idx.BBoxMax, idx.Offsets[:], qFloat, normalizeNProbe(opts.NProbe), idx.Labels, idx.OrigIDs)
	return frauds, SearchStats{ScannedClusters: clusters, ScannedVectors: vectors}
}

func normalizeNProbe(nprobe int) int {
	if nprobe < 1 {
		return 1
	}
	if nprobe > K {
		return K
	}
	return nprobe
}
