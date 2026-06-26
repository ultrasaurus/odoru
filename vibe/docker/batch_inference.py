"""Production batched-generate() entry point for POST /batches.

Reads one JSON batch request from stdin:

    {"segments": [{"text": "...", "name": "seg41"}, ...],
     "seed": 71463, "speaker": "Sarah", "cfg_scale": 1.3,
     "temp": null, "speed": null}

Loads the model once, builds one batch across all segments, runs a
single model.generate() call, and writes one wav per segment to
--output_dir named "<name>.wav". One shared set of generation knobs for
the whole batch (seed/speaker/cfg_scale/temp/speed) — matches current
workflow (one set of knobs per CLI invocation already), no per-segment
override.

Mirrors inference_from_file.py's text-parsing/voice/temp/speed handling
(parse_txt_script, adjust_voice_speed) for behavioral parity with the
existing single-job path — this is the production path, not a
benchmarking tool (see batch_bench.py for that).

Prints "Seed used: <seed>" to stdout once, matching the existing
stdout-scraping pattern run_inference_inner uses today.
"""
import time

# Captured before the heavy imports below (numpy/torch/vibevoice/transformers)
# so [timing] lines can attribute import + CUDA-init cost separately from
# model-load and generation cost — see dev/parallel.md Task 4 (measuring
# whether subprocess-start warmup is paid once per instance or once per
# subprocess).
_T0 = time.time()

import argparse
import json
import os
import re
import sys
import traceback
from typing import List, Tuple

import numpy as np
import torch

from vibevoice.modular.modeling_vibevoice_inference import VibeVoiceForConditionalGenerationInference
from vibevoice.processor.vibevoice_processor import VibeVoiceProcessor
from transformers.utils import logging

logging.set_verbosity_info()
logger = logging.get_logger(__name__)


def adjust_voice_speed(audio_np: np.ndarray, speed_factor: float) -> np.ndarray:
    """Time-stretch a reference voice clip to alter cloned speaking rate. See
    inference_from_file.py's identical helper for the full rationale."""
    original_length = len(audio_np)
    target_length = max(1, int(original_length / speed_factor))
    original_indices = np.arange(original_length)
    target_indices = np.linspace(0, original_length - 1, target_length)
    return np.interp(target_indices, original_indices, audio_np).astype(np.float32)


def parse_txt_script(txt_content: str) -> Tuple[List[str], List[str]]:
    """Identical to inference_from_file.py's parser — kept in sync by hand
    since this script doesn't import from it (separate process, no shared
    package). Pattern: 'Speaker N: ...' lines."""
    lines = txt_content.strip().split('\n')
    scripts, speaker_numbers = [], []
    speaker_pattern = r'^Speaker\s+(\d+):\s*(.*)$'
    current_speaker, current_text = None, ""
    for line in lines:
        line = line.strip()
        if not line:
            continue
        match = re.match(speaker_pattern, line, re.IGNORECASE)
        if match:
            if current_speaker and current_text:
                scripts.append(f"Speaker {current_speaker}: {current_text.strip()}")
                speaker_numbers.append(current_speaker)
            current_speaker = match.group(1).strip()
            current_text = match.group(2).strip()
        else:
            current_text = f"{current_text} {line}" if current_text else line
    if current_speaker and current_text:
        scripts.append(f"Speaker {current_speaker}: {current_text.strip()}")
        speaker_numbers.append(current_speaker)
    return scripts, speaker_numbers


def resolve_voice_path(speaker: str) -> str:
    voices_dir = os.path.join(os.path.dirname(__file__), "voices")
    wav_files = [f for f in os.listdir(voices_dir) if f.lower().endswith('.wav')]
    presets = {os.path.splitext(f)[0]: os.path.join(voices_dir, f) for f in wav_files}
    new_entries = {}
    for name, path in presets.items():
        short = name.split('_')[0] if '_' in name else name
        short = short.split('-')[-1] if '-' in short else short
        new_entries[short] = path
    presets.update(new_entries)

    if speaker in presets:
        return presets[speaker]
    speaker_lower = speaker.lower()
    for preset_name, path in presets.items():
        if preset_name.lower() in speaker_lower or speaker_lower in preset_name.lower():
            return path
    if not presets:
        raise FileNotFoundError(f"No voice files found in {voices_dir}")
    default_voice = next(iter(presets.values()))
    print(f"Warning: No voice preset found for '{speaker}', using default voice: {default_voice}")
    return default_voice


def build_full_script(text: str) -> str:
    """Same reconstruction inference_from_file.py does: parse 'Speaker N:'
    lines, rejoin, normalize the right-single-quote character."""
    scripts, _ = parse_txt_script(text)
    if not scripts:
        raise ValueError("no valid speaker scripts found in segment text")
    return '\n'.join(scripts).replace("’", "'")


def parse_args():
    parser = argparse.ArgumentParser(description="VibeVoice batched production inference")
    parser.add_argument("--model_path", type=str, default="microsoft/VibeVoice-1.5b")
    parser.add_argument("--output_dir", type=str, required=True,
                         help="Directory to write <name>.wav per segment")
    parser.add_argument("--device", type=str,
                         default=("cuda" if torch.cuda.is_available() else "cpu"))
    return parser.parse_args()


def main():
    args = parse_args()
    request = json.load(sys.stdin)

    segments = request["segments"]
    if not segments:
        raise ValueError("segments must be non-empty")
    seed = request.get("seed")
    speaker = request["speaker"]
    cfg_scale = request.get("cfg_scale", 1.3)
    temp = request.get("temp")
    speed = request.get("speed") or 1.0

    print(f"[timing] imports_done: {time.time() - _T0:.2f}s")
    print(f"Using device: {args.device}")
    if seed is not None:
        print(f"Setting seed: {seed}")
        torch.manual_seed(seed)
        if torch.cuda.is_available():
            torch.cuda.manual_seed_all(seed)

    voice_path = resolve_voice_path(speaker)
    print(f"Speaker '{speaker}' -> Voice: {os.path.basename(voice_path)}")

    print(f"Loading processor & model from {args.model_path}")
    processor = VibeVoiceProcessor.from_pretrained(args.model_path)
    print(f"[timing] processor_loaded: {time.time() - _T0:.2f}s")

    load_dtype = torch.bfloat16 if args.device == "cuda" else torch.float32
    attn_impl_primary = "flash_attention_2" if args.device == "cuda" else "sdpa"
    try:
        model = VibeVoiceForConditionalGenerationInference.from_pretrained(
            args.model_path, torch_dtype=load_dtype,
            device_map=args.device, attn_implementation=attn_impl_primary,
        )
    except Exception as e:
        if attn_impl_primary != "flash_attention_2":
            raise
        print(f"[ERROR] : {type(e).__name__}: {e}")
        print(traceback.format_exc())
        print("Falling back to SDPA.")
        model = VibeVoiceForConditionalGenerationInference.from_pretrained(
            args.model_path, torch_dtype=load_dtype,
            device_map=args.device, attn_implementation="sdpa",
        )
    model.eval()
    model.set_ddpm_inference_steps(num_steps=10)
    print(f"[timing] model_loaded: {time.time() - _T0:.2f}s")

    full_scripts = [build_full_script(seg["text"]) for seg in segments]
    names = [seg["name"] for seg in segments]

    voice_for_batch = voice_path
    if speed != 1.0:
        print(f"Applying voice speed factor: {speed}")
        adjusted = adjust_voice_speed(
            processor.audio_processor._load_audio_from_path(voice_path), speed,
        )
        voice_for_batch = adjusted

    inputs = processor(
        text=full_scripts,
        voice_samples=[[voice_for_batch] for _ in full_scripts],
        padding=True,
        return_tensors="pt",
        return_attention_mask=True,
    )
    for k, v in inputs.items():
        if torch.is_tensor(v):
            inputs[k] = v.to(args.device)

    if temp is not None:
        generation_config = {'do_sample': True, 'temperature': temp}
        print(f"Starting generation with cfg_scale: {cfg_scale}, temperature: {temp}")
    else:
        generation_config = {'do_sample': False}
        print(f"Starting generation with cfg_scale: {cfg_scale}")

    print(f"[timing] generation_start: {time.time() - _T0:.2f}s")
    start_time = time.time()
    outputs = model.generate(
        **inputs,
        max_new_tokens=None,
        cfg_scale=cfg_scale,
        tokenizer=processor.tokenizer,
        generation_config=generation_config,
        verbose=True,
        is_prefill=True,
    )
    generation_time = time.time() - start_time
    print(f"Generation time: {generation_time:.2f} seconds")
    print(f"[timing] generation_done: {time.time() - _T0:.2f}s")

    os.makedirs(args.output_dir, exist_ok=True)
    sample_rate = 24000
    total_audio_secs = 0.0
    for name, speech in zip(names, outputs.speech_outputs):
        if speech is None:
            print(f"WARNING: no audio output generated for segment '{name}'")
            continue
        n_samples = speech.shape[-1] if len(speech.shape) > 0 else len(speech)
        duration = n_samples / sample_rate
        total_audio_secs += duration
        output_path = os.path.join(args.output_dir, f"{name}.wav")
        processor.save_audio(speech, output_path=output_path)
        print(f"Saved {name}: {duration:.2f}s -> {output_path}")

    rtf = generation_time / total_audio_secs if total_audio_secs > 0 else float('inf')
    print(f"Batch size: {len(segments)}")
    print(f"Total audio duration: {total_audio_secs:.2f} seconds")
    print(f"RTF (Real Time Factor): {rtf:.2f}x")
    if seed is not None:
        print(f"Seed used: {seed}")


if __name__ == "__main__":
    main()
