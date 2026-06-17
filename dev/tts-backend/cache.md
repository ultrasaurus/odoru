# Audio Cache

## In-memory segment cache (`app/src/main.rs`)

- Keyed by SHA-256(voice_id + "|" + text) → `Vec<CachedSegment>`
- Populated after a full WS synthesis session completes
- Cache hit: entire segment list streamed to client without touching disk or
  re-synthesizing
- Cleared when a pronunciation override is added or removed
  - TODO: this may be the wrong behavior in the rare case where two documents
    have the same sentence and voice.

## Audio disk cache (`tts/src/audio_cache.rs`)

- Location: `~/.odoru/audio/`
- Files: `<hash>.mp3` + `<hash>.json` (metadata)
- Key: SHA-256(text + "|" + voice_cache_key) — voice params (speed,
  cfg_strength) are part of the key, so changing them busts the cache
- Both Kokoro and F5 use this cache; key difference:
  - **F5**: key uses *normalised* text (post-normalizer output)
  - **Kokoro**: key uses raw sentence text (misaki handles raw text directly)
- `exists(key)` — checks file presence only; used by `all_audio_cached` to
  decide whether to show "Synthesize" button
- `lookup(key)` — returns `(mp3_bytes, duration_secs)` on hit
- `store(key, text, mp3_bytes, duration)` — written after synthesis;
  called via `spawn_blocking` (blocking I/O)
- `invalidate_word(word)` — scans all `.json` entries and marks any whose
  `text` contains `word` as `invalid: true` (with `invalid_reason:
  "override"`); triggered when a pronunciation override is added or removed
- Metadata sidecar fields: `text`, `duration`, `invalid`, `invalid_reason`
- `invalid: true` entries are skipped by `lookup` (treated as cache miss)

### Per-sentence synthesis lock

`TtsEngine` holds a `DashMap` of per-sentence locks keyed by the disk cache
key. Before synthesizing a sentence, the engine acquires the lock, re-checks
the disk cache (post-lock), synthesizes on miss, writes to disk, then releases.
This prevents two concurrent callers (e.g. a live WS session and a background
job) from synthesizing the same sentence simultaneously — the second waiter
gets a disk cache hit instead.

### Resumable synthesis

Because every completed sentence is written to disk immediately, a synthesis
job (or WS session) can be interrupted and restarted cheaply: already-completed
sentences load from disk in milliseconds.

## Future work

See [future.md](../future.md) for open questions on cache eviction and stale-audio
invalidation.
