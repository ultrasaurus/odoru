# Kokoro Backend

## Architecture

- **ONNX inference**: pure Rust via `ort` crate — no Python at inference time
- **G2P**: misaki (Python/PyO3) converts text → IPA phonemes; runs on the calling thread
  inside a `tokio::runtime::Runtime::new()` (blocking context, called from `spawn_blocking`)
- **Voice style files**: `~/.kokoro/voices/<name>.bin` — each is a float32 lookup table,
  one 256-float style vector per token index

## Pipeline per sentence

```
text → G2P (misaki Python) → IPA phoneme string
     → tokenize (char → id via tokenizer.json vocab)
     → load_voice_style: voice.bin[n_tokens * 256 .. n_tokens * 256 + 256]
     → ONNX inference: (input_ids, style, speed) → (waveform, durations)
     → return (Vec<f32>, 24_000 Hz, duration_secs)
```

## Voice style file format

- Binary: sequence of f32le values, row-major, shape `[max_tokens, 256]`
- Row `n` is the style vector for a sequence of `n` tokens
- Max supported token count: `file_size_bytes / (256 * 4)`
- Voice name maps directly to filename: `af_heart` → `~/.kokoro/voices/af_heart.bin`
- `KokoroInference` caches max_tokens per voice after the first call (just reads file metadata)

## Token limit and long-sentence splitting

Long sentences can exceed the voice file's max token count, causing a
"Voice file too short for N tokens" error. Handled transparently in
`synthesize_with_voice` (`tts/src/kokoro/mod.rs`):

1. Phonemize + tokenize the full sentence
2. If `token_ids.len() >= max_tokens`: split into sub-groups, synthesize each,
   concatenate audio with 50ms silence between groups
3. Return a single `(Vec<f32>, u32, f64)` — transparent to the engine and client

**Splitting strategy** (`split_into_clauses`):
- `split_on_clause_boundaries`: finds all candidate split points — `;` `:` `—` `,`
  (separator stays with left piece), then ` and ` ` but ` ` or ` ` which ` ` that `
  ` while ` etc. (split before the conjunction)
- Greedy grouping: accumulate pieces until adding the next would push estimated tokens
  ≥ max_tokens (estimated proportionally: `piece.len / text.len * total_tokens`)
- **Word-split fallback**: any piece whose estimated tokens alone exceed max_tokens
  is split into N equal word-count chunks (N = ceil(estimated / max_tokens))
- Clamp safety net: if a group still exceeds max_tokens after all splitting (e.g. single
  very long unbreakable token sequence), truncate to `max_tokens - 1`
- Logs `warn!` with sentence index, token count, and number of groups produced

**Performance**: long sentences are rare; extra G2P calls for split groups are acceptable.

## Timing

- `durations_to_segment`: converts ONNX duration outputs → `Segment {start, end}`
  - Formula: `hf(v) = v * 2.0 / 80.0` (converts duration units to seconds)
  - `durations[0]` = leading silence, `durations[last]` = trailing silence — excluded
    from the spoken span; middle values are phoneme durations
- `audio_duration_from_durations`: sums all duration values for total audio length

## Performance

- ~0.2 sec/word on M1 (fast enough that disk caching was thought to be not needed for Kokoro, but with long documents needed for seeking and upcoming export feature)
- G2P (misaki) is the main latency contributor for short sentences
- ONNX session is shared across sentences via `Mutex<KokoroInference>`

## Known issues / constraints

- **No disk cache**: Kokoro sentences are never written to the audio disk cache
  (`engine.rs` line 87: `false // Kokoro not cached`). This means seeking near the
  end of a long article re-synthesizes all sentences up to that point on every load.
  Disk caching should be added for Kokoro — the seek latency and re-synthesis cost
  will become noticeable on long articles. The cache infrastructure already exists
  (`audio_cache.rs`); Kokoro just needs to opt in the same way F5 does.
- **Single voice per backend instance**: `KokoroBackend` is initialized with one voice;
  voice switching requires a new instance (but is cheap unlike F5's full model reload)
- **Runtime per sentence**: `tokio::runtime::Runtime::new()` is created per
  `synthesize_with_voice` call because this runs inside `spawn_blocking`; acceptable
  overhead given synthesis dominates
