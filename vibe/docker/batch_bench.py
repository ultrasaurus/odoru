"""Measure VibeVoice batched generate() at increasing batch sizes.

Answers the open question in dev/parallel.md ("Max batch size is unknown")
by loading the model once and running model.generate() with batch_size > 1,
logging VRAM and throughput at each size — as opposed to the N=2/4/8 data
already collected, which came from N independent subprocesses each loading
their own model copy and therefore measures weight duplication, not the
per-item activation/KV-cache cost that actually bounds a real batch.

Usage (run from /workspace/VibeVoice on the pod/instance, same cwd
inference_from_file.py expects, so `from vibevoice...` imports resolve):

    python3 demo/batch_bench.py \
        --speaker Sarah \
        --batch_sizes 1,2,4,8,16 \
        --output_dir /tmp/batch_bench

Defaults to demo/bench_segments/ (augment_seg41-71.txt, 31 raw — not yet
normalized — segments baked into the image; see Dockerfile.cloudrun-blackwell).
Segments are consumed sequentially across the batch-size sweep (not
restarted at index 0 each time), so the default 1,2,4,8,16 sweep (31 items
total) uses each of the 31 segments exactly once. Pass --texts_dir to point
at a different directory of .txt files instead. If the sweep needs more
items than are available, segments cycle to fill the batch — fine for a
throughput/VRAM measurement, not for listening quality.

Stops escalating batch size on CUDA OOM and reports results gathered so far
rather than crashing the whole sweep.
"""
import argparse
import gc
import glob
import json
import os
import time
from datetime import datetime, timezone

import torch

from vibevoice.modular.modeling_vibevoice_inference import VibeVoiceForConditionalGenerationInference
from vibevoice.processor.vibevoice_processor import VibeVoiceProcessor


def parse_args():
    parser = argparse.ArgumentParser(description="VibeVoice batched-generate() benchmark")
    parser.add_argument("--model_path", type=str, default="microsoft/VibeVoice-1.5b")
    parser.add_argument("--texts_dir", type=str,
                         default=os.path.join(os.path.dirname(__file__), "bench_segments"),
                         help="Directory of .txt segment files, one per batch item "
                              "(defaults to the seg41-56 sample baked into the image)")
    parser.add_argument("--speaker", type=str, default="Sarah",
                         help="Voice name to resolve via demo/voices/ (same lookup as inference_from_file.py)")
    parser.add_argument("--batch_sizes", type=str, default="1,2,4,8",
                         help="Comma-separated batch sizes to sweep, ascending")
    parser.add_argument("--cfg_scale", type=float, default=1.3)
    parser.add_argument("--seed", type=int, default=71463)
    parser.add_argument("--output_dir", type=str, default="./batch_bench_outputs")
    parser.add_argument("--results_jsonl", type=str, default="batch_bench_runs.jsonl",
                         help="Append one JSON line per batch-size result here")
    parser.add_argument("--gcs_bucket", type=str, default=os.environ.get("GCS_BUCKET"),
                         help="If set (or GCS_BUCKET env var is set), upload wavs + "
                              "results_jsonl here on completion. Needed on Cloud Run, "
                              "which has no shell/exec to scp files out otherwise.")
    parser.add_argument("--gcs_prefix", type=str, default=None,
                         help="Object prefix under --gcs_bucket; required if --gcs_bucket is set")
    return parser.parse_args()


def upload_to_gcs(bucket_name: str, prefix: str, output_dir: str, results_jsonl: str):
    from google.cloud import storage  # local import: optional dependency, only needed here

    client = storage.Client()
    bucket = client.bucket(bucket_name)

    if os.path.exists(results_jsonl):
        bucket.blob(f"{prefix}/{os.path.basename(results_jsonl)}").upload_from_filename(results_jsonl)

    if os.path.isdir(output_dir):
        for root, _, files in os.walk(output_dir):
            for fn in files:
                local_path = os.path.join(root, fn)
                rel = os.path.relpath(local_path, output_dir)
                bucket.blob(f"{prefix}/{rel}").upload_from_filename(local_path)

    print(f"Uploaded results to gs://{bucket_name}/{prefix}/")


def resolve_voice_path(speaker: str) -> str:
    voices_dir = os.path.join(os.path.dirname(__file__), "voices")
    candidates = glob.glob(os.path.join(voices_dir, f"*{speaker}*.wav"))
    if not candidates:
        raise FileNotFoundError(f"No voice file matching '{speaker}' in {voices_dir}")
    return candidates[0]


def load_texts(texts_dir: str) -> list[tuple[str, str]]:
    paths = sorted(glob.glob(os.path.join(texts_dir, "*.txt")))
    if not paths:
        raise FileNotFoundError(f"No .txt files found in {texts_dir}")
    out = []
    for path in paths:
        with open(path, "r", encoding="utf-8") as f:
            text = f.read().strip().replace("’", "'")
        out.append((os.path.splitext(os.path.basename(path))[0], text))
    return out


def gpu_name() -> str:
    if not torch.cuda.is_available():
        return "cpu"
    return torch.cuda.get_device_name(0)


def run_batch(model, processor, items: list[tuple[str, str]], voice_path: str, cfg_scale: float):
    """items: list of (segment_name, text). Returns (wall_secs, peak_vram_bytes, per_item_results)."""
    texts = [text for _, text in items]
    voice_samples = [[voice_path] for _ in items]

    inputs = processor(
        text=texts,
        voice_samples=voice_samples,
        padding=True,
        return_tensors="pt",
        return_attention_mask=True,
    )
    device = next(model.parameters()).device
    for k, v in inputs.items():
        if torch.is_tensor(v):
            inputs[k] = v.to(device)

    torch.cuda.synchronize()
    torch.cuda.reset_peak_memory_stats()
    start = time.time()
    outputs = model.generate(
        **inputs,
        max_new_tokens=None,
        cfg_scale=cfg_scale,
        tokenizer=processor.tokenizer,
        generation_config={"do_sample": False},
        verbose=False,
        is_prefill=True,
    )
    torch.cuda.synchronize()
    wall = time.time() - start
    peak_vram = torch.cuda.max_memory_allocated()

    per_item = []
    for (name, text), speech in zip(items, outputs.speech_outputs):
        if speech is None:
            per_item.append({"segment": name, "audio_duration_secs": None, "words": len(text.split())})
            continue
        sample_rate = 24000
        n_samples = speech.shape[-1] if len(speech.shape) > 0 else len(speech)
        duration = n_samples / sample_rate
        per_item.append({
            "segment": name,
            "audio_duration_secs": duration,
            "words": len(text.split()),
        })
    return wall, peak_vram, per_item, outputs.speech_outputs


def save_wavs(processor, speech_outputs, items, output_dir: str, batch_size: int):
    os.makedirs(output_dir, exist_ok=True)
    for (name, _), speech in zip(items, speech_outputs):
        if speech is None:
            continue
        path = os.path.join(output_dir, f"n{batch_size}_{name}.wav")
        processor.save_audio(speech, output_path=path, sampling_rate=24000)


def main():
    args = parse_args()
    batch_sizes = sorted(int(n) for n in args.batch_sizes.split(","))
    if args.gcs_bucket and not args.gcs_prefix:
        raise ValueError("--gcs_prefix is required when --gcs_bucket is set")

    torch.manual_seed(args.seed)
    if torch.cuda.is_available():
        torch.cuda.manual_seed_all(args.seed)

    all_texts = load_texts(args.texts_dir)
    voice_path = resolve_voice_path(args.speaker)
    print(f"Loaded {len(all_texts)} segments from {args.texts_dir}; voice: {voice_path}")
    print(f"GPU: {gpu_name()}")

    print(f"Loading processor & model from {args.model_path}")
    processor = VibeVoiceProcessor.from_pretrained(args.model_path)
    try:
        model = VibeVoiceForConditionalGenerationInference.from_pretrained(
            args.model_path,
            torch_dtype=torch.bfloat16,
            device_map="cuda",
            attn_implementation="flash_attention_2",
        )
    except Exception as e:
        print(f"flash_attention_2 load failed ({e}); falling back to SDPA")
        model = VibeVoiceForConditionalGenerationInference.from_pretrained(
            args.model_path,
            torch_dtype=torch.bfloat16,
            device_map="cuda",
            attn_implementation="sdpa",
        )
    model.eval()
    model.set_ddpm_inference_steps(num_steps=10)

    results = []
    offset = 0
    for n in batch_sizes:
        items = [all_texts[(offset + i) % len(all_texts)] for i in range(n)]
        offset += n
        print(f"\n=== batch_size={n} ===")
        try:
            wall, peak_vram, per_item, speech_outputs = run_batch(model, processor, items, voice_path, args.cfg_scale)
        except torch.cuda.OutOfMemoryError as e:
            print(f"OOM at batch_size={n}: {e}")
            torch.cuda.empty_cache()
            gc.collect()
            results.append({"batch_size": n, "oom": True})
            break

        total_audio_secs = sum(p["audio_duration_secs"] for p in per_item if p["audio_duration_secs"] is not None)
        throughput = total_audio_secs / wall if wall > 0 else float("inf")
        peak_vram_mb = peak_vram / (1024 * 1024)
        print(f"wall={wall:.2f}s peak_vram={peak_vram_mb:.0f}MiB "
              f"total_audio={total_audio_secs:.1f}s throughput={throughput:.3f}x")

        save_wavs(processor, speech_outputs, items, args.output_dir, n)

        result = {
            "batch_size": n,
            "wall_secs": wall,
            "peak_vram_mb": peak_vram_mb,
            "total_audio_secs": total_audio_secs,
            "throughput": throughput,
            "per_item": per_item,
            "gpu_name": gpu_name(),
            "cfg_scale": args.cfg_scale,
            "seed": args.seed,
            "timestamp": datetime.now(timezone.utc).isoformat(),
        }
        results.append(result)
        with open(args.results_jsonl, "a") as f:
            f.write(json.dumps(result) + "\n")

    print("\n=== Summary ===")
    for r in results:
        if r.get("oom"):
            print(f"N={r['batch_size']}: OOM")
        else:
            print(f"N={r['batch_size']}: wall={r['wall_secs']:.1f}s "
                  f"peak_vram={r['peak_vram_mb']:.0f}MiB throughput={r['throughput']:.3f}x")

    if args.gcs_bucket:
        upload_to_gcs(args.gcs_bucket, args.gcs_prefix, args.output_dir, args.results_jsonl)


if __name__ == "__main__":
    main()
