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

### Testing ramp

Test at N=2, then N=4, before trying N=8 — each step validates an
assumption before committing to the next:

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

## Stage 2 — decide based on Stage 1 data

- If redundant per-job model load time is a small fraction of total job
  time (plausible: VibeVoice-1.5B load is seconds, synth is tens of
  seconds), **stop here**. Tune `MAX_CONCURRENT_JOBS` per instance type
  and ship it.
- If model load time meaningfully eats into the parallelism gain (e.g.
  load time becomes comparable to synth time once N jobs load roughly
  together), that's the trigger for Stage 3.

## Stage 3 — persistent model server (only if Stage 2 says it's needed)

- Replace the per-job subprocess spawn with a long-lived Python process
  that loads the model once and serves jobs over a local socket/HTTP,
  with its own internal worker slots up to N.
- Host-agnostic by construction — no Cloud Run or RunPod specifics.
- Bigger lift: process lifecycle/health, request queueing, and error
  isolation so one bad job can't take down the shared process for
  everyone else.
- Not started unless Stage 1/2 data shows it's needed — avoids wasted
  work if the simple semaphore approach is already good enough.
