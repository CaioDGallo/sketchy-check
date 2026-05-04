//go:build !cgo || !amd64

package kernel

import "math"

const ivfClusters = 256

// ScanRange — pure-Go scalar fallback. Used on macOS ARM dev machines or
// any build without cgo.
//
// Behavior is bit-identical to the C scalar fallback in kernel_amd64.c:
// same dim ordering, same early-exit pruning, same tie-break.
func ScanRange(
	dimsBuf []int16, nPerDim, start, end int,
	q [14]int16,
	labels []uint8, ids []uint32,
	top *Top5,
) {
	if start >= end {
		return
	}
	d0 := dimsBuf[0*nPerDim : 1*nPerDim]
	d1 := dimsBuf[1*nPerDim : 2*nPerDim]
	d2 := dimsBuf[2*nPerDim : 3*nPerDim]
	d3 := dimsBuf[3*nPerDim : 4*nPerDim]
	d4 := dimsBuf[4*nPerDim : 5*nPerDim]
	d5 := dimsBuf[5*nPerDim : 6*nPerDim]
	d6 := dimsBuf[6*nPerDim : 7*nPerDim]
	d7 := dimsBuf[7*nPerDim : 8*nPerDim]
	d8 := dimsBuf[8*nPerDim : 9*nPerDim]
	d9 := dimsBuf[9*nPerDim : 10*nPerDim]
	d10 := dimsBuf[10*nPerDim : 11*nPerDim]
	d11 := dimsBuf[11*nPerDim : 12*nPerDim]
	d12 := dimsBuf[12*nPerDim : 13*nPerDim]
	d13 := dimsBuf[13*nPerDim : 14*nPerDim]

	q0, q1, q2, q3 := q[0], q[1], q[2], q[3]
	q4, q5, q6, q7 := q[4], q[5], q[6], q[7]
	q8, q9, q10, q11 := q[8], q[9], q[10], q[11]
	q12, q13 := q[12], q[13]

	for i := start; i < end; i++ {
		var dist uint64
		dist += sq(q5, d5[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q6, d6[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q2, d2[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q0, d0[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q7, d7[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q8, d8[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q11, d11[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q12, d12[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q9, d9[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q10, d10[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q1, d1[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q13, d13[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q3, d3[i])
		if dist > top.WorstD {
			continue
		}
		dist += sq(q4, d4[i])

		oid := ids[i]
		if betterPair(dist, oid, top.WorstD, top.WorstID) {
			tryInsert(dist, labels[i], oid, top)
		}
	}
}

func sq(a, b int16) uint64 {
	d := int32(a) - int32(b)
	return uint64(int64(d) * int64(d))
}

func betterPair(da uint64, ia uint32, db uint64, ib uint32) bool {
	return da < db || (da == db && ia < ib)
}

func worsePair(da uint64, ia uint32, db uint64, ib uint32) bool {
	return da > db || (da == db && ia > ib)
}

func findWorst5(d *[5]uint64, id *[5]uint32) int {
	w := 0
	if worsePair(d[1], id[1], d[w], id[w]) {
		w = 1
	}
	if worsePair(d[2], id[2], d[w], id[w]) {
		w = 2
	}
	if worsePair(d[3], id[3], d[w], id[w]) {
		w = 3
	}
	if worsePair(d[4], id[4], d[w], id[w]) {
		w = 4
	}
	return w
}

func tryInsert(d uint64, label uint8, origID uint32, t *Top5) {
	if !betterPair(d, origID, t.WorstD, t.WorstID) {
		return
	}
	w := t.Worst
	t.BestD[w] = d
	t.BestL[w] = label
	t.BestID[w] = origID
	nw := findWorst5(&t.BestD, &t.BestID)
	t.Worst = int32(nw)
	t.WorstD = t.BestD[nw]
	t.WorstID = t.BestID[nw]
}

func Search(
	dimsBuf []int16, nPerDim int,
	centroids []float32,
	bboxMin, bboxMax []int16,
	offsets []uint32,
	qFloat *[14]float32,
	nprobe int,
	labels []uint8, ids []uint32,
) int {
	frauds, _, _ := SearchStats(dimsBuf, nPerDim, centroids, bboxMin, bboxMax, offsets, qFloat, nprobe, labels, ids)
	return frauds
}

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
	if nprobe < 1 {
		nprobe = 1
	}
	clusters := len(offsets) - 1
	if clusters > ivfClusters {
		clusters = ivfClusters
	}
	if nprobe > clusters {
		nprobe = clusters
	}

	var qInt [D]int16
	var qGrid [D]float32
	for j := 0; j < D; j++ {
		qInt[j] = quantize(qFloat[j])
		qGrid[j] = float32(qInt[j]) / 10000.0
	}

	var centroidDist [ivfClusters]float32
	for c := 0; c < clusters; c++ {
		centroidDist[c] = centroidSqDist(qGrid[:], centroids[c*D:(c+1)*D])
	}

	var top Top5
	top.Reset()
	scanned := [ivfClusters]bool{}
	for p := 0; p < nprobe; p++ {
		c := closestUnscannedCluster(&centroidDist, &scanned, clusters)
		if c < 0 {
			break
		}
		start, end := int(offsets[c]), int(offsets[c+1])
		if end <= start {
			scanned[c] = true
			continue
		}
		ScanRange(dimsBuf, nPerDim, start, end, qInt, labels, ids, &top)
		scanned[c] = true
		scannedClusters++
		scannedVectors += uint32(end - start)
	}

	for c := 0; c < clusters; c++ {
		if scanned[c] {
			continue
		}
		start, end := int(offsets[c]), int(offsets[c+1])
		if end <= start {
			continue
		}
		if bboxLowerBound(qInt[:], bboxMin[c*D:(c+1)*D], bboxMax[c*D:(c+1)*D]) <= top.WorstD {
			ScanRange(dimsBuf, nPerDim, start, end, qInt, labels, ids, &top)
			scannedClusters++
			scannedVectors += uint32(end - start)
		}
	}
	return top.Frauds(), scannedClusters, scannedVectors
}

func closestUnscannedCluster(centroidDist *[ivfClusters]float32, scanned *[ivfClusters]bool, clusters int) int {
	best := -1
	bestDist := float32(math.MaxFloat32)
	for c := 0; c < clusters; c++ {
		if scanned[c] {
			continue
		}
		if centroidDist[c] < bestDist {
			best = c
			bestDist = centroidDist[c]
		}
	}
	return best
}

func centroidSqDist(q, centroid []float32) float32 {
	var s float32
	for j := 0; j < D; j++ {
		d := q[j] - centroid[j]
		s += d * d
	}
	return s
}

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

func quantize(x float32) int16 {
	if x < -1 {
		x = -1
	} else if x > 1 {
		x = 1
	}
	scaled := x * 10000.0
	if scaled >= 0 {
		scaled += 0.5
	} else {
		scaled -= 0.5
	}
	if scaled < -10000.0 {
		scaled = -10000.0
	} else if scaled > 10000.0 {
		scaled = 10000.0
	}
	return int16(scaled)
}
