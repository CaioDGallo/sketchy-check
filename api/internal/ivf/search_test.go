package ivf

import "testing"

func TestSearchNProbe1DoesNotAllocate(t *testing.T) {
	idx := newBenchmarkIndex(8)
	q := benchmarkQuery()

	allocs := testing.AllocsPerRun(100, func() {
		_ = idx.Search(&q, SearchOpts{NProbe: 1})
	})
	if allocs != 0 {
		t.Fatalf("Search NProbe=1 allocated: got %.2f allocs/op, want 0", allocs)
	}
}

func BenchmarkSearch_NProbe1(b *testing.B) {
	idx := newBenchmarkIndex(64)
	q := benchmarkQuery()

	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = idx.Search(&q, SearchOpts{NProbe: 1})
	}
}

func newBenchmarkIndex(perCluster int) *Index {
	n := K * perCluster
	idx := &Index{N: n}
	idx.DimsBuf = make([]int16, D*n)
	for j := 0; j < D; j++ {
		idx.Dim[j] = idx.DimsBuf[j*n : (j+1)*n]
	}
	idx.Labels = make([]uint8, n)
	idx.OrigIDs = make([]uint32, n)
	idx.Centroids = make([]float32, K*D)
	idx.BBoxMin = make([]int16, K*D)
	idx.BBoxMax = make([]int16, K*D)

	for c := 0; c < K; c++ {
		start := c * perCluster
		end := start + perCluster
		center := benchmarkCenter(c)
		idx.ClusterStart[c] = start
		idx.ClusterEnd[c] = end

		for j := 0; j < D; j++ {
			idx.Centroids[c*D+j] = float32(center) / FixScale
			idx.BBoxMin[c*D+j] = clampI16(int32(center) - 10)
			idx.BBoxMax[c*D+j] = clampI16(int32(center) + 10)
		}

		for i := start; i < end; i++ {
			idx.OrigIDs[i] = uint32(i)
			idx.Labels[i] = uint8(i & 1)
			for j := 0; j < D; j++ {
				idx.Dim[j][i] = clampI16(int32(center) + int32((i+j)%21) - 10)
			}
		}
	}

	return idx
}

func benchmarkQuery() [D]float32 {
	return [D]float32{0.01, 0.02, 0.03, 0.04, 0.05, -1, -1, 0.06, 0.07, 1, 0, 1, 0.5, 0.08}
}

func benchmarkCenter(c int) int16 {
	return int16(-10000 + (20000*c)/(K-1))
}

func clampI16(v int32) int16 {
	if v < -10000 {
		return -10000
	}
	if v > 10000 {
		return 10000
	}
	return int16(v)
}
