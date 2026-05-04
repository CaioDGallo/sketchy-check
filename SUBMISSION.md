# Submitting Sketchy to Rinha de Backend 2026

This document captures the exact steps to register and test our submission
against the official Rinha de Backend 2026 grader. Source: [SUBMISSION.md](../docs/en/SUBMISSION.md)
in the contest repo.

---

## Prerequisites

You need:

1. **A GitHub account.** This account's username becomes the participant
   filename (e.g. `participants/CaioDGallo.json`).
2. **A separate public GitHub repo** for our submission code. The official
   contest repo cannot host our code — we open a PR to that repo only to add
   our participant JSON file.
3. **A public container registry account** to publish the API image:
    - Docker Hub: free; uses `docker login`
    - GitHub Container Registry (`ghcr.io`): free; uses a GitHub PAT with
      `write:packages`. Recommended because it lives next to the source.
4. **`gh` CLI logged in**, plus `docker` running. Both already configured here.

---

## What the official grader expects

The grader workflow, end-to-end:

1. We open a **PR against `zanfranceschi/rinha-de-backend-2026`** that adds
   `participants/<our-github-username>.json`. This file lists the submission
   repos we want graded.
2. After the PR is merged, we open an **issue on the same official repo** with
   `rinha/test` in its body to trigger a preview test.
3. Rinha's engine clones our submission repo's `submission` branch, runs
   `docker compose up -d`, hits the load test against `:9999`, and posts the
   `results.json` back as a comment on our issue.

So our submission repo needs a `submission` branch whose root contains:
   - `docker-compose.yml` referencing a **public** image
   - any config files the compose mounts (`nginx.conf` for us)
   - `info.json` (participant + stack metadata)

The `main` branch holds the source code.

---

## Submission identity

Decisions to lock in before pushing:

| field | value (proposed) |
|---|---|
| GitHub username | `CaioDGallo` |
| Submission repo | `CaioDGallo/rinha-de-backend-2026-sketchy` |
| Submission `id` | `sketchy` (used inside the participant JSON file; one per repo) |
| Public image | Docker Hub `caiodgallo/sketchy-api:0.1.0` **OR** `ghcr.io/caiodgallo/sketchy-api:0.1.0` |
| Stack tags | `["go", "c", "nginx", "avx2", "ivf", "k-means", "cgo"]` |

If anything above needs to change, edit it before running the steps below.

---

## Step 1 — Push the source repo (`main` branch)

```bash
# From the rinha-de-backend-2026 root (this checkout)

# 1. Create the empty repo on GitHub
gh repo create CaioDGallo/rinha-de-backend-2026-sketchy \
  --public \
  --description "Sketchy: Go + cgo/AVX2 fraud detection backend for Rinha de Backend 2026"

# 2. Make a clean copy of just our source
mkdir -p /tmp/sketchy-src
cp -R sketchy/api sketchy/nginx sketchy/docker-compose.yml \
      sketchy/Makefile sketchy/README.md sketchy/SUBMISSION.md \
      /tmp/sketchy-src/

# 3. Initialize and push
cd /tmp/sketchy-src
git init -b main
git add .
git commit -m "Sketchy v0.1.0 — IVF/AVX2 fraud detection (5399 score under emulation)"
git remote add origin https://github.com/CaioDGallo/rinha-de-backend-2026-sketchy.git
git push -u origin main
```

Notes:
- We **don't** push `build_resources/` (it has the 100 MB `index.bin` blob).
  Add a `.gitignore` excluding it.
- We **don't** push the participants/ PR yet — that happens in Step 4.

---

## Step 2 — Build & push the public Docker image

The submission branch references this image by name, so it must already exist
in a public registry when the grader runs.

### Option A — Docker Hub (simplest)

```bash
docker login                 # one-time, prompts for username + password/PAT

# Tag our local build
docker tag sketchy-api:local caiodgallo/sketchy-api:0.1.0

# Push (single-arch linux/amd64 is fine; the contest host is amd64)
docker push caiodgallo/sketchy-api:0.1.0
```

### Option B — GitHub Container Registry (`ghcr.io`)

```bash
# One-time: create a GitHub PAT with "write:packages" scope, then:
echo "$GHCR_TOKEN" | docker login ghcr.io -u CaioDGallo --password-stdin

docker tag sketchy-api:local ghcr.io/caiodgallo/sketchy-api:0.1.0
docker push ghcr.io/caiodgallo/sketchy-api:0.1.0

# Make the package public:
#   gh.com/CaioDGallo?tab=packages → click sketchy-api → Settings → Change visibility → Public
```

### Verify it's public

```bash
docker pull caiodgallo/sketchy-api:0.1.0    # should succeed without `docker login`
```

---

## Step 3 — Create the `submission` branch

The submission branch carries **only what's needed to run the test**: no
source code, no Dockerfile, no Go module. Just compose + nginx config +
metadata.

### Files needed on this branch

| file | content |
|---|---|
| `docker-compose.yml` | services: nginx + api1 + api2, all referencing the public image; resource limits matching the 1 CPU / 350 MB cap |
| `nginx.conf` | identical to `sketchy/nginx/nginx.conf` |
| `info.json` | participant metadata |

A ready-to-copy example tree:

```bash
# From a fresh checkout of CaioDGallo/rinha-de-backend-2026-sketchy

git checkout --orphan submission
git rm -rf .

# docker-compose.yml — note: image: ... not build: ...
cat > docker-compose.yml <<'YAML'
x-api-common: &api-common
  image: caiodgallo/sketchy-api:0.1.0   # ← change if using ghcr.io
  volumes:
    - rinha-sockets:/sockets
  networks:
    - rinha-net
  ulimits:
    nofile:
      soft: 65535
      hard: 65535
  deploy:
    resources:
      limits:
        cpus: "0.425"
        memory: "160MB"

services:
  nginx:
    image: nginx:1.27-alpine
    ports:
      - "9999:9999"
    volumes:
      - ./nginx.conf:/etc/nginx/nginx.conf:ro
      - rinha-sockets:/sockets
    networks:
      - rinha-net
    depends_on:
      - api1
      - api2
    deploy:
      resources:
        limits:
          cpus: "0.15"
          memory: "30MB"

  api1:
    <<: *api-common
    hostname: api1
    environment:
      UDS_PATH: /sockets/api1.sock
      INDEX_PATH: /app/index.bin
      MCC_RISK_PATH: /app/mcc_risk.json
      IVF_NPROBE: "1"
      GOGC: "off"
      GOMEMLIMIT: "140MiB"
      GOMAXPROCS: "1"

  api2:
    <<: *api-common
    hostname: api2
    environment:
      UDS_PATH: /sockets/api2.sock
      INDEX_PATH: /app/index.bin
      MCC_RISK_PATH: /app/mcc_risk.json
      IVF_NPROBE: "1"
      GOGC: "off"
      GOMEMLIMIT: "140MiB"
      GOMAXPROCS: "1"

networks:
  rinha-net:
    driver: bridge

volumes:
  rinha-sockets:
YAML

cp /path/to/sketchy/nginx/nginx.conf nginx.conf

cat > info.json <<'JSON'
{
  "participants": ["Caio Gallo"],
  "social": ["https://github.com/CaioDGallo"],
  "source-code-repo": "https://github.com/CaioDGallo/rinha-de-backend-2026-sketchy",
  "stack": ["go", "c", "nginx", "avx2", "ivf", "k-means", "cgo"],
  "open_to_work": false
}
JSON

git add docker-compose.yml nginx.conf info.json
git commit -m "Submission: sketchy v0.1.0"
git push -u origin submission
```

### Sanity-check the submission branch from a clean dir

```bash
mkdir /tmp/sketchy-grader-sim && cd /tmp/sketchy-grader-sim
git clone -b submission --depth 1 \
  https://github.com/CaioDGallo/rinha-de-backend-2026-sketchy.git .
docker compose up -d
sleep 8
curl -fsS http://localhost:9999/ready -o /dev/null -w "%{http_code}\n"  # → 200
docker compose down
```

If this works end-to-end, the grader will be happy.

---

## Step 4 — PR our participant JSON into the official repo

This step adds **one tiny JSON file** to the contest repo so the grader knows
our submission exists. The file lives at `participants/CaioDGallo.json`.

```bash
# 1. Fork the official repo (one-time)
gh repo fork zanfranceschi/rinha-de-backend-2026 --clone=false --remote=false

# 2. Get a fresh clone of YOUR fork
cd /tmp
git clone https://github.com/CaioDGallo/rinha-de-backend-2026.git rinha-fork
cd rinha-fork

# 3. Branch & add the file
git checkout -b posthog-code/add-caio-sketchy main
cat > participants/CaioDGallo.json <<'JSON'
[
  {
    "id": "sketchy",
    "repo": "https://github.com/CaioDGallo/rinha-de-backend-2026-sketchy"
  }
]
JSON

# 4. Commit & push
git add participants/CaioDGallo.json
git commit -m "Add CaioDGallo participant entry"
git push -u origin posthog-code/add-caio-sketchy

# 5. Open the PR against the official repo
gh pr create \
  --repo zanfranceschi/rinha-de-backend-2026 \
  --base main --head CaioDGallo:posthog-code/add-caio-sketchy \
  --title "Adiciona inscrição do participante CaioDGallo" \
  --body "Submitting sketchy: Go + cgo/AVX2 fraud detection backend.

---
*Created with [PostHog Code](https://posthog.com/code?ref=pr)*"
```

The maintainer reviews and merges. Recent precedent: PRs adding a single
participant JSON have been merged within a day or two (see the recent
commit history of the contest repo).

---

## Step 5 — Trigger a preview test (`rinha/test` issue)

Once the PR is merged:

```bash
gh issue create \
  --repo zanfranceschi/rinha-de-backend-2026 \
  --title "rinha/test sketchy — CaioDGallo" \
  --body "rinha/test sketchy"
```

Rinha's engine polls open issues with `rinha/test` in the description. It
finds ours, runs the load test against our submission, posts the JSON result
as an issue comment, and closes the issue. Expected ~5–10 minutes per cycle.

We can rerun by opening another issue at any time.

---

## Step 6 — Read the result, iterate

The comment posted on our issue will look like:

```json
{
  "expected": { "total": 5000, "fraud_count": 1750, ... },
  "p99": "X.XXms",
  "scoring": {
    "breakdown": { "true_positive_detections": ..., "http_errors": ... },
    "p99_score": { "value": ..., "cut_triggered": false },
    "detection_score": { "value": ..., "cut_triggered": false },
    "final_score": ...
  }
}
```

Locally we measured **5399.31 / 6000 under amd64 emulation**. On the contest
host (a real Linux/amd64 Mac Mini Late 2014, Haswell), expect the latency
component to gain another ~500 points just from removing the emulation tax.

If the result is below expectations, iterate:

1. Push a new image: `caiodgallo/sketchy-api:0.1.1`.
2. Bump the tag in `docker-compose.yml` on the `submission` branch and push.
3. Open a new `rinha/test` issue.

The participant JSON does not need to change between iterations.

---

## Checklist

- [ ] Source repo created & `main` pushed
- [ ] Public Docker image pushed
- [ ] `submission` branch created with `docker-compose.yml`, `nginx.conf`, `info.json`
- [ ] Submission branch verified locally with `docker compose up`
- [ ] PR with `participants/CaioDGallo.json` opened against the contest repo
- [ ] PR merged
- [ ] `rinha/test` issue opened
- [ ] Result posted; final_score recorded

---

## Things that bite

- **`build:` instead of `image:` in submission compose.** The grader's clone
  has no source code, so a `build:` block fails fast. Use `image:` with a
  publicly pullable tag.
- **Private registry.** GHCR packages default to *private* — you must flip
  the visibility to public after the first push.
- **ARM-only image.** Contest host is amd64. If you're on Apple Silicon,
  `docker buildx build --platform linux/amd64 …` (we already pin this in our
  `docker-compose.yml`'s `build` block, but verify the pushed image too).
- **Resource caps.** `deploy.resources.limits` is enforced. The compose v3
  schema also accepts `cpus:`/`mem_limit:` at top-level for some setups; we
  use the `deploy:` form which is the one the spec shows.
- **Stale UDS file.** If a previous grader run leaves a `*.sock` in the
  shared volume, our server's startup `os.Remove(udsPath)` handles it. (Verified.)
- **Two runs in parallel.** The grader serializes runs per repo, so this is
  not a concern.
