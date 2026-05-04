// Package kernel implements the inner-loop distance scan for IVF search.
// Two implementations are selected at build time:
//
//   - cgo + amd64:    AVX2 SIMD via kernel_amd64.c (fast)
//   - everything else: pure Go scalar fallback
//
// Both update a shared Top5 candidate list in place.
package kernel

// K and D mirror the constants in the parent IVF package and intentionally
// duplicate them so this package can stand on its own.
const (
	K = 5  // top-K nearest neighbors
	D = 14 // vector dimension
)

// Top5 holds the running top-K candidates. Layout is fixed and matches the
// equivalent C struct sketchy_top5_t in kernel_amd64.h byte-for-byte so the
// cgo wrapper can hand the same memory to C without copying.
//
// Total size: 88 bytes, alignment 8.
//
// Tie-break rule (matches the C reference): a candidate (d, id) beats (d', id')
// iff d < d' OR (d == d' AND id < id'). This makes the top-5 deterministic
// against the brute-force grader.
type Top5 struct {
	BestD   [5]uint64 // squared distances, each <= ~14 * (20000)^2 = 5.6e9, fits uint64 trivially
	BestID  [5]uint32 // orig_id (record's original position in references.json.gz)
	BestL   [5]uint8  // 1 = fraud, 0 = legit
	_pad    [3]uint8
	Worst   int32  // index in BestD/BestID/BestL of the current worst candidate
	WorstD  uint64 // BestD[Worst], cached for fast comparison in hot loop
	WorstID uint32 // BestID[Worst], cached
	_pad2   uint32
}

// Reset initializes top5 to an empty state ready for a fresh query.
// All slots start as "infinitely bad" so the first 5 valid candidates
// will replace them in any order, then the loop converges to true top-5.
func (t *Top5) Reset() {
	for i := 0; i < 5; i++ {
		t.BestD[i] = ^uint64(0)
		t.BestID[i] = ^uint32(0)
		t.BestL[i] = 0
	}
	t.Worst = 0
	t.WorstD = ^uint64(0)
	t.WorstID = ^uint32(0)
}

// Frauds returns the count of fraud labels among the top-5.
func (t *Top5) Frauds() int {
	n := 0
	for i := 0; i < 5; i++ {
		if t.BestL[i] == 1 {
			n++
		}
	}
	return n
}
