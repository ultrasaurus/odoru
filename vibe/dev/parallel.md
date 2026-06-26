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

### Batch size testing

commit: `ecde1bd264013afaee5d665e1d0c03186a2fc993`

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
segment once, no repeats) baked in.

**Results (first real run, Blackwell, v4 image, 2026-06-25,
`request_id=2c677d7b-2582-4ee0-a164-c4314f4395c8`):**

| N | Wall | Peak VRAM | Throughput |
|---|---|---|---|
| 1 | 30.1s | 5290 MiB | 2.30x |
| 2 | 36.9s | 5417 MiB | 4.23x |
| 4 | 20.9s | 5667 MiB | 8.24x |
| 8 | 43.3s | 6167 MiB | 9.73x |
| 16 | 38.1s | 7173 MiB | 22.48x |

Both open questions from this section are answered, at least directionally:

- **VRAM is not the constraint, by a wide margin.** ~125 MiB/item from
  N=1 to N=16 (5.3 GiB → 7.2 GiB total), nothing like the ~6.3 GiB/job
  from the N=2/4/8 *independent-process* test. Confirms the hypothesis
  above: that number was almost entirely duplicated model weights, not
  per-item activation/KV-cache cost. Massive VRAM headroom remains on
  the 96 GiB card even at N=16 — batch size is nowhere near a
  VRAM-bound ceiling yet.
- **Throughput keeps climbing, no flattening yet** — 2.3x → 22.5x, and
  N=16 is the best point measured, not a plateau. This is a
  fundamentally different shape than the N=2/4/8 process-concurrency
  curve in `cloudrun.md`, which flattened hard by N=8 (~3.0x) from GPU
  compute contention. Real batching is avoiding that contention
  entirely, as expected — one fused forward pass instead of N
  independent kernels time-slicing the same SMs.
- **Caveat — this is one run, each N tested once, and wall times are
  non-monotonic** (N=8's 43.3s is higher than N=4's 20.9s despite 2x
  the work), consistent with cold-start/scheduling noise rather than a
  real per-N effect — same kind of contamination `cloudrun.md` flagged
  for the process-concurrency tests.
- **Checked: this isn't a "got easy segments at high N" confound.**
  `batch_bench.py`'s `run_batch()` already records per-item `words` and
  `audio_duration_secs`; pulling the per-item breakdown (not just the
  aggregate table above) and computing seconds-of-audio-per-word per
  item shows it's stable across every N — roughly 0.30–0.48 s/word with
  no drift as N increases (e.g. N=4: 0.33–0.39, N=16: 0.28–0.48). So
  the content landing in each batch isn't systematically easier or
  harder at higher N — the audio-duration numbers feeding the
  throughput calculation are sound.
- **Not checked, and still the real gap: per-item *wall-clock* time.**
  The aggregate throughput metric (`total_audio_secs / wall_secs`) is
  mathematically identical whether computed in aggregate or "per item"
  (dividing both sides by N cancels out) — so there's no separate
  per-item throughput number to extract beyond what's in the table
  above; the content-confound check above is the meaningful per-item
  analysis available from this data. What's still unverified is
  whether `wall_secs` itself is a reliable measurement at each N — the
  non-monotonic pattern (N=8 slower than N=4) means it might not be.
  **Don't treat the throughput curve above as validated yet.** The
  qualitative conclusion (real batching avoids the time-slicing
  contention seen in N=2/4/8) is a sound mechanism-level argument
  independent of this data and very likely still correct, but the
  specific numbers need a repeat run (and pushing past N=16, since
  nothing here suggests a ceiling) before locking in a production
  batch size off this curve.
- Results: full log + wavs at
  `gs://vibe-jobs-a4127f08/bench/2c677d7b-2582-4ee0-a164-c4314f4395c8/`;
  `batch_bench_runs.jsonl` copied to `vibe/bench_runs.jsonl` (tracked,
  for history — matches the existing `vibe/runs.jsonl` convention for
  per-segment job logs) rather than left only in GCS.

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
  12 tests untouched), `python3 -m py_compile batch_bench.py` passes,
  and a real run against the deployed Cloud Run Blackwell instance
  succeeded end-to-end (see results above) — trigger, background
  watchdog-safe execution, GCS upload, and `/log` tailing all worked
  as designed on the first clean run (the very first attempt failed on
  a missing uploaded voice, not a bug in this path — see below).

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

## Stage 3 implementation plan: client-side batching API

Scope for making the client (`vibe` CLI) actually submit batches,
end to end, building on the real N=1..16 results above. Planning only
— nothing below is implemented yet.

**Key design choice: reuse per-segment job IDs for storage, batch only
the execution.** `jobstore.rs` is already keyed 1:1 per job_id, with one
`StoredStatus` + named blobs per id (`jobstore.rs:41-46`). Keeping
`job_id` as the unit of durable state means the existing `GET
/jobs/:id`, `/jobs/:id/wav`, `/jobs/:id/transcript`, `/jobs/:id/report`
endpoints don't need to change shape at all. Batching only changes how
the synth step runs (N segments in one `generate()` call instead of N
subprocesses) and how results get demuxed back into N pre-existing job
records. A thin `batch_id` just groups job_ids for submission.

### 1. Client (`vibe` CLI)

- **Not "whole document" — an explicit list/range of segment names.**
  `SynthInput::Doc` is currently an unimplemented stub
  (`main.rs:501-503`: `anyhow::bail!("synthesize doc is not yet
  implemented...")`), and it would be a mistake to finish it as
  originally framed ("synthesize a whole doc"). Current reality: 3 docs
  are in progress, work is iterative and listen-driven (only
  `authorship` has been listened to all the way through, and that run
  is itself still being refined), and we're not yet at the point of
  confidently firing off a whole-doc render. What's actually needed now
  is the ability to pick a range or explicit list of segment names and
  fire them off together — the server-side `/batches` API supports
  this regardless of how the segments are chosen, so the client just
  needs a new input mode (e.g. `synthesize segments seg41-56` or
  `synthesize segments seg41,seg43,seg50`) alongside the existing
  single-`segment` mode, not a doc-level mode. Whole-doc submission
  can become a thin wrapper over this later, once the workflow is
  ready for it — this also keeps `SynthInput::Doc` itself unimplemented
  for now rather than building something ahead of actual need.
- New flow: read/normalize the selected segment files (reuse
  `segment::resolve_basedir` + the existing normalize loop, just looped
  over N files instead of one), `POST /batches` once with all N texts.
- **Polling: once, not N times.** All N segments in a batch come from
  one `generate()` call, so they share fate — there's no scenario where
  one finishes meaningfully before another from the client's
  perspective (the batch returns as a unit, then gets demuxed into N
  job records in quick succession). Polling N job_ids concurrently
  (`join_all`) would be redundant network calls for an answer that
  arrives essentially atomically. Poll the new `GET /batches/:id` (see
  "Server API" below) until it's `done`, then fetch all N individual
  results — `wav`/`transcript`/`report` still need one `GET` per
  job_id each, since those payloads genuinely differ per segment, but
  that's a fetch fan-out, not a polling fan-out.

### 2. Server API (`vibe-service`)

- New `POST /batches`: body is `{segments: [{text, name}, ...], seed,
  speaker, cfg_scale, temp, speed}` (one shared set of knobs for the
  whole batch, per-segment text/name — confirmed this matches current
  workflow, no per-segment override needed) — mirrors `JobRequest`
  (`main.rs:69`) but with an array. Returns `{batch_id, job_ids: [...]}`.
- Each segment gets inserted into `state.jobs` as `JobState::Pending`
  exactly like `create_job_handler` does today (`main.rs:178`) — just N
  inserts instead of 1.
- New `GET /batches/:id`: aggregates status across the batch's job_ids
  (likely needs a small `batch_id -> Vec<job_id>` map in `AppState`).
  Decided server-side rather than having the client treat one job_id as
  a stand-in for the whole batch — but keep the status semantics thin
  for now (`pending`/`running`/`done`/`error`, "any job errored" →
  `error` plus which ones). We haven't had a single-job failure yet, so
  there's nothing concrete to design richer partial-status semantics
  (e.g. "3 of 5 done") around — build the cheap aggregation now, let
  real failures inform anything beyond that later.
  - **Explicit decision needed on the `batch_id -> job_ids` map's
    durability, not "in-memory is fine to start" sliding in
    unexamined.** Individual job_ids are durable specifically because
    of the Cloud Run instance-churn problem this whole project started
    from (`gcs-job-state.md`) — an in-memory-only batch map regresses
    exactly that failure mode for the batch grouping, even though the
    underlying job_ids would resurrect fine individually. If an
    instance dies/churns mid-batch, `GET /batches/:id` on a replacement
    instance has nothing to resurrect from.
  - **Accepted mitigation, not a gap left unaddressed**: the client
    already has the `job_ids` array from `POST /batches`'s response, so
    a fallback to per-job polling (`GET /jobs/:id` × N, same as the
    pre-batching client code) works without needing the batch map to
    survive churn. Naming this explicitly: in-memory batch tracking is
    accepted *because* this fallback exists and the client should
    implement it (e.g. treat a 404 from `GET /batches/:id` as "fall
    back to polling job_ids individually," not as a hard failure) —
    not because the churn risk doesn't exist.

### 3. Server execution: language boundary

Worth being explicit about what's negotiable here and what isn't,
given the long-term interest in eventually moving more of this to Rust
for performance:

- **Not negotiable now: the `model.generate()` call stays in Python.**
  Moving it to Rust would mean reimplementing VibeVoice's architecture
  (transformer backbone + diffusion head + voice tokenizer +
  flash-attention bindings) in something like `candle` — not "new
  logic we're free to write fresh," but a from-scratch port of someone
  else's model. `forced-alignment` already proves Rust+CUDA ML
  inference works in this codebase (`candle`-based), but that's a much
  smaller wav2vec2 CTC model — porting VibeVoice is a legitimate
  "someday" idea but a separate, large, speculative project on its
  own, not something to fold into this batching work.
- **Already Rust, per this plan: nearly everything else.** Per-job_id
  result demux into `JobStore`, batch-level error handling,
  semaphore/tracker wiring, naming/IDs — `run_inference_inner` already
  does the Rust-side half of this for single jobs (reads the output
  dir, scrapes stdout for `"Seed used:"`, writes `JobStore` entries),
  and this plan just extends that pattern to N jobs. The new Python is
  genuinely thin (see below) — there isn't much glue logic left over
  to debate a language for.
- **Logging doesn't require porting anything.** The Python subprocess's
  stdout/stderr already flows into the same per-request log file Rust
  manages today (`/log/:request_id`), and `tracing` already wraps the
  whole batch lifecycle server-side. The existing `"Seed used:"`
  stdout-scraping pattern is the template for pulling any other
  per-segment data out of the Python log without moving the call
  itself.

**The new script itself**, not a reuse of `inference_from_file.py`
as-is — that script's "wrap in list" batching is for multiple speakers
within one script (one output), not N independent segment requests with N
  separate outputs. The batching logic already exists in
  `batch_bench.py`'s `run_batch()`: N texts in, N wavs out, demuxed by
  name. Production version = `run_batch()` minus the benchmarking
  instrumentation (VRAM tracking, sweep loop), plus writing results in
  whatever format the Rust side reads back per job_id.
- A fresh subprocess-per-batch-call (like today's per-job subprocess,
  just handed N texts instead of 1) ships first, and **may turn out to
  be all that's needed** — but the checkpoint-load timing below was the
  wrong cost to check, and the right one needs an explicit measurement
  before this is settled. Checkpoint load (`Loading checkpoint shards`
  completing in under a second once weights are warm in local cache)
  isn't the cost that matters: `cloudrun.md` already measured a
  *different* cold-start cost directly — the first job after a fresh
  Blackwell deploy ran at RTF 0.92 vs. steady-state ~0.53, attributed to
  CUDA/cuDNN kernel autotuning and flash-attn warmup, not checkpoint
  loading. If that cost recurs per *process* rather than being paid
  once per *instance*, every fresh `/batches` subprocess pays it again,
  which is exactly the cost a persistent server would eliminate — and
  it's not what the under-a-second checkpoint-load number speaks to at
  all.
  Good news: `cloudrun.md` already has the data to answer this, just
  not framed this way — all 14 jobs in that steady-state sequence
  (`augment_seg13–18`, RTF 0.475–0.670) were each their own fresh
  `python3` subprocess (the documented baseline architecture: "loads
  the VibeVoice model fresh every job," no persistent process existed
  at the time), and only the very first job of the deploy showed the
  elevated 0.92 RTF. That's existing evidence the warmup cost is paid
  once per *instance* (likely a CUDA driver/cuDNN autotune cache
  persisting on disk across process boundaries), not once per
  subprocess — which would mean subprocess-per-batch-call is fine after
  all, for a different reason than originally argued. But this wasn't
  measured for the *new* batch script specifically, and the mechanism
  (disk-cached autotune) is inferred, not confirmed. Before concluding
  persistence is skippable: explicitly time subprocess-start→first-token
  (not just checkpoint-load) across a couple of back-to-back `/batches`
  calls on the same warm instance, the same "measure before committing"
  discipline Stage 2 used for load time — don't extend the inference
  from the old single-segment architecture without checking it holds
  for the new script too.
- Text reaches the script via **JSON over stdin, not temp files** — the
  `/tmp/<id>.txt` pattern in today's single-job path (`main.rs:632,661`)
  exists only because `inference_from_file.py`'s CLI expects
  `--txt_path` (inherited from the upstream demo script's convention),
  not a technical requirement. Segment text is small; stdin avoids
  temp-file cleanup/races and shell-escaping problems for arbitrary
  text (quotes, unicode, newlines) that argv would be fragile against.
- Rust side (`run_inference_inner` → a new `run_batch_inference_inner`)
  spawns this script, writes the batch JSON to its stdin, and reads
  back N wavs, writing each into its own job_id's `StoredStatus`/
  objects via the existing
  `JobStore` trait.

### 4. Alignment

- Already scoped above: N `spawn_blocking` calls fired concurrently
  after the batch returns (`FuturesUnordered`/`join_all`), not a
  sequential loop. No new design needed, just implementation.

### 5. Watchdog / semaphore

- `ActivityTracker`: increment once per segment in the batch (not once
  per batch call), decrement as each segment's alignment finishes —
  matches the per-segment semantics `run_job` already uses.
- `job_semaphore` (currently `MAX_CONCURRENT_JOBS=1`, gates both
  `/jobs` and `/bench`): a batch call should also acquire it — but this
  needs to be a deliberate redefinition of what the knob means, not a
  silent reuse of whatever value the old architecture used. The N=2/4/8
  tuning (eventually expected to settle around N=4) was for independent
  single-segment subprocesses time-slicing one GPU. Real batching's
  entire point is one fused call avoiding that time-slicing — running
  multiple concurrent batch calls under `MAX_CONCURRENT_JOBS > 1` would
  reintroduce the exact contention problem one level up (N batches
  time-slicing instead of N segments), undermining the reason batching
  helps in the first place. So for the batching architecture, the
  semaphore should stay at **1** on purpose: one batch call owns the
  GPU at a time, and batch size — not concurrent process count — is the
  scaling lever. This isn't the same setting carried over from before;
  it's a different knob with a different meaning that happens to share
  a name and a default value.

### 6. Error handling

- If the batch-level subprocess fails entirely (bad input, crash), all
  N job_ids in that batch transition to `JobState::Error` together —
  mirrors today's single-job error path applied N times.
- Per-segment failure *within* a successful batch call is the same
  open risk already logged above ("accepted, not blocking") — no new
  design needed, just a reminder this is still unresolved if it ever
  surfaces.

### Suggested phasing

Not a strict pipeline — numbered for reference, but the real
dependencies are narrower than "finish N before starting N+1":

1. Python: new batch-inference script (adapt `batch_bench.py`'s
   `run_batch`).
2. Rust: `POST /batches` + `GET /batches/:id` + batch subprocess runner
   + per-job-id result demux + semaphore/tracker wiring. Only needs (1)
   for its own integration test, not to write the handlers — can be
   built against a stub/echo script in parallel with (1), same pattern
   already used for `/bench` (built and curl-tested standalone before
   any client work touched it).
3. Rust CLI: two independent halves. The segment-list/range input mode
   (parsing, reading/normalizing files) doesn't depend on (1) or (2) at
   all and can be built in parallel with both. Actually calling
   `/batches`/`GET /batches/:id` does genuinely depend on (2) being
   done — that part can't move earlier.
4. End-to-end test on a real multi-segment selection (not a whole doc —
   e.g. a contiguous range from one of the 3 in-progress docs), compare
   wall-clock against today's sequential-per-segment loop. This is the
   final cross-check, not the first time anything gets tested — each
   piece above should be verified on its own as it's built (same
   discipline as `batch_bench.py` standalone, then `/bench` wrapping
   it, before this plan existed at all).

### Open questions (not yet decided)

These are called out inline above where they come up; collected here
so they're not lost before implementation starts. Resolved questions
are kept below (struck through in spirit, not deleted) so the reasoning
trail stays visible.

- ~~**Per-segment generation knobs.**~~ **Resolved: one shared set of
  knobs per batch.** Matches current workflow (one set of knobs per CLI
  invocation already, even across the `for seg in ...` shell loop) — no
  per-segment override needed in the `POST /batches` request shape.
- ~~**Representative poll vs. a real aggregate status endpoint.**~~
  **Resolved: build a real `GET /batches/:id`,** not a job_id-as-proxy
  hack — pushing the decision server-side, per discussion. Keep its
  semantics deliberately thin for now: `pending`/`running`/`done`/
  `error`, with "any job errored" reported as `error` plus which
  job_ids. We haven't had a single-job failure yet, so there's nothing
  to learn from yet about what richer partial-batch status should look
  like (e.g. "3 of 5 done") — build the cheap, honest aggregation now;
  let real failures (if/when they happen) drive any richer semantics
  later rather than guessing them in now.
- ~~**Decouple batching from the persistent server, or ship combined?**~~
  **Resolved, leaning further than before: may not need persistence at
  all.** Stage 2 already concluded load time was never shown to be the
  bottleneck (compute contention was — see Stage 2 above), and the
  batch_bench run showed `Loading checkpoint shards` completing in
  under a second once weights are warm in local cache (the 86s
  "Fetching 3 files" in that log was a one-time cold cache-miss, not
  representative per-call cost) against tens of seconds of `generate()`
  time per batch. Subprocess-per-batch-call ships first; persistence
  gets built only if a real measurement of repeated `/batches` calls
  shows subprocess-startup+load overhead actually matters — same
  "measure before committing" discipline Stage 2 already used for load
  time, not an assumption that it's needed.
- ~~**How does the Rust side hand N texts to the Python batch script?**~~
  **Resolved: JSON over stdin, not temp files.** The `/tmp/<id>.txt`
  pattern in today's single-job path (`main.rs:632,661`) exists only
  because `inference_from_file.py`'s CLI interface expects `--txt_path`
  (inherited from the upstream demo script's convention) — not a
  technical requirement. The new batch script has no such legacy
  interface to match, segment text is small, and stdin avoids both
  temp-file cleanup/races and shell-escaping problems for arbitrary
  text content (quotes, unicode, newlines) that argv would be fragile
  against.
- **Per-sample / per-instance failure isolation** (already logged
  above as accepted-not-blocking) still applies unchanged to the batch
  API — no new mitigation planned, just flagging it's inherited here
  too, not re-litigated.
