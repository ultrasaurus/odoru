# Cloud Run GPU evaluation

vibe-service runs on both RunPod (primary, performance path) and Google
Cloud Run (serverless, scale-to-zero). This documents what we learned
evaluating Cloud Run's GPU options for VibeVoice synthesis.

The durable job-state work (`gcs-job-state.md`) is independent of all this
and works on Cloud Run regardless — ambient GCS auth via the metadata
server is proven. The open question here is purely whether Cloud Run's GPUs
are a viable *synthesis* target vs RunPod.

## Build / deploy

See `dev/setup.md` for the `Dockerfile.cloudrun` build + `gcloud run deploy`
commands. Cloud Run specifics that bit us are below.

## Option 1: NVIDIA L4 (24 GB) — works, but not competitive

Status: **end-to-end working, but too slow to be the synth target.**

- **VRAM is fine.** L4 has 24 GB, which meets our documented minimum (the
  artifact/hallucination problems were the 16 GB RTX A4000, not 24 GB).
- **Too slow.** Observed synth RTF ~2.4–3.0 (e.g. a 180-word segment:
  wall 139.6 s, RTF 2.43) vs ~1.0 on RunPod's RTX cards (0.29–0.40 on the
  24 GB cards per `quirks.md`). Roughly 3× slower.
- **Flash Attention does not work on the NGC base.** VibeVoice requests
  `flash_attention_2` on CUDA. The Cloud Run base is `nvcr pytorch 24.05`,
  which ships NVIDIA's patched torch `2.4.0a0`. Prebuilt flash-attn wheels
  are built against *stable* torch 2.4.0, so the `.so` fails to load with
  `undefined symbol: _ZNK3c105Error4whatEv` (`c10::Error::what()`). Worse:
  because `transformers` auto-imports `flash_attn` when it's installed, that
  import error **crashes synth entirely** — not a graceful SDPA fallback.
  So on this base flash-attn must be **source-built** against the in-image
  torch (`pip install flash-attn --no-build-isolation`, ~20–40 min compile),
  which we did not pursue given L4's other limits. L4 therefore runs synth
  on SDPA (slower, and per VibeVoice's own warning, less-tested quality).
- **CUDA forced-alignment crashes on L4.** The candle alignment kernels are
  compiled with the CUDA 12.4 toolchain; Cloud Run's L4 host driver is too
  old to accept that PTX → `DriverError(CUDA_ERROR_UNSUPPORTED_PTX_VERSION)`,
  which makes `/transcript` and `/report` 404. Fix: run alignment on CPU via
  `FORCED_ALIGNMENT_DEVICE=cpu` (forced-alignment v0.2.1 honors this even
  with the cuda feature compiled in). `Dockerfile.cloudrun` bakes this ENV
  in. VibeVoice synth still uses the GPU — only the Rust aligner moves to
  CPU. (RunPod leaves the var unset and auto-detects CUDA; its newer driver
  accepts the 12.4 PTX.)

Net: L4 proves the Cloud Run *plumbing* (durable state, ambient auth,
CPU alignment) but is ~3× too slow for synth and can't easily use flash
attention. Not the synth target.

For reference:

```
source vibe/.env
VERSION=v5
docker build --platform=linux/amd64 -f vibe/Dockerfile.cloudrun -t vibe-cloudrun:latest .
docker tag vibe-cloudrun:latest  us-central1-docker.pkg.dev/$PROJECT/vibe/vibe-cloudrun:$VERSION
docker push us-central1-docker.pkg.dev/$PROJECT/vibe/vibe-cloudrun:$VERSION
```

```
gcloud run deploy vibe-cloudrun \
  --image us-central1-docker.pkg.dev/$PROJECT/vibe/vibe-cloudrun:$VERSION \
  --region us-central1 \
  --gpu 1 --gpu-type nvidia-l4 \
  --no-gpu-zonal-redundancy \
  --cpu 4 --memory 16Gi \
  --no-cpu-throttling \
  --concurrency 1 \
  --min-instances 0 \
  --set-env-vars VIBE_SERVICE_SECRET=$VIBE_SERVICE_SECRET,GCS_BUCKET=vibe-jobs-a4127f08
```


## Option 2: NVIDIA RTX Pro 6000 Blackwell (96 GB) — to evaluate

Cloud Run also offers the RTX Pro 6000 Blackwell. Worth testing because it
is far more powerful and its 96 GB VRAM opens up parallelism.

- **Hardware**: 96 GB GDDR7.
- **Instance requirements**: min 20 vCPU / 80 GiB memory (up to 44 vCPU /
  176 GB).
- **Cost — GPU is not the whole bill.** GPU itself is $1.3148/hour, but
  Cloud Run bills CPU and memory separately at $0.000024/vCPU-sec and
  $0.0000025/GiB-sec, and the RTX Pro 6000 *requires* at least 20 vCPU /
  80 GiB attached. That's $1.728/hr (CPU) + $0.72/hr (memory) — **$2.448/hr
  on top of the GPU**, i.e. CPU+memory alone cost almost twice the GPU and
  make up ~65% of the real total. **True instance cost: ~$3.76/hour**, not
  $1.31/hour. This is fixed by the GPU's minimum vCPU/memory requirement —
  it doesn't shrink with concurrency, since `MAX_CONCURRENT_JOBS` doesn't
  change the instance's CPU/memory allocation.
- **Break-even revised accordingly**: vs RunPod's $0.25–0.50/hr, the real
  hourly gap is ~7.5–15x, not ~3–4x. See the Results/cost sections below
  for what this means once parallelism is factored in — short version: it
  doesn't close the gap.
- **Upside beyond speed**: 96 GB VRAM (vs ~10 GB used per VibeVoice-1.5B
  inference) could run **many jobs in parallel** on one instance, which
  changes throughput-per-dollar substantially. If the single-job numbers
  look good, parallel synth is the follow-up worth the engineering (relates
  to the parked Cloud Run Jobs / N-parallel-segments idea in
  `gcs-job-state.md`).

### Dependency upgrades required for Blackwell

RTX Pro 6000 Blackwell is compute capability **sm_120**, which needs **CUDA
12.8+**. Our current images are CUDA 12.4 and will not run on it. Expected
changes (to be confirmed during the build):

- **New base image** with CUDA 12.8+ and a torch build carrying sm_120
  kernels (e.g. a newer `nvcr pytorch` 25.x, or a `cu128` PyTorch ≥ 2.7).
- **torch / transformers / VibeVoice** compatibility re-checked against the
  newer torch (VibeVoice is pinned to a commit; verify it still imports).
- **flash-attn** rebuilt for sm_120 / cu128 (prebuilt wheels may not exist
  yet for Blackwell — possibly another source build).
- **forced-alignment**: keep `FORCED_ALIGNMENT_DEVICE=cpu` to start (avoids
  needing the candle CUDA kernels rebuilt for sm_120). Revisit CUDA
  alignment later if the driver accepts a 12.8-built PTX.

### Build / deploy (viability test)

Build/deploy commands live in `dev/setup.md` (the canonical home for active
build commands): image `Dockerfile.cloudrun-blackwell`, tag
`vibe-cloudrun-bw`, deployed with the RTX Pro 6000 GPU type and 20 vCPU /
80 GiB. The L4 `Dockerfile.cloudrun` stays as a working fallback. The
Blackwell binary is built CPU-only, so no `FORCED_ALIGNMENT_DEVICE` is
needed.

### Results

| Build | Synth RTF | Synth wall | Align (CPU) | Notes |
|---|---|---|---|---|
| v1, SDPA | 1.476 | 98.2s | 18.5s | First working Blackwell run; no flash-attn |
| v2, flash-attn, 1st job | 0.920 | 64.1s | 20.1s | First job after deploy — likely cold-start (cuDNN/kernel autotuning) |
| v2, flash-attn, steady-state (n=14) | **0.475–0.670, avg ~0.53** | 23.0–46.6s | 7.1–20.1s | augment_seg13–18, varied lengths (137–184 words); one segment (seg13) re-run twice |

flash-attn took the GPU from ~1.5x slower than RunPod to, once warmed up,
**roughly half RunPod's typical RTF (~1.0)**. `flash_attn_2_cuda` imported
and executed correctly — no crash, no fallback. The 0.920 first-job sample
looks like one-time warmup cost, not representative of steady throughput;
all 14 subsequent jobs clustered tightly in 0.475–0.670 regardless of
segment length. Alignment (CPU) scaled with segment length as expected
(7.1s–20.1s); a few segments flagged 1–2 alignment "suspects" (review-worthy,
not failures) — none filtered/errored.

One run (seg14–18, first pass) used the wrong voice (forgot to set it before
running) — timing data is still valid and included above, but that batch's
audio isn't the one to listen-test; seg13–18 second pass has the correct
voice.

**Surprise**: `pip install flash-attn --no-build-isolation` did **not**
source-compile despite the flag — install took 1.3s and the resulting
`flash_attn_2_cuda.cpython-312-…so` carries a March 2025 mtime, meaning pip
silently grabbed a matching prebuilt PyPI wheel and skipped the sdist build
entirely. `TORCH_CUDA_ARCH_LIST=12.0` had no effect since nothing compiled.
This worked out fine here — the wheel's ABI happens to match this NGC
torch (2.7.0a0) and its kernels run correctly on sm_120 — but it means the
Dockerfile comment claiming a "source build" is currently inaccurate; the
actual mechanism is "whatever pip resolves," verified empirically rather
than guaranteed. Revisit if a future torch/transformers bump changes which
wheel pip picks.

### Decision criteria

Measure on a representative segment set (varied cfg-scale / seed / speed):
- synth RTF (target: ≥3–4× the L4, i.e. roughly RunPod-class or better) —
  **met, comfortably**: steady-state ~0.53 avg, roughly **2x faster** than
  RunPod's typical ~1.0 RTF (using the single early 0.920 sample
  understated this — see Results above).
- cost-per-segment vs RunPod at $0.25–0.50/hr — **not met, and worse than
  first estimated.** The true Blackwell instance cost is **~$3.76/hr**
  (GPU $1.3148 + the *required* 20 vCPU/80 GiB billed separately at
  ~$2.448/hr — see Option 2 above), not the $1.3148/hr GPU-only price used
  in earlier passes of this doc. Against RunPod's $0.25–0.50/hr (7.5–15x
  the hourly rate) offset by only a ~2x speed advantage, Blackwell's
  cost-per-segment lands around **3.8–7.5x RunPod's** on a single job —
  worse than every earlier estimate in this doc, which didn't account for
  CPU/memory billing.

So: single-job Blackwell is a clear technical win (fastest synth path we've
measured, ~2x RunPod) but costs substantially more per segment than RunPod
at list price — more than initially estimated, once CPU/memory billing is
included. Two ways the calculus could still favor Blackwell:
- **Parallel jobs** exploiting the 96 GB (~10 GB used per VibeVoice-1.5B
  job leaves room for many concurrent jobs on one instance) — the CPU/
  memory cost is fixed regardless of `MAX_CONCURRENT_JOBS`, so parallelism
  at least amortizes that fixed cost across more segments. **Update after
  N=4/N=8 testing, with CPU/memory included: it helps, but doesn't come
  close to closing the gap** — see the cost table in the Parallel-job
  section below.
- **Operational value** (serverless scale-to-zero, no RunPod pod
  start/stop lifecycle, durable job state) independent of raw cost.

Listen test on the corrected-voice batch (seg13–18, 15 segments total so
far): quality good.

## Parallel-job support (Stage 1 implemented, N=2/4/8 tested)

Given the cost math above, running multiple jobs concurrently on one
Blackwell instance is the natural next lever — at N≈4–8 concurrent jobs,
cost-per-segment could drop well below RunPod's. Full design and ramp
plan: `dev/parallel.md`.

### What's implemented

Stage 1 from `dev/parallel.md`: a `tokio::sync::Semaphore` gates how many
synth subprocesses run at once, sized from `MAX_CONCURRENT_JOBS` (env var,
default 1). The semaphore is acquired inside the spawned `run_job` task —
not in the `POST /jobs` handler — so a job waiting for a free slot stays
`Pending` rather than looking stuck. `AppState.jobs`/the GCS `JobStore`
already supported multiple in-flight jobs keyed by `job_id`, so no
bookkeeping change was needed there. Per-job model loading is unchanged
(still a fresh `python3` subprocess per job, loading VibeVoice from
scratch each time) — that redundant-load cost is the trigger for Stage 2
(persistent model server) if it turns out to matter; see `dev/parallel.md`.

Also added: `HEARTBEAT_SECS` (env var, default 60) controls how often a
running job logs a heartbeat with current VRAM (`nvidia-smi
memory.used,memory.total`) — lowered to 10 for the tests below since
these segments finish well under the default 60s interval.

Deploy wires both through `--set-env-vars`; `--concurrency` is set to
match `MAX_CONCURRENT_JOBS` so Cloud Run actually routes that many
requests to one instance. See `dev/setup.md`.

### Results so far (N=2, N=4, N=8)

VRAM is not the constraint at any tested N: combined usage scaled
roughly linearly with N (~6.3 GiB/job) — 12.7 GiB at N=2, 25.5 GiB at
N=4, ~51 GiB at N=8 — all comfortably under the 96 GiB budget. Headroom
on memory alone would support going well past N=8.

RTF degrades under concurrency at every step:

| Scenario | RTF | Notes |
|---|---|---|
| Solo, warm instance | ~0.475–0.520 | baseline (no concurrency) |
| Solo, cold instance | 1.392–2.459 | first request after idle/fresh deploy; model load + CUDA/cuDNN warmup overlap with generation — not a concurrency effect |
| 2 concurrent, one cold | 0.835–2.459 | seg20/21 pair (0.835/1.095) and seg23/24 pair (1 job hit 2.459) — cold-start contamination, not steady-state |
| 2 concurrent, both warm | 0.81–0.89 | seg22 pair (0.881/0.887), seg26/27 pair (0.809/0.839) |
| 4 concurrent, all warm | 1.14–1.39 | seg31/13/29/30 — see breakdown below |
| 8 concurrent, all warm | 2.03–2.69 | seg33–40 — see breakdown below |

N=4 breakdown (seg31 started first, finished fastest; the rest stretched
out as contention increased — classic GPU-scheduling fairness pattern):

| Segment | Wall | RTF |
|---|---|---|
| seg31 | 55.8s | 1.391 |
| seg13 | 74.7s | 1.187 |
| seg29 | 87.3s | 1.143 |
| seg30 | 89.2s | 1.150 |

N=8 breakdown (same fairness pattern, more pronounced):

| Segment | Words | Wall | RTF |
|---|---|---|---|
| seg40 | 82 | 86.9s | 2.693 |
| seg33 | 114 | 107.5s | 2.512 |
| seg36 | 147 | 110.2s | 2.474 |
| seg38 | 184 | 138.3s | 2.105 |
| seg35 | 182 | 148.1s | 2.358 |
| seg37 | 214 | 150.6s | 2.103 |
| seg34 | 207 | 154.4s | 2.032 |
| seg39 | 198 | 154.5s | 2.066 |

All jobs at each N entered `Running` within tens of milliseconds of each
other (semaphore let all N through at once, confirmed via `job
concurrency limit max_concurrent_jobs=N` in the logs) and ran
concurrently for their full duration (overlapping `gpu_mem` heartbeats
throughout) — this is real GPU compute contention, not cold-start noise.
Cold-start contamination (RTF 2+ on a single solo job) is a separate,
measurement-only artifact of testing on a fresh instance — **always warm
the instance with one solo request before timing a concurrency test**
(see the pattern in `dev/artifact-augment.md`).

**Throughput** (the metric that actually matters: audio-seconds produced
per wall-clock second, computed as Σ(wall/RTF) ÷ makespan — this
normalizes for segments of different lengths, unlike comparing raw
wall-clock times):

| N | Throughput | Marginal gain over previous N |
|---|---|---|
| Solo | ~2.0x | — |
| N=2 | ~2.4x | +0.4x |
| N=4 | ~2.9x | +0.5x |
| N=8 | ~3.0x | +0.1x |

The curve is flattening hard but **did not reverse** — N=8 is still a
net throughput win over N=4, just barely. An earlier draft of this doc
predicted N=8 might net out *worse* than N=4; that was wrong, and is
corrected here based on the actual N=8 data. The practical takeaway:
N=4 already captures most of the available throughput gain on this
workload/hardware; N=8 buys very little extra and costs much higher
per-job RTF, so N=4 is the more practical operating point unless the
marginal 0.1x matters for your use case.

**Alignment (CPU, `spawn_blocking`) holds up even under genuine 4-way
overlap** — resolving the open question from the N=4 test, which only
exercised a 2-way overlap. At N=8, alignment for seg35/37/34/39
overlapped 4 ways simultaneously (~17:59:29–17:59:34). Normalized by
word count, those four ran at 0.064–0.075 s/word — within the normal
*solo* alignment variance seen throughout this doc (0.060–0.110 s/word).
No detectable CPU contention even at true 4-way concurrency.

**Cost-per-segment at each N — corrected for CPU/memory billing.** The
$1.3148/hr figure used in earlier passes of this analysis is the GPU
*alone*. The RTX Pro 6000 requires a minimum 20 vCPU / 80 GiB attached,
billed separately ($0.000024/vCPU-sec, $0.0000025/GiB-sec) — that's
**$1.728/hr CPU + $0.72/hr memory = $2.448/hr**, on top of the GPU, for a
**true instance cost of ~$3.76/hr**. Unlike the GPU's per-job throughput
gain, this CPU/memory cost is fixed per instance regardless of
`MAX_CONCURRENT_JOBS` — so parallelism amortizes it across more segments,
but it never goes away.

| N | Throughput | Blackwell cost-per-audio-sec vs RunPod |
|---|---|---|
| Solo | 2.0x | 3.76x–7.53x more expensive |
| N=4 | 2.9x | **2.60x–5.19x** |
| N=8 | 3.0x | 2.51x–5.02x |

This is the real number, and it reverses the earlier (GPU-only) read of
"roughly parity." Even at N=8 — past the point of diminishing throughput
returns — Blackwell costs **2.5x to 5x more per segment than RunPod**,
not "roughly parity." Parallelism helps (it nearly halves the N=1 gap),
but CPU/memory billing dominates the total enough that no amount of
synth-side speedup or process-level concurrency closes it; that would
need either a fundamentally cheaper way to get the GPU without the
20 vCPU/80 GiB minimum, or RunPod becoming meaningfully more expensive
than its current $0.25–0.50/hr. Whether Blackwell is still "worth it"
now rests entirely on operational value (serverless, durable state) —
not on cost.

**Caveat: the table above assumes RunPod runs at RTF≈1.0.** `quirks.md`
documents RunPod hitting RTF 0.29–0.40 in its best case (throughput
2.5x–3.45x, not 1.0x). If RunPod is actually running at that best case
rather than RTF≈1.0, the gap is meaningfully worse than the table shows
— at N=8, recomputing against RunPod's best-case RTF range gives roughly
**6.3x–17.3x**, not 2.5x–5.0x. The 2.5x–5.0x figures above are a
floor, not a typical case; whether they're realistic depends on which
RunPod RTF this workload actually sees in practice (see `quirks.md` for
what's been measured there).

### Next

N=4 looks like the practical sweet spot for raw process-level
concurrency on this workload — N=8 is a real but marginal throughput
gain (+0.1x) for a much higher per-job RTF cost. Going further (N=16+)
is not expected to help based on this flattening curve, and VRAM
headroom (still ample at ~51 GiB/8 jobs) was never the actual
constraint — GPU compute is.

Important distinction for whatever comes after that: **the N=4 result is
GPU compute contention** (N concurrent forward passes time-slicing the
same SMs/tensor cores), not redundant model-loading overhead. Stage 2 in
`dev/parallel.md` (a persistent model server that loads VibeVoice once
and serves many jobs) only removes the *load* cost — it does nothing for
compute contention, since the GPU still has to do N times the matmul
work in roughly the same window whether each job has its own loaded copy
or they share one resident model. Stage 2 is still worth doing
eventually (the redundant load time is real, measurable waste), but it
won't move the RTF-under-concurrency numbers measured here.

The lever that actually addresses compute contention is **true request
batching** — combining N requests into a single batched forward pass
(one matmul over a batch dimension) instead of N independent ones
time-slicing the GPU. That's a bigger change than Stage 2: it needs
VibeVoice's inference code to support batched generation, plus a
request-queueing layer to accumulate a batch before running it, and a
persistent model process is a prerequisite for it (you can't batch
across independent subprocesses). Worth scoping separately, and only if
the throughput numbers at N=4–8 don't already meet the cost target on
their own.
