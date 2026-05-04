// Package ivf loads the IVF6 index file produced by cmd/preprocess and serves
// it to search.go. On load, the row-major int16 vectors are transposed into 14
// column-major slices (Dim[0..13]) so the AVX2 inner loop can iterate one
// dimension at a time across many vectors.
package ivf

import (
	"bufio"
	"encoding/binary"
	"fmt"
	"io"
	"math"
	"os"
)

const (
	D        = 14         // vector dimension
	K        = 256        // IVF cluster count
	FixScale = 10000.0    // quantization scale: float [-1,1] → int16 [-10000,10000]
	magic    = "IVF6"
)

// Index holds the in-memory IVF6 index. All slices are read-only after Load returns.
type Index struct {
	N int

	// DimsBuf is one contiguous int16 buffer of length D*N; access pattern
	// is DimsBuf[j*N + i] = vector i's j-th dim. Dim[j] is a re-slice into
	// the same backing storage so the AVX2 kernel can be passed a single pointer.
	DimsBuf []int16

	// Dim[j] holds N int16 quantized values for dimension j across all vectors,
	// sorted by cluster (so cluster c's records live in Dim[j][cluster_start[c]:cluster_end[c]]).
	// Dim[j] points into DimsBuf — do not mutate.
	Dim [D][]int16

	// Labels[i] is 1 for fraud, 0 for legit, in cluster-sorted order.
	Labels []uint8

	// OrigIDs[i] is the position of record i in references.json.gz, used for
	// deterministic tie-breaks in the top-5.
	OrigIDs []uint32

	// Centroids[c*D + j] is centroid c's j-th dim in float-grid space (== quantized/FixScale).
	Centroids []float32

	// BBoxMin/Max[c*D + j] is the per-cluster, per-dim min/max in quantized int16 space.
	BBoxMin []int16
	BBoxMax []int16

	// ClusterStart/End[c] are inclusive/exclusive offsets into Dim[j], Labels, OrigIDs.
	ClusterStart [K]int
	ClusterEnd   [K]int
}

// Load reads an IVF6 file from path and returns the populated Index.
// Returns an error on bad magic, mismatched K/D/scale, or short reads.
func Load(path string) (*Index, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	br := bufio.NewReaderSize(f, 1<<20)

	var hdr [4]byte
	if _, err := io.ReadFull(br, hdr[:]); err != nil {
		return nil, fmt.Errorf("read magic: %w", err)
	}
	if string(hdr[:]) != magic {
		return nil, fmt.Errorf("bad magic: got %q, want %q", hdr[:], magic)
	}

	var nU32, kU32, dU32, strideU32 uint32
	var scale float32
	if err := binary.Read(br, binary.LittleEndian, &nU32); err != nil {
		return nil, err
	}
	if err := binary.Read(br, binary.LittleEndian, &kU32); err != nil {
		return nil, err
	}
	if err := binary.Read(br, binary.LittleEndian, &dU32); err != nil {
		return nil, err
	}
	if err := binary.Read(br, binary.LittleEndian, &strideU32); err != nil {
		return nil, err
	}
	if err := binary.Read(br, binary.LittleEndian, &scale); err != nil {
		return nil, err
	}
	if kU32 != K || dU32 != D || strideU32 != D {
		return nil, fmt.Errorf("incompatible index: K=%d D=%d stride=%d (expected K=%d D=%d)", kU32, dU32, strideU32, K, D)
	}
	if math.Abs(float64(scale-FixScale)) > 0.01 {
		return nil, fmt.Errorf("incompatible scale: got %f, want %f", scale, FixScale)
	}

	idx := &Index{N: int(nU32)}
	idx.Centroids = make([]float32, K*D)
	idx.BBoxMin = make([]int16, K*D)
	idx.BBoxMax = make([]int16, K*D)
	if err := binary.Read(br, binary.LittleEndian, idx.Centroids); err != nil {
		return nil, fmt.Errorf("read centroids: %w", err)
	}
	if err := binary.Read(br, binary.LittleEndian, idx.BBoxMin); err != nil {
		return nil, fmt.Errorf("read bboxMin: %w", err)
	}
	if err := binary.Read(br, binary.LittleEndian, idx.BBoxMax); err != nil {
		return nil, fmt.Errorf("read bboxMax: %w", err)
	}

	offsets := make([]uint32, K+1)
	if err := binary.Read(br, binary.LittleEndian, offsets); err != nil {
		return nil, fmt.Errorf("read offsets: %w", err)
	}
	for c := 0; c < K; c++ {
		idx.ClusterStart[c] = int(offsets[c])
		idx.ClusterEnd[c] = int(offsets[c+1])
	}

	idx.DimsBuf = make([]int16, D*idx.N)
	for j := 0; j < D; j++ {
		idx.Dim[j] = idx.DimsBuf[j*idx.N : (j+1)*idx.N]
	}

	const chunk = 16384
	tmp := make([]int16, chunk*D)
	done := 0
	for done < idx.N {
		take := idx.N - done
		if take > chunk {
			take = chunk
		}
		buf := tmp[:take*D]
		if err := binary.Read(br, binary.LittleEndian, buf); err != nil {
			return nil, fmt.Errorf("read vectors: %w", err)
		}
		// Transpose row-major → column-major (Dim[j][i] layout).
		for i := 0; i < take; i++ {
			row := buf[i*D : (i+1)*D]
			for j := 0; j < D; j++ {
				idx.Dim[j][done+i] = row[j]
			}
		}
		done += take
	}

	idx.Labels = make([]uint8, idx.N)
	if _, err := io.ReadFull(br, idx.Labels); err != nil {
		return nil, fmt.Errorf("read labels: %w", err)
	}
	idx.OrigIDs = make([]uint32, idx.N)
	if err := binary.Read(br, binary.LittleEndian, idx.OrigIDs); err != nil {
		return nil, fmt.Errorf("read orig_ids: %w", err)
	}

	return idx, nil
}
