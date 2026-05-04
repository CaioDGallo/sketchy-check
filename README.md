# Sketchy — Fraud Detection Backend (Rinha de Backend 2026)

A Go + cgo/AVX2 fraud-detection backend built for the [Rinha de Backend 2026](../README.md) challenge. Approves or denies card transactions by k-NN vector search against a 3-million-vector reference dataset, end-to-end in a few milliseconds, inside **1 CPU and 350 MB of RAM** total.

The implementation deliberately mirrors the algorithmic structure of the highest-scoring C reference ([thiagorigonatti/rinha-2026](https://github.com/thiagorigonatti/rinha-2026)) but stays in Go for everything except the hot SIMD inner loop, which is a small cgo + AVX2 kernel.

---

## What the app does

Each request to `POST /fraud-score` carries a card transaction. The backend:

1. Parses the JSON payload manually (no `encoding/json`).
2. Builds a 14-dimension feature vector following the [DETECTION_RULES.md](../docs/en/DETECTION_RULES.md) formulas.
3. Quantizes the vector to int16 (`scale = 10000`).
4. Searches a pre-built IVF (Inverted File) index of 3,000,000 reference vectors for the 5 nearest neighbors using Euclidean distance.
5. Counts how many of the 5 are labeled `fraud`.
6. Returns one of 6 pre-built HTTP responses — `{ approved: bool, fraud_score: 0.0..1.0 }` — chosen by the fraud count.

`approved = (fraud_score < 0.6)` ↔ `fraud_count_in_top5 < 3`.

`GET /ready` returns 200 once the index is loaded.

---

## Architecture

```
                ┌──────────────────┐
   client ────► │   nginx :9999    │   (1 worker, epoll, keepalive 64)
                │   round-robin    │
                └────────┬─────────┘
                         │   Unix Domain Sockets
              ┌──────────┴──────────┐
              ▼                     ▼
     ┌─────────────────┐   ┌─────────────────┐
     │    api-1        │   │    api-2        │
     │  Go + cgo/AVX2  │   │  Go + cgo/AVX2  │
     │  IVF index in   │   │  IVF index in   │
     │  RAM (~100 MB)  │   │  RAM (~100 MB)  │
     └─────────────────┘   └─────────────────┘
```

### Resource budget (matches the 1 CPU / 350 MB cap exactly)

| Service | CPU | Memory | Role |
|---|---|---|---|
| `nginx`  | 0.15  | 30 MB  | Round-robin LB on `:9999`, UDS upstream |
| `api-1`  | 0.425 | 160 MB | Go server + IVF index + cgo AVX2 kernel |
| `api-2`  | 0.425 | 160 MB | Identical to api-1 |
| **total** | **1.00** | **350 MB** | hard cap |

Everything runs on a `bridge` network. No `host`, no `privileged`. Images are public (`nginx:1.27-alpine` plus our `sketchy-api:local`).

---

## Algorithms

### IVF (Inverted File) index with K = 256 clusters

Build-time work:

1. Stream-decode `references.json.gz` into 3 M float32 vectors.
2. Run **k-means** (10 iterations, 131 K sample) with K = 256 to get cluster centroids.
3. Assign every reference vector to its nearest centroid.
4. Sort the vectors physically by cluster, so cluster *c* lives in a contiguous slab `vectors[start[c]:end[c]]`.
5. Compute, per cluster, the **per-dimension bounding box** `[min, max]` over the cluster's vectors.
6. Quantize all 14 dims to int16 with `scale = 10000` (real `[-1, 1]` → int16 `[-10000, 10000]`).
7. Persist everything in a binary `index.bin` (IVF6 layout).

Runtime work, per request:

1. Quantize the query: `qInt[14]` int16 + `qGrid[14]` float (== `qInt[j] / 10000`, the same grid space the cluster centroids live in).
2. Compute the squared float distance from `qGrid` to every cluster centroid (256 × 14 ≈ 3.6 K float ops).
3. **NPROBE pass** — pick the closest 1 cluster (configurable; we use NPROBE = 1) and scan its vectors to seed the top-5.
4. **Bounding-box repair pass** — for every still-unscanned cluster, compute the cheapest possible squared distance from `qInt` to the cluster's bbox. If that lower bound is already worse than the current top-5's worst slot, skip the cluster. Otherwise scan it.
5. Count fraud labels in the final top-5.

Why bounding-box repair matters: NPROBE = 1 alone would lose accuracy (some queries' true neighbors live in another cluster). The bbox check is a *cheap exact lower bound*; it prevents us from skipping any cluster that could possibly contain a better neighbor. The result is the **exact same top-5 as brute-force k-NN**, while in practice scanning ~3-8 % of the dataset.

### Deterministic top-5 tie-breaking

The contest grader labels test payloads with k = 5 brute-force k-NN. When two reference vectors have identical squared distance to the query (which happens — the dataset has duplicates and quantization compresses 32-bit floats into 16-bit integers), the grader breaks ties by the vectors' original position in `references.json.gz`. We use the exact same rule:

> A candidate `(d, orig_id)` beats `(d', orig_id')` iff `d < d'` OR `(d == d' AND orig_id < orig_id')`.

Because we keep `orig_ids[]` in the index and apply this rule everywhere we update the top-5, our predictions match the grader's labels **byte-for-byte**. That's how we hit a perfect detection score (0 false positives, 0 false negatives).

---

## Techniques

These are the moves that turn a correct k-NN backend into a *fast* one. Each one is small, but they compose multiplicatively.

### 1. int16 quantization (`scale = 10000`)

All dim values land in `[-1, 1]` (real values are clamped to `[0, 1]`; the sentinel `-1` for missing `last_transaction` rides along). Mapping to `[-10000, 10000]` int16 cuts memory in half (vs float32) and lets the entire distance computation stay in fixed-point integer math.

`(int16 - int16)² ≤ (20000)² = 4 × 10⁸` fits in int32. Sum of 14 squared diffs fits in int64 with massive headroom. No FPU touch in the hot loop.

### 2. Column-major (Structure of Arrays) layout

On disk the index is row-major: 14 ints per record, back-to-back. At load time we transpose into 14 contiguous arrays, one per dimension. With AVX2 we can then load 8 lanes of the same dim at once (`_mm_loadu_si128` of int16, widen to int32), broadcast the query value (`_mm256_set1_epi32`), subtract, square, and accumulate into int64 lanes. One AVX2 micro-op processes 8 vectors of 1 dim. Repeat 14 times to finish 8 vectors. Heavily memory-bound — prefetchers love the stride.

### 3. cgo + AVX2 inner kernel (with scalar fallback)

`internal/ivf/kernel/kernel_amd64.c` carries the AVX2 implementation, behind `#ifdef __AVX2__`. Compiled with `-march=haswell -mavx2 -O3 -flto`.

`internal/ivf/kernel/scanrange_fallback.go` is a pure-Go scalar version with the same dim-ordering and early-exit pruning. Build tags pick one or the other:

- `cgo && amd64` → AVX2 kernel
- otherwise (Mac ARM dev, etc.) → Go scalar

Both paths produce **bit-identical** output for the same input — same dim order, same tie-break, same pruning predicates. So you can develop and verify on a laptop and trust the prod numbers.

### 4. Bounding-box repair instead of "more probes"

The reference C uses NPROBE = 1 plus bbox repair. We do the same. Compared to NPROBE = 32 with no repair (a more typical IVF setup), this scans far fewer vectors *and* is provably exact.

### 5. nginx → Unix Domain Sockets → Go

The shared `rinha-sockets` named volume mounts `/sockets` in nginx and both api containers. Each api binds `/sockets/api{N}.sock`; nginx upstreams over `unix:/sockets/api{N}.sock`. No TCP/IP stack in the loop — just kernel `read`/`write` between two processes on the same host. Saves tens of microseconds of TCP overhead per request. nginx upstream `keepalive 64` keeps the UDS sockets pooled, so the Go server sees a small handful of long-lived connections, not a churn of new ones.

`proxy_set_header Connection "";` is *required* for upstream keepalive to work — without it, nginx sends `Connection: close` on every request.

### 6. Pre-built HTTP responses

The decision space has exactly **six** outcomes (`fraud_count ∈ {0..5}`), so the response set is tiny:

```
{"approved":true,"fraud_score":0.0000}
{"approved":true,"fraud_score":0.2000}
{"approved":true,"fraud_score":0.4000}
{"approved":false,"fraud_score":0.6000}
{"approved":false,"fraud_score":0.8000}
{"approved":false,"fraud_score":1.0000}
```

We build these once at startup with full HTTP/1.1 status line + headers + body, store them as `[6][]byte`, and at request time emit the indexed entry with a single `conn.Write`. Zero JSON marshaling on the hot path. Same for `/ready`, 400, 404, 413, 500.

### 7. Manual JSON parsing

`encoding/json` uses reflection and allocates per request. At sub-millisecond budgets that's significant. Our `internal/vectorize` walks the body bytes directly:

1. Locate the four top-level objects (`transaction`, `customer`, `merchant`, `terminal`) by `bytes.Index` on quoted keys, and find each one's matching `}` via brace counting.
2. Within each object, scan for known keys, find the colon, and parse the value (`strconv.ParseFloat` for numbers, byte-slice compare for booleans/strings).
3. `last_transaction` is checked separately and may be missing/null.
4. Date fields are parsed by *byte position* — the format is fixed ISO-8601, so `hour` is at chars 11–12, etc. Weekday is computed via Howard Hinnant's `days_from_civil` algorithm (pure integer math).

The output goes into a stack-allocated `[14]float32`.

### 8. Memory budget pinning

Each api process runs with:

```
GOGC=off
GOMEMLIMIT=140MiB
GOMAXPROCS=1
```

`GOGC=off` disables the periodic GC trigger. Per-request allocations are tiny (a few small strings inside JSON parsing); over the 120 s test the heap stays well under the limit. `GOMEMLIMIT` is a soft cap that would force GC if we approached the 160 MB container limit. `GOMAXPROCS=1` matches our 0.4-ish CPU share — no goroutine bouncing between OS threads.

---

## Index file format (IVF6)

`index.bin` layout, all little-endian, all read once at startup into RAM:

| section | content | size (N = 3 M, K = 256, D = 14) |
|---|---|---|
| header | `"IVF6"` magic, N (u32), K (u32), D (u32), stride (u32 = D), scale (f32 = 10000) | 28 B |
| centroids | K × D × float32 | 14 KB |
| bbox_min | K × D × int16 (per-cluster, per-dim min, quantized) | 7 KB |
| bbox_max | K × D × int16 | 7 KB |
| offsets | (K + 1) × uint32 — prefix sums; cluster `c` spans `[offsets[c], offsets[c+1])` | 1 KB |
| vectors | N × D × int16, **row-major on disk**, sorted by cluster | ~84 MB |
| labels | N × uint8, parallel to vectors (1 = fraud) | ~3 MB |
| orig_ids | N × uint32 — record's original position in `references.json.gz` | ~12 MB |

Total ≈ **99 MB** on disk, ≈ **100 MB** in RAM after the load-time transpose to column-major.

The transpose is the one expensive thing the runtime does at startup: read 16 KiB chunks of row-major int16, scatter them into 14 column-major dim arrays. Cost is ~120 ms locally, ~500 ms under amd64 emulation. After that, everything is steady-state.

---

## Goals & scoring

The contest's [scoring formula](../docs/en/EVALUATION.md) sums two log-scaled components, each capped at ±3000:

- **Latency (`p99_score`)** — every 10× improvement in p99 is +1000 points. p99 ≤ 1 ms saturates at +3000.
- **Detection (`detection_score`)** — log-rate of weighted errors (FP × 1, FN × 3, HTTP × 5), with a `−β·log(1+E)` absolute penalty. Hard cutoff at 15 % failure rate → −3000.

Our design targets both:

| Component | Mechanism | Expected ceiling |
|---|---|---|
| **Detection** | Exact IVF + bbox repair + deterministic tie-break → 0 FP / 0 FN | **3000 / 3000** |
| **Latency** | Quantization + SoA + AVX2 + UDS + pre-built responses → p99 ≈ 1-3 ms | **2300-3000 / 3000** |

Measured iteration-1 result on Apple Silicon under amd64 emulation:

```
final_score = 5399.31
  p99            = 3.99 ms       p99_score = 2399.31
  FP / FN / Err  = 0 / 0 / 0     detection = 3000.00 (max)
  total requests = 54059         failures  = 0.00 %
```

On a real Linux/amd64 Haswell host (the contest target), the latency component should improve substantially — emulation alone is a 3-5× tax — putting the realistic score in the **5700-5900** band.

---

## File map

```
sketchy/
├── README.md                              ← this file
├── Makefile                               (build orchestration)
├── docker-compose.yml                     (3 services, resource caps)
├── nginx/
│   └── nginx.conf                         (UDS upstream, round-robin)
└── api/
    ├── Dockerfile                         (multi-stage; cgo build)
    ├── go.mod
    ├── cmd/
    │   ├── api/main.go                    (runtime entry: load + serve)
    │   └── preprocess/main.go             (build-time: gz → IVF6 index.bin)
    └── internal/
        ├── kmeans/
        │   └── kmeans.go                  (Lloyd's algorithm; build-time)
        ├── ivf/
        │   ├── index.go                   (Load; row→col-major transpose)
        │   ├── search.go                  (NPROBE + bbox repair + Frauds())
        │   └── kernel/
        │       ├── top5.go                (Top5 struct, shared)
        │       ├── scanrange_cgo.go       (cgo wrapper, build: cgo+amd64)
        │       ├── scanrange_fallback.go  (Go scalar, build: !cgo || !amd64)
        │       ├── kernel_amd64.h         (C struct + extern decl)
        │       └── kernel_amd64.c         (AVX2 kernel + scalar fallback)
        ├── vectorize/
        │   └── vectorize.go               (manual JSON → [14]float32)
        ├── mcc/
        │   └── mcc.go                     (mcc_risk lookup w/ baked defaults)
        ├── responses/
        │   └── responses.go               (pre-built HTTP responses)
        └── server/
            └── server.go                  (UDS HTTP/1.1 server)
```

---

## Build & run

Prereqs: Docker (Compose v2), Go 1.23+, k6 (for load testing). On Apple Silicon, OrbStack or Docker Desktop with x86 emulation.

```bash
cd sketchy

# 1. Generate the IVF6 index from references.json.gz (~120 s once)
make index

# 2. Build the docker images and start the stack
make up

# 3. Smoke
make smoke

# 4. Tail logs / down
make logs
make down
```

Run the contest's k6 load script (from the project root):

```bash
k6 run test/test.js
cat test/results.json | jq .scoring.final_score
```

---

## Why this works (in two sentences)

The detection score is *unconditional* — IVF with bbox repair plus deterministic tie-break produces the exact same top-5 as brute force, so the grader's labels match ours. The latency score follows from removing every unnecessary syscall, allocation, copy, and instruction-level dependency from the hot path: int16 SIMD, SoA layout, UDS, pre-built responses, manual JSON. Stack the two and the result is correct *and* fast inside a tiny resource envelope.

---

## Reference

- Reference C implementation we studied: [thiagorigonatti/rinha-2026](https://github.com/thiagorigonatti/rinha-2026) — IVF/K-Means + io_uring + UDS + AVX2.
- Contest spec: [docs/en/README.md](../docs/en/README.md).
- Detection rules: [docs/en/DETECTION_RULES.md](../docs/en/DETECTION_RULES.md).
- Evaluation formula: [docs/en/EVALUATION.md](../docs/en/EVALUATION.md).
- Submission process: [docs/en/SUBMISSION.md](../docs/en/SUBMISSION.md).
