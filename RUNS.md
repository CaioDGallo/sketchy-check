# Submitting Test Runs

This is the **iteration loop** — what to do every time you want to push a new
version of sketchy and have the Rinha grader score it. For the **one-time
setup** (creating the source repo, the participant PR, etc.) see
[`SUBMISSION.md`](./SUBMISSION.md).

---

## TL;DR — the four-line iteration

```bash
docker login                                                   # if logged out
make ship VERSION=0.1.1                                        # build, push, bump submission
gh issue create --repo zanfranceschi/rinha-de-backend-2026 \
  --title "rinha/test sketchy-check" \
  --body  "rinha/test sketchy-check"
```

The `make ship` target is defined below. The issue body must be **exactly**
`rinha/test sketchy-check` — no extra lines, no markdown, no links. The grader
parses everything after `rinha/test ` as the submission `id` and silently
fails (and closes the issue) if it can't match.

---

## What the grader does, end-to-end

1. Polls open issues on `zanfranceschi/rinha-de-backend-2026` every ~30 s.
2. Finds an issue whose **body** starts with `rinha/test`.
3. Reads `participants/<your-github-username>.json`, looks up the `id` from
   the issue body, and resolves the submission's repo URL.
4. Clones the `submission` branch of that repo.
5. Pulls the public Docker image referenced by `docker-compose.yml`.
6. Runs `docker compose up -d`, waits for `GET :9999/ready` (up to 20 retries
   × 3 s = 60 s).
7. Runs `k6 run test/test.js` for ~120 s with 0→900 RPS ramp.
8. Posts the JSON `results.json` as a **comment** on the issue.
9. Closes the issue.

Total cycle ≈ 4–6 minutes per run.

The grader is the source of truth — local `k6 run` is a useful sanity check,
but the score that counts is the one in the issue comment.

---

## Step-by-step iteration

### 1. Make the change

Edit code locally on `main`. Run `cd api && go build ./... && go vet ./...`
to verify the code compiles, then `cd .. && make smoke` to spot-check.

### 2. Commit on `main`

```bash
git add -A
git commit -m "Tweak X to do Y"
git push origin main
```

The grader doesn't read `main` — but pushing keeps the source in sync with
what's actually being deployed.

### 3. Build and push a new image tag

Pick a version. Increment the patch level for tweaks, the minor for new
techniques, the major for big rewrites. We started at `0.1.0`.

```bash
# from sketchy-check root, one-time per session: log in to Docker Hub
docker login

# build a fresh local image (uses the cgo+AVX2 path in the Dockerfile)
make resources                           # stages the index.bin into build_resources/
docker compose build api1                # any of the api services rebuilds the image

# tag and push
VERSION=0.1.1
docker tag sketchy-api:local caiogallo2401/sketchy-check:${VERSION}
docker push                  caiogallo2401/sketchy-check:${VERSION}
```

### 4. Bump the submission branch

The submission branch's `docker-compose.yml` has the image tag pinned. Bump
it so the grader pulls the new image.

```bash
git fetch origin submission
git checkout submission
sed -i '' "s|caiogallo2401/sketchy-check:[0-9.]*|caiogallo2401/sketchy-check:${VERSION}|" \
    docker-compose.yml
git diff docker-compose.yml             # sanity check the diff
git commit -am "Bump image to ${VERSION}"
git push
git checkout main
```

### 5. Trigger a grader run

```bash
gh issue create \
  --repo zanfranceschi/rinha-de-backend-2026 \
  --title "rinha/test sketchy-check"  \
  --body  "rinha/test sketchy-check"
```

**Critical**: the issue **body** must be exactly `rinha/test sketchy-check`.
The title can be anything (other participants put extra context like
`memsplit` there); the body cannot. The grader treats the body as
`rinha/test <id>` and looks `<id>` up in our `participants/CaioDGallo.json`,
which currently lists `id: sketchy-check`.

### 6. Read the result

Watch the issue. The grader posts a comment within ~5 minutes:

```json
{
  "expected": { "total": 5000, "fraud_count": 1750, ... },
  "p99": "X.XXms",
  "scoring": {
    "breakdown": {
      "true_positive_detections":  ...,
      "true_negative_detections":  ...,
      "false_positive_detections": ...,
      "false_negative_detections": ...,
      "http_errors":               ...
    },
    "p99_score":       { "value": ..., "cut_triggered": false },
    "detection_score": { "value": ..., "cut_triggered": false },
    "final_score":     ...
  }
}
```

Reading guide:

- **`final_score`** is the headline number, max 6000.
- **`p99_score`** caps at 3000 when `p99 ≤ 1 ms`. Floor of `−3000` if `p99 > 2000 ms`.
- **`detection_score`** caps at 3000 with zero errors. Floor of `−3000` if `(FP+FN+Err) / total > 15 %`.
- **`http_errors`** > 0 likely means the API container OOM-killed, the image failed to pull, or the UDS path is wrong.

---

## A canned `make ship` target

Add this to `Makefile` (or run the steps manually) — it bundles step 3 + 4:

```makefile
ship: resources
	@if [ -z "$(VERSION)" ]; then echo "set VERSION=x.y.z"; exit 1; fi
	docker compose build api1
	docker tag sketchy-api:local caiogallo2401/sketchy-check:$(VERSION)
	docker push caiogallo2401/sketchy-check:$(VERSION)
	git fetch origin submission
	git worktree add /tmp/sketchy-submission submission || true
	cd /tmp/sketchy-submission && \
		sed -i '' "s|caiogallo2401/sketchy-check:[0-9.]*|caiogallo2401/sketchy-check:$(VERSION)|" \
			docker-compose.yml && \
		git commit -am "Bump image to $(VERSION)" && \
		git push origin submission
	git worktree remove /tmp/sketchy-submission
	@echo
	@echo "Pushed caiogallo2401/sketchy-check:$(VERSION) and bumped submission branch."
	@echo "Now open a grader run:"
	@echo "  gh issue create --repo zanfranceschi/rinha-de-backend-2026 \\"
	@echo "    --title \"rinha/test sketchy-check\" --body \"rinha/test sketchy-check\""
```

Usage: `make ship VERSION=0.1.1`.

---

## Local sanity check (optional but recommended)

Before opening a grader run, validate locally:

```bash
make up                             # builds + starts the full stack
make smoke                          # both DETECTION_RULES examples
k6 run ../rinha-de-backend-2026/test/test.js   # full 120 s load test
cat ../rinha-de-backend-2026/test/results.json | jq .scoring.final_score
make down
```

The local k6 score under amd64 emulation will be lower than the native
Linux/amd64 grader run (emulation tax on AVX2 ≈ 3–5×), but the **detection
score** will match. If detection drops below 3000 locally, fix it before
shipping — the grader uses the same brute-force-k=5 reference labels.

---

## Common gotchas

- **Issue body has any extra characters** → grader replies with
  `could not read submission info from id "<your-whole-body>"` and closes
  the issue. Reopen a fresh one with the body set to literally
  `rinha/test sketchy-check`.
- **Image tag in `submission/docker-compose.yml` doesn't exist on Docker Hub**
  yet → grader hangs on `docker pull` and times out. Always push the image
  *before* pushing the bump on the submission branch.
- **Image only built for `linux/arm64`** → grader is amd64. Always
  `docker tag` from the build that explicitly targeted `linux/amd64`. Our
  Dockerfile and `docker-compose.yml` already pin this; verify with
  `docker manifest inspect <image>`.
- **`docker login` token expired** → push fails silently or returns 401. Run
  `docker login` again.
- **Forgot to push the source repo** → the submission still works (the
  grader only reads the `submission` branch), but the source-of-truth on
  `main` drifts. Easy to forget, easy to fix.
- **Wanted to fix the body of an already-failed issue** → the grader closes
  the issue after one parse attempt. `gh issue edit` on a closed issue does
  nothing useful. Open a new issue instead.

---

## Multiple submissions on one PR

`participants/CaioDGallo.json` is an array — you can register additional
submissions later without re-PRing if you want to test variants:

```json
[
  { "id": "sketchy-check", "repo": "https://github.com/CaioDGallo/sketchy-check" },
  { "id": "sketchy-rust",  "repo": "https://github.com/CaioDGallo/sketchy-rust"  }
]
```

A new entry needs a new PR to `participants/`, but you can swap which one is
active per run by changing the `<id>` after `rinha/test` in the issue body.

---

## Quick reference

| What | Where |
|---|---|
| Source code | `https://github.com/CaioDGallo/sketchy-check` (`main`) |
| Submission compose | same repo, `submission` branch |
| Public image | `caiogallo2401/sketchy-check:<version>` (Docker Hub) |
| Participant JSON | `zanfranceschi/rinha-de-backend-2026/participants/CaioDGallo.json` |
| Grader trigger | issue on `zanfranceschi/rinha-de-backend-2026` with body `rinha/test sketchy-check` |
| Local benchmark | `k6 run ../rinha-de-backend-2026/test/test.js` |
