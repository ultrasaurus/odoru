# Parallelizing vibe-service on a single GPU instance

Goal: run multiple synth jobs concurrently on one GPU instance (Cloud Run
Blackwell first; same code should apply to RunPod once its CUDA driver
issue is fixed — see `cloudrun.md` for why RunPod is currently blocked on
newer CUDA). Nothing here should be Cloud-Run-specific.

Correction to an earlier assumption: Cloud Run `--concurrency` is a
per-instance request-routing limit, not a multi-instance-only setting.
Raising it is what lets Cloud Run send more than one job to the same
instance at all — it's necessary for any of this to matter.

## Current architecture (baseline)

- Each job spawns its own `python3 inference_from_file.py` subprocess
  (`run_inference_inner`, `vibe/service/src/main.rs:609-673`), invoked from
  `run_job` (`main.rs:463`) at `main.rs:521`. `run_job` itself is
  `tokio::spawn`'d unbounded from the `POST /jobs` handler (`main.rs:180`).
- The Python subprocess loads the VibeVoice model fresh every job
  (`vibe/vv/demo/inference_from_file.py:279-332`, `device_map="cuda"`).
  No singleton/persistent model process exists.
- No concurrency limit anywhere. `ActivityTracker` (`watchdog.rs:20-26`)
  counts active jobs for idle-shutdown, not for limiting concurrency.
- Job state is in-memory (`HashMap`) plus GCS-backed durable store
  (`jobstore.rs`) — orthogonal to this work, already handles instance
  churn.
- Forced alignment runs CPU-side via `spawn_blocking` (`main.rs:403-441`,
  `run_alignment`) — not GPU-bound, not a concern for this plan.

## Stage 1 — semaphore-capped concurrent subprocesses

Keep the per-job subprocess model as-is. Add a gate so Cloud Run can
route N jobs to one instance and the Rust service runs up to N synth
subprocesses at once, no more.

- Add `tokio::sync::Semaphore` sized from `MAX_CONCURRENT_JOBS` env var
  (default 1, to preserve current single-job behavior if unset).
- Acquire a permit before spawning the synth subprocess in `run_job` /
  `run_inference_inner`; release on completion (drop).
- Env var only — no new HTTP surface, no queue beyond what Tokio/the
  semaphore already does (excess requests just wait for a permit).

### Where the wait happens

`POST /jobs` (`main.rs:161-185`) stays fire-and-forget: it inserts
`JobState::Pending`, fires `tokio::spawn(run_job(...))`, and returns
immediately — it does not touch the semaphore. The permit is acquired
inside the spawned `run_job` task, so it's that background task that
blocks, not the HTTP handler.

Consequence for the (N+1)th job submitted while all N permits are held:
its `run_job` task parks on `semaphore.acquire()` before transitioning
the job out of `Pending`. A client polling `GET /jobs/:id` during that
wait sees `Pending`, not `Running` and not stuck — it looks like a job
that hasn't started yet, which is true, rather than a hung job. This is
consistent with the existing stuck-running rule (a job stuck at
`Running` with no instance behind it is treated as dead); a job parked
on the semaphore is correctly still `Pending`, so it won't trip that
rule.
- Deploy with `--concurrency` set to match `MAX_CONCURRENT_JOBS` so Cloud
  Run actually forwards that many concurrent requests to the instance.

### Testing ramp (as planned, before results — see Stage 2 for what was found)

Original plan: test at N=2, then N=4, before trying N=8, each step
validating an assumption before committing to the next. The "~10 GB per
job" VRAM estimate and the idea of N=8 as "the stage-1 ceiling" below
were both pre-test assumptions — **both turned out wrong**: actual VRAM
use was ~6.3 GiB/job (never near the 96 GiB ceiling even at N=8, so VRAM
was never the real ceiling), and the actual constraint that emerged was
GPU compute contention, not memory. See Stage 2 for the corrected
picture; this subsection is kept as a record of the original reasoning,
not as current guidance.

1. **N=2**: confirm two jobs really run concurrently on GPU (not
   serialized somewhere unexpected), VRAM stays well under 96 GB, and
   per-job RTF doesn't degrade much vs. running solo.
2. **N=4**: same checks, watch for early signs of GPU contention
   (RTF creeping up) or CPU/alignment becoming a bottleneck now that
   more `spawn_blocking` alignment tasks run together.
3. **N=8** (target): if RTF still holds up and VRAM has headroom,
   this is the stage-1 ceiling per the "~10 GB per job" estimate
   (96 GB / ~10 GB ≈ 8-9, leaving margin for non-model overhead).

At each step, measure:
- Wall-clock per job (synth RTF) vs. solo baseline.
- VRAM usage (`nvidia-smi` during the run).
- Whether redundant model loading (every subprocess reloads weights)
  is a meaningful fraction of per-job wall time at this N — this is the
  key number that decides whether Stage 2 below is needed.

**Value if we stop here:** N-way parallelism on one instance, achieved
with one semaphore and one env var, zero Cloud-Run-specific code. Reusable
on RunPod immediately once its driver issue is fixed — just set the same
env var, no architecture change.

## Stage 2 — decided from Stage 1 data (N=2/4/8 on Blackwell)

Originally framed as "is redundant per-job model load time a meaningful
fraction of job time?" — that was never directly measured. It turned out
not to be the right question: the N=2/4/8 results (`cloudrun.md`,
"Parallel-job support" section) show RTF degrading smoothly with N in a
GPU-scheduling-fairness pattern (~1.0x solo → 1.14-1.39x at N=4 →
2.0-2.7x at N=8) while VRAM scales cleanly linear (~6.3 GiB/job, never
near the 96 GiB ceiling). That shape is **GPU compute contention** — N
independent forward passes time-slicing the same SMs — not redundant
model loading, which would show up as roughly constant per-job overhead
regardless of N, not a curve that tracks contention this smoothly.

`cloudrun.md` draws the conclusion directly: a persistent model server
removes redundant *load* cost (real, but it's not what's being measured
here) and does nothing for compute contention, since the GPU still has
to do N times the matmul work in roughly the same window whether each
job has its own loaded copy or shares one resident model. So the
decision is made, not pending: go to Stage 3, and Stage 3 has to be
batching, not just a persistent server, because a persistent server
alone wouldn't move any of the N=2/4/8 numbers above.

## Stage 3 — persistent model server + real batched generation

Originally scoped as "persistent server only" (eliminate redundant
model loads). Revised: a persistent server alone still runs N
independent batch-1 `generate()` calls side by side, each competing for
the same SMs — most of the GPU-underutilization problem from Stage 1
remains. `VibeVoiceForConditionalGenerationInference.generate()`
(`vv/vibevoice/modular/modeling_vibevoice_inference.py:391`) already
supports `batch_size > 1` natively (per-sample `finished_tags`,
`audio_chunks = [[] for _ in range(batch_size)]`), so real batching is
not extra model-side work, just plumbing. Doing persistent-server and
batching together is the right scope, not two sequential stages: most
of the lift (process lifecycle, health, error isolation) is shared
infrastructure either way, and stopping at "server only" leaves the
bigger win on the table for little savings in complexity.

- Replace the per-job subprocess spawn with a long-lived Python process
  that loads the model once and serves batched requests over a local
  socket/HTTP.
- Host-agnostic by construction — no Cloud Run or RunPod specifics.

**Max batch size is unknown and needs its own measurement — the
existing N=2/4/8 data doesn't answer it.** That data came from N
*independent processes*, each loading its own full copy of the model
weights; the ~6.3 GiB/job figure is mostly duplicated weights, not
per-item activation cost. Real batching loads weights once and only
the per-item KV-cache/activation memory (`past_key_values`, scaled by
`attention_mask`/sequence length —
`modeling_vibevoice_inference.py:382,402`) grows with batch size, which
should be much smaller per item than 6.3 GiB — but nothing has measured
that directly. It's also not purely a VRAM question: the N=2/4/8
results showed a *throughput* ceiling from compute contention (gains
flattening hard by N=8) separate from VRAM headroom, and a single fused
batched forward pass may hit a different kind of ceiling than N
processes competing for SMs. To find the real number: run an actual
batched `generate()` call (not subprocesses) at increasing batch size,
watching both `nvidia-smi` VRAM and per-item throughput at each step —
same methodology as the Stage 1 ramp, just on the batched code path.
Whatever max-batch number comes out is specific to the current
segment-length window (`max_new_tokens` and the KV-cache scale with
sequence length — `modeling_vibevoice_inference.py:373,421`); it would
need re-checking if the segmentation policy's length bounds ever
change.

`vibe/docker/batch_bench.py` implements this measurement: loads the
model once, sweeps a comma-separated list of batch sizes
(`--batch_sizes 1,2,4,8,16`), and for each size logs wall time, peak
VRAM (`torch.cuda.max_memory_allocated()`), and throughput
(audio-seconds produced ÷ wall-clock seconds — same definition used for
the N=2/4/8 throughput table in `cloudrun.md`) to
`batch_bench_runs.jsonl`. Stops escalating on CUDA OOM rather than
crashing the sweep, and saves one wav per batch item per size for a
quick listen-check that quality doesn't degrade under batching. COPYed
into both `Dockerfile` and `Dockerfile.cloudrun-blackwell` alongside
`inference_from_file.py`, with its sample segments (`bench_segments/`,
seg41-71, 31 total — enough for the default 1,2,4,8,16 sweep to use each
segment once, no repeats) baked in; not yet run on real GPU hardware (no CUDA available
outside the pod/instance) — running it is the actual next step, this
doc update is not a substitute for that data.

**Why Cloud Run is the priority target, despite no shell access.** Vibe
is on a 90-day Cloud Run eval, and the open question this whole doc is
chasing is whether Cloud Run can be made cheaper than RunPod (the
batching fix helps both, but the eval clock is on Cloud Run, so testing
there is the priority — RunPod stays a fallback that must keep working,
not a thing to actively build on). Cloud Run has no shell/exec into the
container at all, so `batch_bench.py` needed an HTTP trigger and a way
to ship results out — both now implemented:

- **`POST /bench`** (`service/src/main.rs`, `create_bench_handler`)
  takes `{batch_sizes, speaker, cfg_scale, seed}` (all optional, same
  defaults as `batch_bench.py`), spawns `demo/batch_bench.py` in the
  background (mirrors the existing `run_inference_inner` subprocess
  pattern), and returns `{request_id, gcs_prefix, log_url}` immediately.
  Progress is tailable via the existing `/log/:request_id` endpoint —
  no new polling mechanism needed.
- **GCS upload** (`batch_bench.py`, `upload_to_gcs`): when `GCS_BUCKET`
  is set (already is, for job-state durability — `jobstore.rs`) and a
  `--gcs_prefix` is given, the script uploads its output wavs and
  `batch_bench_runs.jsonl` to `gs://$GCS_BUCKET/<prefix>/` using ambient
  credentials (Cloud Run's metadata-server auth, already proven to work
  for `jobstore.rs`) — no service-account key needed. `/bench` always
  passes `--gcs_prefix bench/<request_id>` when `GCS_BUCKET` is set, so
  the response tells you exactly where to fetch results:
  `gcloud storage cp -r gs://$GCS_BUCKET/<gcs_prefix>/ ./local-dir/`.
- **RunPod path unaffected.** `batch_bench.py` still runs standalone
  over SSH with no GCS args (`python3 demo/batch_bench.py --batch_sizes
  1,2,4,8,16`, then plain `scp` of `--output_dir`) — the GCS upload is
  opt-in (only triggers when `--gcs_bucket`/`GCS_BUCKET` is set), and
  `/bench` is an additive route, nothing existing changed shape.
- Added `google-cloud-storage` to both `Dockerfile` and
  `Dockerfile.cloudrun-blackwell` as its own late `RUN pip install`
  layer (after the heavy clone/pip/model-download layers, so editing it
  later doesn't bust those caches).
- **Watchdog-aware**: the spawned bench task calls
  `tracker.touch()`/`increment()` and holds a `DecrementGuard` for its
  duration, mirroring `run_job`'s pattern. Without this the RunPod idle
  watchdog (3 min of `active_requests == 0` — `watchdog.rs`) would see
  no activity during a multi-minute sweep and could stop the pod
  mid-benchmark, since nothing else about a fire-and-forget `/bench`
  call looks different from idle to it.
- Verified: `cargo build`/`cargo test -p vibe-service` pass (existing
  12 tests untouched), `python3 -m py_compile batch_bench.py` passes.
  Not yet run against a real Cloud Run deploy or real CUDA — that's the
  actual next step; this is the harness, not the data.

**Batch is exposed at the API, not collected from independent jobs.**
Real usage feeds a whole document through (tens of segments at once);
small batches (1-3) only happen in ad hoc testing. So there's no need
for a timing-window collector guessing at batch membership from
separately-submitted jobs (`JobRequest` currently models one segment
per job — `service/src/main.rs:69`). Instead, add a batch submission
shape that takes an array of segments up front and fires one
`generate()` call across all of them, sized to whatever the caller
sends (typically the whole document's segment list). The existing
per-segment `JobRequest` path can stay for the small-N test case, or
itself become a batch of one.

**This requires a client change too, not just the server.** The `vibe`
CLI is the actual (only) client and currently drives everything
per-segment in a loop: `POST /jobs` once per segment, then `GET
/jobs/:id` polling and `GET /jobs/:id/wav` fetch once per segment
(`vibe/src/main.rs:585-663`). None of that is a server-side
implementation detail the API can hide — to actually get a batched
`generate()` call, the CLI has to gather a document's segments and
submit them together as one batch request, then demux N results back
out (poll once for batch status, or poll/fetch per segment within the
batch — exact shape TBD when this is designed). Both sides of this
boundary move together; this isn't purely additive server work.

**Same-length-ish segments reduce the early-stopping mismatch risk.**
`finished_tags` means a batch only finishes when its slowest member
does — segments are already constrained to a narrow length window
(tuned to avoid degradation artifacts; see "Qualitative notes" in
`vibevoice.md`), so idle waste from one long outlier holding up a batch
of short ones should be minor. Worth confirming with real RTF
measurements once this is built, not assumed.

**Per-job result demux:** the server needs to map each item in the
batched `generate()` output back to its originating segment id to
report into the existing async job-store API (`jobstore.rs`).

**Alignment pipeline interaction.** `run_alignment` (`main.rs:405-441`,
CPU-side via `spawn_blocking`) currently runs once per job, after that
job's single synth call returns (`main.rs:576`). A batched `generate()`
call returns N wavs at once, so alignment becomes N separate
`spawn_blocking` calls fired together after the batch completes, not
one call per job as today. We have plenty of CPU headroom for this —
the N=8 Blackwell test already confirmed genuine 4-way alignment overlap
with no detectable contention (see `cloudrun.md`, "Alignment ... holds
up even under genuine 4-way overlap"), and Tokio's blocking-task pool
schedules these independently. So this should parallelize without extra
work, as long as the N alignment calls are fired concurrently
(`tokio::join!`/`FuturesUnordered` over the batch) rather than awaited
one at a time in a loop — that ordering choice belongs in the
implementation, not assumed away here.

**ActivityTracker / idle-watchdog semantics need a decision, not just
an assumption.** `tracker.increment()`/`decrement()` (`main.rs:496,500`,
`watchdog.rs`) currently wraps one job's lifetime 1:1 — `active_requests`
counts in-flight jobs for idle-shutdown purposes. With a batch of N
segments sharing one `generate()` call, counting once per batch would
make the watchdog see "1 active job" while the GPU is actually running
N segments worth of work, which is fine for idle-shutdown correctness
(non-zero is non-zero) but loses granularity if `active_requests` is
ever used for more than a zero/non-zero check. Counting once per
segment (increment N times, decrement as each segment's alignment
finishes) is the more faithful option and is also what the existing
stuck-job heuristics already assume per-item semantics for. No code
decision was made here — flagging that the doc needs to say which, since
Stage 1's semaphore-per-job model doesn't have a batched-job precedent
to follow.

**`/health` must gate on actual model readiness, not just process-up.**
Currently `health_handler` (`main.rs:98-103`) returns `"status": "ready"`
unconditionally once the Rust binary is serving — it doesn't check
whether VibeVoice is loaded, because today there's no persistent model
to load (each job spawns its own subprocess). Once there's a persistent
model server, `/health` needs to reflect whether *that* process has
finished loading, not just whether the Rust process is up — otherwise
Cloud Run/RunPod can route real traffic before the model is ready,
and the first request(s) eat load time inline instead of it happening
once, before any traffic is routed. This is the version worth building;
stated explicitly here so it isn't assumed away by reusing the current
health check as-is.

**Open risks, accepted rather than blocking:**
- No per-sample try/except exists in the generation loop —
  `finished_tags` only tracks normal completion (EOS/max-length), not
  failure, so it's unverified whether a CUDA error or NaN in one batch
  member would take down the whole batch's `generate()` call. We
  haven't observed this failure mode in practice and don't have a
  reliable way to trigger it for testing.
- One layer up: if the persistent server process itself crashes (OOM,
  segfault, an unhandled exception escaping the request handler), that
  takes down every in-flight job across the whole instance at once —
  the entire batch and any other concurrently-running batches, with no
  recovery path unless something outside the process supervises and
  restarts it. This is a bigger failure surface than the per-sample one
  above (blast radius is the whole instance, not one batch), but for
  the same reason — never observed, no cheap way to manufacture it for
  testing — it's tracked the same way: accepted, not blocking. If
  production sees an instance go dark with all its in-flight jobs
  unresolved, that's the signal to add process supervision (e.g. a
  watchdog that restarts the model server and fails any jobs it was
  holding), not something to build preemptively.

This decision is made, not pending (see Stage 2) — proceed to Stage 3
rather than gating it on further data collection.
