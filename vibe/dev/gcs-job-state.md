# GCS-backed job state

## Problem

`vibe-service` tracks synthesis jobs in an in-memory
`Arc<RwLock<HashMap<String, JobState>>>`. This breaks down differently on
each platform:

- **Cloud Run**: with `--min-instances 0`, the autoscaler tracks instance
  liveness by active HTTP requests, not background GPU work. `POST /jobs`
  returns immediately (inference runs in a `tokio::spawn` background task),
  so between polls there's no "active request" signal. Cloud Run can decide
  the instance is idle and start a replacement, which gets routed
  subsequent traffic — the replacement has no record of the job, so
  `GET /jobs/:id` 404s mid-run. Confirmed in testing 2026-06-23 (see
  `vibe/run.log`): job created and ran fine for ~40s, a new instance
  started ("Reason: AUTOSCALING"), and the next poll 404'd.
- **RunPod**: doesn't have that autoscaler-driven churn — the pod is a VM
  you control, and the existing idle watchdog
  (`service/src/watchdog.rs`) correctly tracks background job activity via
  `ActivityTracker::increment`/`decrement` wrapping the actual inference
  work, not just the HTTP request, so it won't kill a busy pod. What GCS
  state fixes here instead: if `vibe-service` itself crashes or restarts
  (OOM, redeploy, reconnect after an SSH drop), in-memory job state is
  lost regardless of backend. GCS-backed state survives that.

Other options considered and parked:
- **Cloud Run Jobs** (run-to-completion, no live traffic routing) — GPU
  is supported (max 1hr timeout for GPU tasks), and this is architecturally
  a better fit for batch-style inference long-term. Parked for now because
  it's a bigger rewrite (no more `vibe-service` HTTP API for the Jobs path;
  status tracking via the Jobs Execution API or GCS markers; CLI trigger
  logic changes). High on the list once the current path works reliably —
  also of interest for running N segments in parallel across N GPU task
  executions.
- **Session affinity** (`--session-affinity`) — only pins routing to a
  still-alive instance; doesn't protect against Google draining/killing an
  instance for infra reasons (maintenance, etc.). Not a fix on its own.
- **Synchronous (blocking) `POST /jobs`** — would keep one continuously
  open request for the whole job duration, but doesn't actually need to
  bundle wav/transcript/report into that response (no multipart rework
  needed) — superseded by the long-poll idea below, then superseded again
  by going straight to durable storage, which also covers the
  infra-drain case that neither blocking nor long-polling can.
- **Long-poll `GET /jobs/:id`** — bounds the wait under RunPod's ~100s
  proxy timeout, keeps a continuously-open request during inference.
  Doesn't protect against Google draining the instance outright (not just
  routing traffic elsewhere, but killing the running container). Shared
  persistence covers this case too, so long-polling adds complexity
  without covering the worst-case failure.

## De-risk results (2026-06-23)

Crate chosen: **`object_store` 0.14** (Apache Arrow) — its `InMemory` and
`LocalFileSystem` backends give the `MemJobStore` test fake for free.

Standalone de-risk crate: `vibe/gcs-eval/` (own `[workspace]`). Bucket
`gs://vibe-jobs-a4127f08` (us-central1, 7-day lifecycle delete).

- ✅ **Cloud Run ambient auth WORKS.** Ran `gcs-eval` as a Cloud Run Job
  under the real service identity
  (`369234196163-compute@developer.gserviceaccount.com`) with **no
  explicit credentials** — `object_store` resolved via the instance
  metadata server and did PUT/GET/list/delete round-trips. This was the
  critical undocumented risk; it's cleared.
- ⚠️ **API corrections vs the original plan:**
  - `object_store` does **not** read `GOOGLE_APPLICATION_CREDENTIALS`.
    `from_env()` uses its own names (`GOOGLE_SERVICE_ACCOUNT` /
    `GOOGLE_SERVICE_ACCOUNT_PATH` for a key file,
    `GOOGLE_SERVICE_ACCOUNT_KEY` for inline JSON). For an explicit key
    file use `with_service_account_path()` /
    `with_application_credentials()`.
  - put/get/delete live on the `ObjectStoreExt` trait (0.14) — must be in
    scope alongside `ObjectStore`.
- ✅ **RunPod / non-GCP key auth WORKS.** Org policy
  `iam.disableServiceAccountKeyCreation` initially blocked key creation;
  resolved by granting self the perms and minting a key. Proved the key
  path by running `gcs-eval` locally with `SA_KEY_PATH` set,
  `GOOGLE_APPLICATION_CREDENTIALS` unset, and gcloud ADC out of scope —
  i.e. a non-GCP host with no metadata server, the faithful RunPod proxy.
  Used `with_service_account_path()`. Remaining RunPod work is pure
  plumbing (entrypoint.sh: base64-decode the key, set the path env var) —
  no longer a crate risk.

## Design direction

Persistent job state is the right invariant **independent of deploy
backend**. Cloud Run autoscaler churn, a RunPod/GCE crash-restart, OOM,
or an SSH drop are all the same failure shape: the instance holding the
job vanishes and in-memory state is lost mid-run. Build for "the instance
holding the job can disappear," not for one platform's quirk — that keeps
us un-tied to Cloud Run vs GCE vs RunPod. (GCE in particular would give a
wider GPU choice and an architecture identical to RunPod, just a
different start/stop API; this state work is a prerequisite either way.)

### Job phases

`run_job` goes through more than three states; the wav is **not** the
final step:

1. `Running` — synthesizing.
2. wav produced — alignment (`run_alignment`) runs next; wav exists but
   the job is not yet `Done`.
3. `Done { wav, align: Option<..> }` — reached **after** the align step.
   wav is guaranteed present; align is optional (alignment can fail and
   the job still completes — `transcript`/`report` then 404, as today).

So **`Done` is the commit marker, not "wav written."**

### GCS is the durability layer, not the hot read path

In-memory `HashMap` stays the fast path. GCS is written only on **genuine
state changes** and read only on a **local miss** (resurrection):

- **Write-through on transition**: write to GCS when a job goes
  `Running`, when it reaches `Done` (wav, plus transcript/report if align
  succeeded), and on `Error`. Not on every poll.
- **Read-on-miss**: a `GET /jobs/:id` for a `job_id` this instance has
  **never seen** is the resurrection signal — fall back to GCS once,
  rehydrate into local memory, then serve from memory thereafter.
- Normal polling stays in-memory and free; GCS gets touched only on the
  handful of write transitions plus the rare resurrection.

### "Stuck running" is eliminated by construction (no heartbeat)

We've been burned before by a job wedged in `Running` forever. We avoid
that without heartbeat machinery, leaning on the fact that **a job
interrupted before the wav is written has to be re-run anyway**:

- The signal is the **commit marker, not wav presence**: a resurrecting
  instance that finds `status.json` still at `Running` (no `Done` marker)
  treats the job as **dead → returns a terminal `Error`** (not an
  indefinite `Pending`/`Running`). The CLI stops polling and the caller
  resubmits. This holds even though the wav may have been written before
  the original instance died mid-alignment — we don't try to resume a
  partial job, we re-run it.
- `Done` resurrects normally: wav is guaranteed present and served;
  transcript/report served if align succeeded, else 404 as today.

**Future optimization (after the main path works):** if a resurrecting
instance finds a wav written but no `Done` marker (died mid-alignment),
it could *resume* by re-running alignment from the existing wav rather
than re-running the whole job. Synthesis is by far the most expensive
step and alignment is reproducible from the wav, so this salvages the
costly work. Deferred because it adds resume logic (writing `audio.wav`
as its own pre-`Done` commit point, plus a resurrecting instance
operating on another instance's partial output) — not worth it until the
re-run-from-scratch path is proven reliable.
- No "stuck forever" state can exist — that was the bug, and this design
  removes the possibility.
- **Accepted limitation**: this assumes effectively serial jobs (no two
  live instances both legitimately `Running`). With `--min-instances 0`
  and single-job concurrency that holds. If we later run concurrent jobs
  across live instances, revisit with a real liveness/heartbeat signal
  rather than the "no wav ⇒ dead" heuristic.

## Plan: durable job state in GCS

### Storage layout

One bucket, flat addressing by `job_id` — no doc/segment namespacing:

```
gs://<bucket>/{job_id}/status.json
gs://<bucket>/{job_id}/audio.wav
gs://<bucket>/{job_id}/transcript.json
gs://<bucket>/{job_id}/report.json
```

`GET /jobs/:id` only has `job_id` in the URL, not `name` — nesting by
doc/segment would need a separate `job_id → name` index just to know
where to look, for a benefit that's mostly nicer console browsing.
`name` is already part of `JobRequest` and gets embedded in
`status.json`, so a job is still findable by name via a search across
`status.json` contents if ever needed.

These GCS objects are short-lived (cleaned up via a lifecycle rule) —
they exist to survive instance churn during a run, not as a long-term
archive. The durable per-doc archive already in use
(`vibe/data/<doc>/<dated-run>/...`) stays on local disk, untouched by
this change.

`status.json` contents: status, name, seed, wall_secs, audio_secs, rtf,
error — everything from `JobState` except the wav bytes and alignment
payloads, which get their own objects.

### Key property: no CLI changes needed

The external HTTP contract (`POST /jobs`, `GET /jobs/:id`, `/wav`,
`/transcript`, `/report`) stays identical. Only `vibe-service`'s internal
storage backend changes — the poll loop and fetch logic in `vibe/src/
main.rs` don't move.

### Write ordering / atomicity

`run_job` currently does one in-place mutation to `Done` carrying wav +
align together. Against GCS that becomes several object PUTs. Order
matters:

- Write `audio.wav` first, then `transcript.json`/`report.json` **if
  align succeeded** (omitted on align failure — matches `align: None`),
  then `status.json = Done` **last as the commit marker**.
- Otherwise a poll can see `status = Done` before `/wav` exists and the
  fetch logic in `vibe/src/main.rs` will 404. `status.json` is the single
  source of truth for "is this job complete"; never advance it to `Done`
  until the payload objects that exist for this job are durably written.

### `JobStore` abstraction (trait + fake)

- Add a `JobStore` **trait** wrapping read/write of job state, with two
  impls:
  - `GcsJobStore` — GCS client + bucket name, used in production.
  - `MemJobStore` (fake) — in-memory only, used for unit tests and local
    dev so **tests never touch GCS**. The write-through cache means the
    real store is also memory-fronted, so the fake is a faithful stand-in.
- The trait replaces direct `HashMap` access in `create_job_handler`,
  `get_job_handler`, `get_job_wav_handler`, `get_job_transcript_handler`,
  `get_job_report_handler`, and `run_job`.
- Each handler keeps the local `HashMap` as the read cache; the trait is
  consulted on write (transitions) and on local miss (resurrection), per
  the design above.
- Drop the "remove wav from memory after fetch" optimization currently in
  `get_job_wav_handler` — GCS storage cost is trivial, no memory pressure
  to manage. Use a GCS lifecycle rule (e.g. delete after 7 days) instead
  of app-level cleanup.
- Crate: `object_store` 0.14 (`gcp` feature). Ambient credentials on Cloud
  Run (metadata server); explicit key via `with_service_account_path()` on
  RunPod. One generic `StoreImpl<S>` covers both `GoogleCloudStorage` and
  the `InMemory` test/local backend. (Implemented in
  `vibe/service/src/jobstore.rs`.)

### Auth — differs per platform

| Platform  | Credential path                                              |
|-----------|---------------------------------------------------------------|
| Cloud Run | Ambient — default service account (`369234196163-compute@…`) via the metadata server, which has `roles/storage.objectAdmin` on the bucket. Only `GCS_BUCKET` is set; no key. |
| RunPod    | Not on GCP, no metadata server. Uses an explicit service-account key (SA `vibe-jobs-runpod`): base64'd into the `GCS_SA_KEY_B64` RunPod template env var (same pattern as `VIBE_SERVICE_SECRET`); `entrypoint.sh` decodes it to `/root/gcs-sa-key.json` and exports `GCS_SA_KEY_PATH`, which `main()` feeds to `object_store`'s `with_service_account_path()`. |

### Infra setup (one-time)

- [x] Create the GCS bucket — `gs://vibe-jobs-a4127f08` (us-central1).
- [x] Grant Cloud Run's default service account `storage.objectAdmin` on
      it (also granted to SA `vibe-jobs-runpod`).
- [x] Generate a service-account key for RunPod; base64'd into `vibe/.env`
      as `GCS_SA_KEY_B64` (not baked into the public `dockersaura/vibe`
      image). Required lifting the org policy
      `iam.disableServiceAccountKeyCreation` for the project.
- [x] Wire the key through `vibe/entrypoint.sh` (decode + set
      `GCS_SA_KEY_PATH`).
- [x] Add a bucket lifecycle rule — delete objects after 7 days.
- [x] Add `GCS_BUCKET` to the Cloud Run deploy command (`dev/setup.md`)
      and the RunPod template (`runpod.md`).

Deploy/build not yet done — both images need a rebuild+push carrying the
JobStore code before durable state is live.

### Crate de-risk — done

Both credential paths were proven with the standalone `vibe/gcs-eval/`
binary against `object_store` 0.14 (not `google-cloud-storage` — see the
De-risk results section at the top for the rationale and the API
corrections). Cloud Run ambient auth ran as a Cloud Run Job; the non-GCP
key path ran locally with `SA_KEY_PATH` set and ADC out of scope (faithful
RunPod proxy).

### Open items for whoever picks this up

- **Cloud Run churn frequency is uncharacterized** — hit once in a few
  tests, could be rare bad luck. Not worth measuring before doing this
  work: write-on-transition is cheap and also buys crash-resilience on
  every backend, so the work pays off regardless of how often Cloud Run
  specifically churns.

## TODO (follow-ups)

- **Split `vibe/service/src/main.rs` into modules.** It's ~900 lines now
  and mixes HTTP handlers, the job lifecycle (`run_job`/`run_inference`),
  resurrection, and `main()` wiring. Candidate split: a `handlers` module
  (the axum route handlers) and a `jobs` module (`run_job`, `resurrect`,
  `persist_status`/`persist_align`), leaving `main.rs` as wiring +
  `AppState`. The resurrection logic and store contract are now covered by
  tests in `main.rs` (`mod tests`) and `jobstore.rs`, so the move has a
  safety net — do the split without changing behaviour, keep tests green.
