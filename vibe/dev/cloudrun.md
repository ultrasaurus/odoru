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
- **Cost**: $1.3148/hour, vs the $0.25–0.50/hour we typically pay on RunPod
  — roughly 3–4× more per hour.
- **Break-even**: it needs to be ~3–4× faster per segment to match RunPod's
  cost-per-segment; given Blackwell vs RTX 3090/A40, that is plausible.
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
- cost-per-segment vs RunPod at $0.25–0.50/hr — **closer than it looked,
  still not a clean win on a single-job basis**. At $1.3148/hr vs RunPod's
  $0.25–0.50/hr (2.6–5.3x the hourly rate) offset by a ~2x speed advantage,
  Blackwell's cost-per-segment lands around **1.3–2.6x RunPod's** (using
  RunPod RTF ~1.0; if RunPod is actually running its best-case 0.29–0.40
  RTF per `quirks.md`, Blackwell's relative cost is worse, not better).
  Better than the earlier ~3–4x estimate, but still a premium, not a win.

So: single-job Blackwell is a clear technical win (fastest synth path we've
measured, ~2x RunPod) but still costs more per segment than RunPod at
list price. Two ways the calculus could still favor Blackwell:
- **Parallel jobs** exploiting the 96 GB (~10 GB used per VibeVoice-1.5B
  job leaves room for many concurrent jobs on one instance) — at N≈4-8
  concurrent jobs per instance, cost-per-segment could drop well below
  RunPod's. This is the natural next step given steady-state numbers hold.
- **Operational value** (serverless scale-to-zero, no RunPod pod
  start/stop lifecycle, durable job state) independent of raw cost.

Listen test on the corrected-voice batch (seg13–18, 15 segments total so
far): quality good.

## Parallel-job support (Stage 1 implemented, N=2 tested)

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

### Results so far (N=2)

VRAM is not the constraint: two concurrent jobs peaked around 12.7 GiB
combined, far under the 96 GiB budget — plenty of headroom for more than
2 concurrent jobs on memory alone.

RTF degrades under concurrency, but throughput still wins:

| Scenario | RTF | Notes |
|---|---|---|
| Solo, warm instance | ~0.475–0.520 | baseline (no concurrency) |
| Solo, cold instance | 1.392–2.459 | first request after idle/fresh deploy; model load + CUDA/cuDNN warmup overlap with generation — not a concurrency effect |
| 2 concurrent, one cold | 0.835–2.459 | seg20/21 pair (0.835/1.095) and seg23/24 pair (1 job hit 2.459) — cold-start contamination, not steady-state |
| 2 concurrent, both warm | 0.81–0.89 | seg22 pair (0.881/0.887), seg26/27 pair (0.809/0.839) |

Once both jobs are warm, 2-way concurrency costs roughly 60–70% more RTF
per job than solo (~0.85 vs ~0.5), but completes 2 jobs in ~33s instead of
~60s sequential (2× solo) — a real net throughput win. Cold-start
contamination (RTF 2+) is a measurement artifact of testing on a fresh
instance, not a concurrency cost — **always warm the instance with one
solo request before timing a concurrency test** (see the pattern in
`dev/artifact-augment.md`).

### Next

Move to N=4 per the `dev/parallel.md` ramp (2 → 4 → 8), watching for
whether RTF degradation worsens disproportionately (sign of real GPU
contention, not just warmup noise) or holds roughly flat. If model-load
redundancy becomes a meaningful fraction of wall time at higher N, that's
the trigger for Stage 2 (persistent model server) in `dev/parallel.md`.
