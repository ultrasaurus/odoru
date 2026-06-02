"""
tts.py — Python TTS layer called from Rust via PyO3.

Exposes:
    synthesize_sentence(text, backend, voice_ref=None, ref_text=None,
                        speed=0.85, cfg_strength=2.0) -> dict
        {"samples": list[float], "sample_rate": int, "duration": float}

Intentionally thin — no async, no orchestration, no timestamps.
All streaming and sequencing logic lives in Rust (engine.rs).
"""
from __future__ import annotations
import numpy as np


# ---------------------------------------------------------------------------
# Mock backend — sine-wave tone, no model weights needed
# ---------------------------------------------------------------------------

def _synthesize_mock(text: str) -> dict:
    import math
    sample_rate = 24_000
    duration = max(0.3, len(text.split()) * 0.35)  # ~350ms per word
    n = int(sample_rate * duration)
    samples = [0.3 * math.sin(2 * math.pi * 440 * i / sample_rate) for i in range(n)]
    return {"samples": samples, "sample_rate": sample_rate, "duration": duration}


# ---------------------------------------------------------------------------
# F5-TTS backend (MLX, M1/M2/M3 Mac only)
# ---------------------------------------------------------------------------

SAMPLE_RATE = 24_000
TARGET_RMS  = 0.1

_f5_state = None  # (model, ref_audio, ref_text)

def _load_f5(voice_ref: str, ref_text: str):
    global _f5_state
    if _f5_state is not None and _f5_state[2] == ref_text:
        return

    import mlx.core as mx
    import soundfile as sf
    from f5_tts_mlx.cfm import F5TTS  # type: ignore

    model = F5TTS.from_pretrained(
        "lucasnewman/f5-tts-mlx",
        quantization_bits=4,
    )

    audio, sr = sf.read(voice_ref)
    if sr != SAMPLE_RATE:
        raise ValueError(
            f"Reference audio must be 24 kHz (got {sr} Hz). "
            "Convert with: ffmpeg -i input.wav -ac 1 -ar 24000 ref.wav"
        )

    audio = mx.array(audio)
    rms = mx.sqrt(mx.mean(mx.square(audio)))
    if rms < TARGET_RMS:
        audio = audio * TARGET_RMS / rms

    _f5_state = (model, audio, ref_text)


def _synthesize_f5(text: str, voice_ref: str, ref_text: str,
                   speed: float = 0.85, cfg_strength: float = 2.0) -> dict:
    import mlx.core as mx
    from f5_tts_mlx.utils import convert_char_to_pinyin  # type: ignore

    _load_f5(voice_ref, ref_text)
    model, ref_audio, _ = _f5_state

    combined = convert_char_to_pinyin([ref_text + " " + text])

    wave, _ = model.sample(
        mx.expand_dims(ref_audio, axis=0),
        text=combined,
        duration=None,
        steps=8,
        method="rk4",
        speed=speed,
        cfg_strength=cfg_strength,
        sway_sampling_coef=-1.0,
        seed=None,
    )

    wave = wave[ref_audio.shape[0]:]
    mx.eval(wave)

    samples = np.array(wave).astype("float32")
    duration = len(samples) / SAMPLE_RATE
    return {"samples": samples.tolist(), "sample_rate": SAMPLE_RATE, "duration": duration}


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------

def synthesize_sentence(
    text: str,
    backend: str = "mock",
    voice_ref: str | None = None,
    ref_text: str | None = None,
    speed: float = 0.85,
    cfg_strength: float = 2.0,
) -> dict:
    if backend == "mock":
        return _synthesize_mock(text)
    elif backend == "f5_tts":
        if voice_ref is None:
            raise ValueError("F5-TTS requires voice_ref")
        if ref_text is None:
            raise ValueError("F5-TTS requires ref_text")
        return _synthesize_f5(text, voice_ref, ref_text, speed=speed, cfg_strength=cfg_strength)
    else:
        raise ValueError(f"Unknown backend: {backend!r}")
