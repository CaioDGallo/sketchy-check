// Package kmeans runs k-means clustering for the preprocess pipeline.
// Used to partition the 14-dim reference vectors into IVF cells.
package kmeans

import (
	"fmt"
	"math"
)

// Train runs deterministic k-means on a stride-D vector buffer and returns
// the K*D centroid array (row-major, K rows of D floats each).
//
// vectors: flat float32 buffer of length N*D in row-major order.
// k:       number of clusters (e.g. 256).
// d:       vector dimensionality (14).
// sampleN: how many vectors to use for the iterative update step. Must be <= N.
//          The full N is used at the end to assign each vector to its cluster
//          via the caller; this only affects centroid quality.
// iters:   k-means iterations (Lloyd's algorithm).
//
// Initialization is deterministic (evenly-spaced indices into the sample),
// matching the reference C implementation.
func Train(vectors []float32, n, k, d, sampleN, iters int) []float32 {
	if sampleN > n {
		sampleN = n
	}
	if sampleN < k {
		sampleN = k
	}

	sample := make([]int, sampleN)
	for i := 0; i < sampleN; i++ {
		sample[i] = int(uint64(i) * uint64(n) / uint64(sampleN))
	}

	centroids := make([]float32, k*d)
	for c := 0; c < k; c++ {
		si := int(uint64(c) * uint64(sampleN) / uint64(k))
		if si >= sampleN {
			si = sampleN - 1
		}
		copy(centroids[c*d:(c+1)*d], vectors[sample[si]*d:(sample[si]+1)*d])
	}

	sums := make([]float32, k*d)
	counts := make([]int, k)
	for it := 0; it < iters; it++ {
		for i := range sums {
			sums[i] = 0
		}
		for i := range counts {
			counts[i] = 0
		}
		for _, idx := range sample {
			c := nearestCentroid(vectors[idx*d:(idx+1)*d], centroids, k, d)
			vec := vectors[idx*d : (idx+1)*d]
			sum := sums[c*d : (c+1)*d]
			for j := 0; j < d; j++ {
				sum[j] += vec[j]
			}
			counts[c]++
		}
		empty := 0
		for c := 0; c < k; c++ {
			if counts[c] == 0 {
				empty++
				continue
			}
			inv := float32(1) / float32(counts[c])
			cc := centroids[c*d : (c+1)*d]
			sum := sums[c*d : (c+1)*d]
			for j := 0; j < d; j++ {
				cc[j] = sum[j] * inv
			}
		}
		fmt.Printf("kmeans iter %d/%d sample=%d empty=%d\n", it+1, iters, sampleN, empty)
	}
	return centroids
}

// Assign returns the index of the centroid nearest to vec.
func Assign(vec, centroids []float32, k, d int) int {
	return nearestCentroid(vec, centroids, k, d)
}

func nearestCentroid(vec, centroids []float32, k, d int) int {
	best := 0
	bestD := float32(math.MaxFloat32)
	for c := 0; c < k; c++ {
		cc := centroids[c*d : (c+1)*d]
		var dist float32
		for j := 0; j < d; j++ {
			diff := vec[j] - cc[j]
			dist += diff * diff
		}
		if dist < bestD {
			bestD = dist
			best = c
		}
	}
	return best
}
