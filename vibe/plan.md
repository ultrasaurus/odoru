# Plan: RunPod control via Rust crate

## Goal
- Replace the SSH/curl one-liners against RunPod with a small Rust
  binary.

## Process
- Review  `pod-lifecycle` workspace (separate repo in 
  `../pod-lifecycle`) code and docs (all .md files) to gather
  known wisdom about using RunPod and learn its quirks
- New code should be saved in odoru/vibe, depending on normalize.rs
  (see `dev/normalize-future.md`).

## Steps
1. `pilot-worker` runs on `worker` (Cloudflare Workers/wasm) — not
   usable for a native CLI. `lifecycle` crate is just hmac/auth
   helpers, no RunPod client. `crates/server` uses plain `reqwest`
   against RunPod's REST API (`https://rest.runpod.io/v1/...`,
   `Authorization: Bearer $RUNPOD_API_KEY`) — that's the pattern to
   follow here: `reqwest` + `serde_json`, no dedicated RunPod crate
   needed (checked crates.io, nothing well-maintained).
2. Add config loading for `RUNPOD_API_KEY`
   (or RUNPOD_USER_API_KEY if used within the pod)
   keep it in `vibe/.env` with gitignore.
3. Implement commands we currently do by hand:
   - list pods / templates
   - create pod from template
   - start/stop pod
   - get pod status + SSH connection info
4. Add a small CLI (`clap`) wrapping these commands.
5. Also add a download command (scp/sftp over the pod's SSH) to pull
   the generated wav back to `vibe/` so it can be played locally.
6. Replace the ad-hoc ffmpeg-over-ssh silence-detect workflow with a
   helper that runs ffmpeg silencedetect locally on the wav
   downloaded in step 5 — no remote ffmpeg/ssh needed for this part.
7. Once working, add a note to the relevant `pod-lifecycle` docs
   (e.g. `dev/runpod.md`) pointing at this CLI, so it's discoverable
   alongside the existing curl-based workflows.

## Future
- `start-pod` can fail with "not enough free GPUs on the host
  machine" (hit during testing). Per `pod-lifecycle/dev/runpod.md`,
  the fix is terminate + recreate from template:
  - `delete-pod <pod_id>` (DELETE `/pods/{id}`)
  - `create-pod-from-template <template_id>` (POST `/pods` with
    templateId)
  - Then update any stored pod ID references (e.g.
    `RUNPOD_POD_ID` secret in pilot-worker, like `dev/new-pod.sh`
    does).

## Related
- [dev/normalize-future.md](../dev/normalize-future.md) — normalizer
  test cases found while listening and comparing `vibe/odoru_markers_normalized.txt` output to `data/markers.txt`.
