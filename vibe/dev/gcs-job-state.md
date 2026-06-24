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

## Plan: replace in-memory state with GCS

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

### `vibe-service` changes

- Add a `JobStore` abstraction (trait or struct) wrapping a GCS client +
  bucket name, replacing direct `HashMap` access in `create_job_handler`,
  `get_job_handler`, `get_job_wav_handler`, `get_job_transcript_handler`,
  `get_job_report_handler`, and `run_job`.
- Drop the "remove wav from memory after fetch" optimization currently in
  `get_job_wav_handler` — GCS storage cost is trivial, no memory pressure
  to manage. Use a GCS lifecycle rule (e.g. delete after 7 days) instead
  of app-level cleanup.
- Crate: `google-cloud-storage` (implements the standard Application
  Default Credentials chain — metadata server *or*
  `GOOGLE_APPLICATION_CREDENTIALS` file — so the same code works on both
  backends without branching).

### Auth — differs per platform

| Platform  | Credential path                                              |
|-----------|---------------------------------------------------------------|
| Cloud Run | Ambient — default service account via the metadata server. Just needs IAM: `roles/storage.objectAdmin` on the bucket. No code/config beyond that. |
| RunPod    | Not on GCP, no metadata server. Needs an explicit service-account key. Plan: base64 the JSON key into a RunPod template env var (same pattern as `VIBE_SERVICE_SECRET`), decode to a temp file in `entrypoint.sh`, set `GOOGLE_APPLICATION_CREDENTIALS` to that path. |

### Infra setup (one-time)

- [ ] Create the GCS bucket.
- [ ] Grant Cloud Run's default service account `storage.objectAdmin` on
      it.
- [ ] Generate a service-account key for RunPod; add it to `vibe/.env`
      (not baked into the public `dockersaura/vibe` image).
- [ ] Wire the key through `vibe/entrypoint.sh` (decode + set
      `GOOGLE_APPLICATION_CREDENTIALS`).
- [ ] Add a bucket lifecycle rule (delete objects after N days).
- [ ] Add `GCS_BUCKET` env var to both the Cloud Run deploy command and
      the RunPod template.

### Open items for whoever picks this up

- Bucket name/project not yet decided.
- `google-cloud-storage` crate version/API surface not yet checked
  against current `vibe/service/Cargo.toml` dependency versions.
- Need to confirm `google-cloud-storage`'s ADC chain actually picks up
  `GOOGLE_APPLICATION_CREDENTIALS` the way we expect on a non-GCP host
  (RunPod) — verify with a small standalone test before wiring it into
  `vibe-service` proper.
