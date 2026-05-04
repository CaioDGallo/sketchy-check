# Rust Port — Next Performance Iteration Plan

This document is a hand-off for the next agent. The Rust port at `api-rust/`
is functionally complete and serves the contest workload with **detection_score
= 3000 (perfect, 0 errors)**. Final-score plateau is at **~5255 / 6000**, with
p99 ≈ 5.56 ms. This plan covers the two paths to break the plateau.

---

## Current state

- Branch: `posthog-code/rust-port` (pushed to origin)
- Submission branch: `submission` (image `caiogallo2401/sketchy-rust:0.4.0`)
- Image is unchanged across v0.5.x; only compose has been hardened
- Detection: 3000/3000 — algorithm is byte-identical to the Go reference
- Architecture: symmetric 0.40+0.40 CPU, multi-threaded epoll fallback
  (WORKERS=2 per container), io_uring path present but engagement on the
  grader is uncertain

### Score history

| Version | Architecture | Final | p99 |
|---|---|---|---|
| v0.1.0 | blocking thread-per-conn, asymmetric 0.80/0.05 | 4619 | 24.04 ms |
| v0.2.0 | single-threaded epoll, asymmetric 0.80/0.05 | 4447 | 35.70 ms |
| v0.3.0 | multi-thread epoll, **symmetric 0.40/0.40** | 5237 | 5.80 ms |
| v0.4.0 | + memchr SIMD memmem in JSON parser | 5246 | 5.68 ms |
| v0.5.0 | + seccomp=unconfined (anchor-merged) | 5248 | 5.65 ms |
| v0.5.1 | + explicit per-service security_opt + memlock=-1 | 5255 | 5.56 ms |

### Latest grader result (commit `f2fa793`, image v0.4.0)

```json
{
  "repo-url": "https://github.com/CaioDGallo/sketchy-check",
  "test-results": {
    "expected": {
      "total": 54100, "fraud_count": 24058, "legit_count": 30042,
      "fraud_rate": 0.4447, "legit_rate": 0.5553,
      "edge_case_count": 797, "edge_case_rate": 0.0147
    },
    "p99": "5.56ms",
    "scoring": {
      "breakdown": {
        "false_positive_detections": 0,
        "false_negative_detections": 0,
        "true_positive_detections": 24037,
        "true_negative_detections": 30022,
        "http_errors": 0
      },
      "failure_rate": "0%",
      "weighted_errors_E": 0,
      "error_rate_epsilon": 0,
      "p99_score": { "value": 2254.91, "cut_triggered": false },
      "detection_score": {
        "value": 3000, "rate_component": 3000,
        "absolute_penalty": 0, "cut_triggered": false
      },
      "final_score": 5254.91
    }
  },
  "runtime-info": {
    "mem": 350, "cpu": 1,
    "images": {
      "haproxy:3.3": [{
        "Name": "/submission-haproxy-1",
        "Image": "haproxy:3.3",
        "HostConfig": { "Memory": 52428800, "NanoCpus": 200000000 }
      }],
      "caiogallo2401/sketchy-rust:0.4.0": [
        {
          "Name": "/submission-api1-1",
          "Image": "caiogallo2401/sketchy-rust:0.4.0",
          "HostConfig": { "Memory": 157286400, "NanoCpus": 400000000 }
        },
        {
          "Name": "/submission-api2-1",
          "Image": "caiogallo2401/sketchy-rust:0.4.0",
          "HostConfig": { "Memory": 157286400, "NanoCpus": 400000000 }
        }
      ]
    },
    "instances-number-ok?": true,
    "unlimited-services": null,
    "commit": "f2fa793"
  }
}
```

### Why the plateau (working theory)

At 900 RPS sustained with 0.80 CPU total budget, the per-request CPU budget
is `0.80 / 900 ≈ 0.89 ms`. Our search alone is ~1 ms per query → we're 12%
over capacity, queue forms, p99 grows to ~5–6× the per-request time. C #1
sits at ~6000 final score, suggesting their per-request CPU is ~0.5 ms (so
they have headroom and queueing collapses).

**To break the plateau we likely need ~half our per-request CPU**, which is
a search-side optimization, not an I/O-layer fix. But step (B) below
quantifies the I/O ceiling first so we don't optimize the wrong thing.

---

## Plan B — quantify the I/O ceiling locally

**Goal:** establish whether engaging io_uring on the grader would meaningfully
move p99, or whether the bottleneck is purely CPU.

**Hypothesis:** if io_uring vs epoll gives <0.5 ms difference under load on
the same hardware/binary, then the grader's still-engaged-or-not status is a
red herring and we should skip directly to (C).

### B.1 — Setup

Build a native arm64 image (no qemu) so io_uring can engage on Docker
Desktop's Linux VM kernel:

```bash
docker build --platform linux/arm64 -f api-rust/Dockerfile -t sketchy-rust:arm64-bench .
```

Run two stacks side-by-side on different ports:

- **Stack A** (io_uring): `--security-opt seccomp=unconfined`, env `SERVER_MODE=` (default)
- **Stack B** (epoll forced): default seccomp, env `SERVER_MODE=epoll`

Both single-instance for clarity (skip HAProxy; bind UDS, send curl directly).

Verify mode by inspecting stderr:
```bash
docker logs <container> 2>&1 | grep "server mode:"
# expect "io_uring (qd=4096, accept_sqes=256)" or "epoll (workers=2, ...)"
```

### B.2 — Load generator

Use `wrk2` or `hey` for sustained-rate testing (closer to k6's behavior than
plain `wrk`, which is open-loop). Install:
```bash
brew install wrk hey   # whichever
```

If only `wrk` is available it still works — just note the difference.

Workload script (legit + fraud randomized) — port from
`/Users/caiodgallo/projects/sketchy-check/Makefile`'s `smoke` target. The
two payloads in there are sufficient for a per-mode comparison.

For UDS targets, bridge to TCP via `socat`:
```bash
socat TCP-LISTEN:9999,reuseaddr,fork UNIX-CONNECT:/tmp/api.sock
```

Then:
```bash
wrk -t2 -c30 -d60s -R 900 -s post.lua http://localhost:9999/fraud-score
```

(`-c30` matches HAProxy's typical pool; `-R 900` matches grader's peak RPS.)

### B.3 — Decision tree

| io_uring p99 | epoll p99 | Conclusion | Next |
|---|---|---|---|
| ≤ 2 ms | ≤ 2 ms | I/O is fine on arm64; grader's per-request CPU is the limit | → (C) |
| ≤ 2 ms | 4–5 ms | io_uring is materially faster; engagement on grader matters | → (D, see below) |
| 4–5 ms | 4–5 ms | Both bottlenecked by per-request CPU on arm64 (no AVX2) | inconclusive — go to (C) anyway |

**Caveat:** arm64 falls back to the scalar kernel (no AVX2). Per-request CPU
is much higher than on the grader's Haswell. So absolute numbers won't match
the grader, but the **relative delta** (io_uring vs epoll) is what we care
about.

### B.4 — Plan D (only if B says io_uring helps)

If io_uring is materially faster locally but the grader is at epoll-tier
latency, the issue is grader-environment-specific. Possible blockers:

- AppArmor profile (separate from seccomp)
- Kernel sysctl `kernel.io_uring_disabled = 1` or `2`
- Docker daemon `--security-opt` defaults
- Older Docker version that ignores `security_opt: seccomp=unconfined`
- io_uring rings count against RLIMIT_MEMLOCK on pre-5.12 kernels (we
  already added `memlock: -1` in v0.5.1, but verify it took effect)

A diagnostic to try: write the **active server mode into a file in the
shared `/sockets` volume** at startup. The grader's runtime info doesn't
expose volume contents, but if we ever can get logs (e.g., by mailing the
contest org), we'll know which mode ran.

```rust
// In server/mod.rs after mode is decided:
let _ = std::fs::write(
    format!("/sockets/{hostname}.mode"),
    mode_label.as_bytes(),
);
```

A more invasive diagnostic: use `IORING_SETUP_SINGLE_ISSUER + COOP_TASKRUN`
flags (kernel 6.0+ / 5.19+). If the grader's kernel is older, the flags fail
and our existing fallback chain catches it; if newer, perf improves slightly.
See `IoUring::builder().setup_single_issuer().setup_coop_taskrun().build(qd)`
in `api-rust/src/server/iouring.rs:33`.

---

## Plan C — search hot-path optimization

**Goal:** drop per-request CPU from ~1 ms to ~0.5 ms so we have headroom for
sustained 900 RPS on 0.80 cgroup CPU. C #1 achieves this with the same
algorithm; the gap is implementation efficiency, not algorithmic novelty.

### C.1 — Profile-guided optimization (PGO)

Highest leverage / lowest risk. rustc supports PGO since Rust 1.37+.

Workflow:

1. Build instrumented binary:
   ```bash
   RUSTFLAGS="-C target-cpu=haswell -C profile-generate=/tmp/pgo-data" \
     cargo build --release --bin api
   ```
2. Run a representative load (use the local load generator from B):
   ```bash
   ./target/release/api & ; sleep 60 ; wait_grader_workload ; kill %1
   ```
3. Merge profile data:
   ```bash
   llvm-profdata merge -o /tmp/pgo-data/merged.profdata /tmp/pgo-data
   ```
4. Rebuild with profile data:
   ```bash
   RUSTFLAGS="-C target-cpu=haswell -C profile-use=/tmp/pgo-data/merged.profdata" \
     cargo build --release --bin api
   ```

Embed in `api-rust/Dockerfile` as a multi-stage build (instrumented build →
seed run → final build). The seed run needs the full index loaded, which
takes ~2 min on emulation but only one-time per image build.

Expected gain: **5–15% on hot-loop performance.** If our hot loop is mostly
the AVX2 dispatch + Top5 update path, PGO should give better register
allocation and inlining.

### C.2 — AVX2 prefetch ahead

Hot path: `api-rust/src/kernel/avx2.rs:48–86`. Each iteration loads 14
streams at offset `i`, processes 8 vectors at a time. The L1d footprint
is ~32 KB; 14 streams × 64 bytes = 896 bytes per iter — well in L1, but
we may be missing on prefetch ahead.

Add explicit prefetch hints:

```rust
use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
// Inside the AVX2 loop, before the acc_dim cluster:
_mm_prefetch(d5.add(i + 64) as *const i8, _MM_HINT_T0);
_mm_prefetch(d6.add(i + 64) as *const i8, _MM_HINT_T0);
// ... for each of the 14 dims, prefetch i+64
```

Or batch by issuing 14 prefetches once per iteration. Tune the offset (try
32, 64, 128 cache lines ahead).

Expected gain: **5–20% on the search loop**, depending on whether the
hardware prefetcher already covers the access pattern.

### C.3 — Reduce bbox-repair scans

`api-rust/src/kernel/mod.rs:84–98`. After scanning the NPROBE=1 cluster,
we walk all 255 remaining clusters and call `bbox_lower_bound` on each.
That's 255 × 14 = 3570 i32 ops per query (~3–5 µs).

If we can short-circuit the bbox check sooner (e.g., sort clusters by
centroid distance and stop when centroid distance² > worst_d), we cut the
average from 255 checks to ~50. Per-query saving: maybe 1–2 µs of the
control-flow work, plus avoiding actual cluster scans for clusters we'd
have skipped anyway.

C reference does this — see `rinha-c-good-latency/src/ivf_search.c:130–180`
for the cluster-prune-and-sort approach.

### C.4 — Top5 update tightening

`api-rust/src/kernel/top5.rs:35–53`. The `try_insert` then `find_worst`
pattern does an O(5) scan after each insert. For most candidates the
insert never happens (we early-exit on `is_better` check). Verify the
compiler is inlining `is_better` at every callsite (look at asm). If not,
mark with `#[inline(always)]`.

Also: in the AVX2 commit phase
(`api-rust/src/kernel/avx2.rs:65–80`), each lane reads `t.worst_d` /
`t.worst_id` — these can change mid-batch. Consider computing all 8 lane
results first, then committing in one pass with a single worst-update at
the end. Subtle, but reduces dependent loads in the hot loop.

### C.5 — Inline the AVX2 dispatch

`api-rust/src/kernel/mod.rs:78–101`. Currently `scan_range` does a runtime
`is_x86_feature_detected!("avx2")` check on every call. Cached after first
call but still a memory load per cluster. With `-C target-cpu=haswell` the
binary always has AVX2; we can drop the runtime check entirely behind a
`cfg!(target_feature = "avx2")`.

---

## Verification & exit criteria

After each change, run:

1. **Unit/smoke** (`make smoke-rust`): both legit and fraud paths return
   byte-identical bodies to the Go reference.
2. **Local k6** if available: `final_score ≥ 5400` under arm64 emulation
   (which has heavy emulation tax on AVX2).
3. **Grader run**: open `gh issue create --repo zanfranceschi/rinha-de-backend-2026
   --title "rinha/test sketchy-check (Rust vX.Y.Z)" --body "rinha/test sketchy-check"`
   after pushing the new image and bumping the submission branch.

**Stop condition:** detection_score < 3000 ever — that means the algorithm
drifted. Revert.

**Target:** `final_score ≥ 5800` with detection still 3000.

---

## Reference paths

- Source: `api-rust/src/`
  - `main.rs` — entry, loads index/MCC, runs server
  - `server/mod.rs` — picks io_uring → epoll → blocking
  - `server/iouring.rs` — single-threaded io_uring loop
  - `server/epoll.rs` — multi-threaded epoll loop (current production path)
  - `kernel/mod.rs` — IVF + bbox-repair orchestrator
  - `kernel/avx2.rs` — AVX2 inner loop (the hot CPU path)
  - `kernel/scalar.rs` — scalar fallback (arm64 dev)
  - `vectorize.rs` — manual JSON → [f32; 14]
- Submission: branch `submission` in this repo, with `docker-compose.yml`
  (Rust image 0.4.0) and `haproxy.cfg`
- C reference for comparison: `/Users/caiodgallo/projects/rinha-c-good-latency/`
  - `src/iouring_server.c` — io_uring loop pattern
  - `src/ivf_search.c` — bbox-repair logic with cluster sort
  - `src/vectorizer.c` — JSON parsing approach
- Submission flow doc: `RUNS.md` at repo root
- Original port plan: `~/Library/Application Support/@posthog/posthog-code/claude/plans/let-s-design-a-rust-declarative-lemur.md`

## Tools

- `make build-rust` — build Rust image
- `make up-rust` — start stack on `localhost:9999`
- `make smoke-rust` — curl smoke test
- `docker tag sketchy-rust:local caiogallo2401/sketchy-rust:VERSION && docker push ...`
- `gh issue create --repo zanfranceschi/rinha-de-backend-2026 --body "rinha/test sketchy-check"`
- Submission worktree pattern: `git worktree add /tmp/sketchy-submission origin/submission`
