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

func TestSearchMatchesBruteForce(t *testing.T) {
	idx := newBenchmarkIndex(17)
	queries := [][D]float32{
		benchmarkQuery(),
		{0.10, 0.20, 0.30, 0.40, 0.50, -1, -1, 0.60, 0.70, 1, 0, 1, 0.5, 0.80},
		{1, 1, 1, 1, 1, 0.2, 0.3, 1, 1, 0, 1, 0, 0.85, 1},
	}

	for _, q := range queries {
		got := idx.Search(&q, SearchOpts{NProbe: 1})
		want := bruteForceFrauds(idx, &q)
		if got != want {
			t.Fatalf("Search()=%d, brute=%d for q=%v", got, want, q)
		}
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
		idx.Offsets[c] = uint32(start)
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
	idx.Offsets[K] = uint32(n)

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

func bruteForceFrauds(idx *Index, q *[D]float32) int {
	var qInt [D]int16
	for j := 0; j < D; j++ {
		qInt[j] = testQuantize(q[j])
	}
	bestD := [5]uint64{^uint64(0), ^uint64(0), ^uint64(0), ^uint64(0), ^uint64(0)}
	bestID := [5]uint32{^uint32(0), ^uint32(0), ^uint32(0), ^uint32(0), ^uint32(0)}
	var bestL [5]uint8
	worst := 0
	worstD := bestD[0]
	worstID := bestID[0]

	for i := 0; i < idx.N; i++ {
		var dist uint64
		for j := 0; j < D; j++ {
			d := int32(qInt[j]) - int32(idx.Dim[j][i])
			dist += uint64(int64(d) * int64(d))
		}
		oid := idx.OrigIDs[i]
		if dist < worstD || (dist == worstD && oid < worstID) {
			bestD[worst] = dist
			bestID[worst] = oid
			bestL[worst] = idx.Labels[i]
			worst = bruteWorst(bestD, bestID)
			worstD = bestD[worst]
			worstID = bestID[worst]
		}
	}
	frauds := 0
	for _, label := range bestL {
		if label == 1 {
			frauds++
		}
	}
	return frauds
}

func bruteWorst(d [5]uint64, id [5]uint32) int {
	w := 0
	for i := 1; i < 5; i++ {
		if d[i] > d[w] || (d[i] == d[w] && id[i] > id[w]) {
			w = i
		}
	}
	return w
}

func testQuantize(x float32) int16 {
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
	return int16(scaled)
}
