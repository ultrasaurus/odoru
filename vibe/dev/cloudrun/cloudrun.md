# Cloud Run GPU evaluation — overview

vibe-service runs on both RunPod (primary, performance path) and Google
Cloud Run (serverless, scale-to-zero). This is the overview; the
device-specific detail lives in two split-out docs:

- [cloudrun-L4.md](cloudrun-L4.md) — NVIDIA L4 (24 GB). Works
  end-to-end, but ~3x too slow to be the synth target — inherent to
  the hardware, not something flash-attn would fix (not pursued there
  since it wasn't expected to close that gap). Alignment runs on CPU
  for an unrelated reason (a CUDA-PTX/driver mismatch), not because of
  the speed verdict. Kept as a working fallback.
- [cloudrun-blackwell.md](cloudrun-blackwell.md) — NVIDIA RTX Pro 6000
  Blackwell (96 GB). The active synth target: fastest path measured so
  far (~2x RunPod), but costs more per segment once CPU/memory billing
  is included. Also covers the N=2/4/8 parallel-job experiments this
  GPU's VRAM headroom enabled.

The durable job-state work (`../gcs-job-state.md`) is independent of
all this and works on Cloud Run regardless — ambient GCS auth via the
metadata server is proven. The open question in both docs below is
purely whether a given Cloud Run GPU is a viable *synthesis* target vs
RunPod.

## Build / deploy

See `../setup.md` for the `Dockerfile.cloudrun*` build + `gcloud run
deploy` commands for both GPU types.
