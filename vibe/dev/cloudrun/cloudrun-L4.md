# Cloud Run: NVIDIA L4 (24 GB) — works, but not competitive

See [cloudrun.md](cloudrun.md) for the overview and how this fits next
to the Blackwell path.

Status: **end-to-end working, but too slow to be the synth target.**

- **VRAM is fine.** L4 has 24 GB, which meets our documented minimum (the
  artifact/hallucination problems were the 16 GB RTX A4000, not 24 GB).
- **Too slow.** Observed synth RTF ~2.4–3.0 (e.g. a 180-word segment:
  wall 139.6 s, RTF 2.43) vs ~1.0 on RunPod's RTX cards (0.29–0.40 on the
  24 GB cards per `../quirks.md`). Roughly 3× slower.
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
