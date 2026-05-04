// preprocess reads references.json.gz, trains a K=256 IVF index via k-means,
// quantizes the 14-dim vectors to int16 (scale=10000), sorts them by cluster,
// computes per-cluster bounding boxes, and writes the result in IVF6 binary
// format compatible with the runtime loader.
//
// Output layout (little-endian):
//
//	magic       [4]byte      "IVF6"
//	n           uint32       record count (e.g. 3,000,000)
//	k           uint32       cluster count (256)
//	d           uint32       dimension (14)
//	stride      uint32       per-record stride in vectors section (== d)
//	scale       float32      quantization scale (10000.0)
//	centroids   K*D float32  cluster centroids in float-grid space
//	bbox_min    K*D int16    per-cluster, per-dim minimum (quantized)
//	bbox_max    K*D int16    per-cluster, per-dim maximum (quantized)
//	offsets     (K+1)*uint32 prefix sums into vectors[]: cluster c spans [offsets[c], offsets[c+1])
//	vectors     N*D int16    quantized vectors, sorted by cluster (row-major: 14 ints per record)
//	labels      N    uint8   1 = fraud, 0 = legit
//	orig_ids    N    uint32  original record index in references.json.gz, for deterministic tie-break
package main

import (
	"bufio"
	"compress/gzip"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"os"
	"strings"

	"github.com/rinha2026/sketchy/api/internal/kmeans"
)

const (
	dim         = 14
	ivfK        = 256
	fixScale    = 10000.0
	defaultIter = 10
	// Sample size for the k-means refinement step. The reference uses 131072.
	defaultSampleN = 131072
)

type record struct {
	Vector [dim]float32 `json:"vector"`
	Label  string       `json:"label"`
}

func main() {
	if len(os.Args) < 3 {
		fmt.Fprintln(os.Stderr, "usage: preprocess <references.json[.gz]> <out index.bin>")
		os.Exit(2)
	}
	in, out := os.Args[1], os.Args[2]

	fmt.Fprintf(os.Stderr, "reading %s\n", in)
	vectors, labels, err := readReferences(in)
	if err != nil {
		fmt.Fprintf(os.Stderr, "read: %v\n", err)
		os.Exit(1)
	}
	n := len(labels)
	if n == 0 {
		fmt.Fprintln(os.Stderr, "no vectors loaded")
		os.Exit(1)
	}
	fmt.Fprintf(os.Stderr, "loaded N=%d vectors (%.2f MB float)\n", n, float64(n)*dim*4/1024/1024)

	fmt.Fprintf(os.Stderr, "kmeans K=%d sample=%d iters=%d\n", ivfK, defaultSampleN, defaultIter)
	centroids := kmeans.Train(vectors, n, ivfK, dim, defaultSampleN, defaultIter)

	fmt.Fprintf(os.Stderr, "assigning %d records to clusters\n", n)
	assign := make([]uint32, n)
	counts := make([]uint32, ivfK)
	for i := 0; i < n; i++ {
		c := kmeans.Assign(vectors[i*dim:(i+1)*dim], centroids, ivfK, dim)
		assign[i] = uint32(c)
		counts[c]++
		if i > 0 && i%500_000 == 0 {
			fmt.Fprintf(os.Stderr, "  assigned %d/%d\n", i, n)
		}
	}

	offsets := make([]uint32, ivfK+1)
	for c := 0; c < ivfK; c++ {
		offsets[c+1] = offsets[c] + counts[c]
	}
	writePos := make([]uint32, ivfK)
	copy(writePos, offsets[:ivfK])

	outVectors := make([]int16, n*dim)
	outLabels := make([]uint8, n)
	outIDs := make([]uint32, n)
	bboxMin := make([]int16, ivfK*dim)
	bboxMax := make([]int16, ivfK*dim)
	for c := 0; c < ivfK; c++ {
		for j := 0; j < dim; j++ {
			bboxMin[c*dim+j] = math.MaxInt16
			bboxMax[c*dim+j] = math.MinInt16
		}
	}

	fmt.Fprintln(os.Stderr, "sorting + quantizing + computing bboxes")
	for i := 0; i < n; i++ {
		c := assign[i]
		pos := writePos[c]
		writePos[c]++
		src := vectors[i*dim : (i+1)*dim]
		dst := outVectors[int(pos)*dim : (int(pos)+1)*dim]
		for j := 0; j < dim; j++ {
			qv := quantize(src[j])
			dst[j] = qv
			bi := int(c)*dim + j
			if qv < bboxMin[bi] {
				bboxMin[bi] = qv
			}
			if qv > bboxMax[bi] {
				bboxMax[bi] = qv
			}
		}
		outLabels[pos] = labels[i]
		outIDs[pos] = uint32(i)
	}
	for c := 0; c < ivfK; c++ {
		if counts[c] == 0 {
			for j := 0; j < dim; j++ {
				bboxMin[c*dim+j] = 0
				bboxMax[c*dim+j] = 0
			}
		}
	}

	fmt.Fprintf(os.Stderr, "writing %s\n", out)
	if err := writeIndex(out, uint32(n), centroids, bboxMin, bboxMax, offsets, outVectors, outLabels, outIDs); err != nil {
		fmt.Fprintf(os.Stderr, "write: %v\n", err)
		os.Exit(1)
	}
	fmt.Fprintf(os.Stderr, "done: %s (N=%d K=%d)\n", out, n, ivfK)
}

func readReferences(path string) (vectors []float32, labels []uint8, err error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, nil, err
	}
	defer f.Close()

	var r io.Reader = f
	if strings.HasSuffix(path, ".gz") {
		gz, gerr := gzip.NewReader(f)
		if gerr != nil {
			return nil, nil, gerr
		}
		defer gz.Close()
		r = gz
	}

	br := bufio.NewReaderSize(r, 1<<20)
	dec := json.NewDecoder(br)

	tok, err := dec.Token()
	if err != nil {
		return nil, nil, err
	}
	if d, ok := tok.(json.Delim); !ok || d != '[' {
		return nil, nil, fmt.Errorf("expected JSON array, got %v", tok)
	}

	const initial = 3_000_000
	vectors = make([]float32, 0, initial*dim)
	labels = make([]uint8, 0, initial)
	for dec.More() {
		var rec record
		if err := dec.Decode(&rec); err != nil {
			return nil, nil, err
		}
		vectors = append(vectors, rec.Vector[:]...)
		var lab uint8
		if rec.Label == "fraud" {
			lab = 1
		}
		labels = append(labels, lab)
		if n := len(labels); n%500_000 == 0 {
			fmt.Fprintf(os.Stderr, "  parsed %d vectors\n", n)
		}
	}
	return vectors, labels, nil
}

// quantize maps a real value in [-1, 1] to int16 in [-10000, 10000].
// The sentinel -1.0 (used for "no last_transaction") becomes -10000, which
// preserves natural distance to other -1 sentinels and stays well-separated
// from real [0,1] values.
func quantize(x float32) int16 {
	if x < -1 {
		x = -1
	} else if x > 1 {
		x = 1
	}
	scaled := x * fixScale
	if scaled >= 0 {
		scaled += 0.5
	} else {
		scaled -= 0.5
	}
	if scaled < -fixScale {
		scaled = -fixScale
	} else if scaled > fixScale {
		scaled = fixScale
	}
	return int16(scaled)
}

func writeIndex(path string, n uint32, centroids []float32, bboxMin, bboxMax []int16, offsets []uint32, vectors []int16, labels []uint8, ids []uint32) error {
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	defer f.Close()
	bw := bufio.NewWriterSize(f, 1<<20)

	if _, err := bw.Write([]byte("IVF6")); err != nil {
		return err
	}
	if err := binary.Write(bw, binary.LittleEndian, n); err != nil {
		return err
	}
	if err := binary.Write(bw, binary.LittleEndian, uint32(ivfK)); err != nil {
		return err
	}
	if err := binary.Write(bw, binary.LittleEndian, uint32(dim)); err != nil {
		return err
	}
	if err := binary.Write(bw, binary.LittleEndian, uint32(dim)); err != nil { // stride == dim
		return err
	}
	if err := binary.Write(bw, binary.LittleEndian, float32(fixScale)); err != nil {
		return err
	}

	if err := binary.Write(bw, binary.LittleEndian, centroids); err != nil {
		return err
	}
	if err := binary.Write(bw, binary.LittleEndian, bboxMin); err != nil {
		return err
	}
	if err := binary.Write(bw, binary.LittleEndian, bboxMax); err != nil {
		return err
	}
	if err := binary.Write(bw, binary.LittleEndian, offsets); err != nil {
		return err
	}
	if err := binary.Write(bw, binary.LittleEndian, vectors); err != nil {
		return err
	}
	if _, err := bw.Write(labels); err != nil {
		return err
	}
	if err := binary.Write(bw, binary.LittleEndian, ids); err != nil {
		return err
	}
	return bw.Flush()
}
