# Client-server implementations closely in sync
For specific significant features the client and server need to be carefully kept in sync.

**Sentence splitting (`tts/src/splitter.rs` + `app/frontend/src/markdown.ts`)**

Paragraphs split on `\n\n`; within each paragraph, single newlines are hard breaks and
`unicode_sentences()` / `Intl.Segmenter` find sentence boundaries. Both sides apply the
same two post-processing rules to keep indices in sync:

- **Outline label merge** — a short all-caps or all-lowercase-Roman-numeral label
  (`I.`, `XIV.`, `ii.`, `A.`) is merged with the sentence that follows it.
  Fixes the UAX #29 behaviour where `"I. Introduction"` splits into `["I.", "Introduction"]`.
  
- **No-alpha filter** — sentences with no alphabetic characters are dropped (see below).

The client maps incoming audio segments to `pendingSpans` by **arrival order** (`receivedCount++`
in `player.ts`), not by `msg.index`. Any sentence skipped server-side must be skipped client-side
too, or highlighting drifts. When adding a new server-side filter, add the matching filter in
`splitLines` in `markdown.ts`.

**Sentence filtering**

The engine and the client both skip sentences with no alphabetic content. This handles
footnote markers (`*1*`, `[12]`) that trafilatura includes in `plain_text` as standalone
sentences after Unicode sentence splitting. Skipping symmetrically on both sides keeps
segment indices in sync.

**Pronunciation overrides**

`tts_overrides.txt` at the workspace root defines per-token pronunciation fixes for the F5
normalizer (two-column: `match  replacement`, case-insensitive, `#` comments).

The override table is held in a process-global `Arc<RwLock<HashMap>>` (in
`tts/src/f5/normalizer.rs`) initialized once from disk and live-reloadable — no server restart
needed. `normalize()` acquires a read lock; `add_override` / `remove_override` acquire a write
lock and rewrite `tts_overrides.txt` immediately.

On override change (`POST /overrides` or `DELETE /overrides/:word`):
1. Normalizer map updated in-memory and written to disk.
2. All `~/.odoru/audio/*.json` sidecar files whose stored `text` contains the word are marked
   `invalid: true, invalid_reason: "override"` (the entry is skipped on next cache lookup).
3. The in-memory `SegmentCache` (`DashMap` in `AppState`) is cleared entirely.

The reader's "Fix pronunciation" popover (author path only): select a word in the transcript,
type the phonetic replacement, Save. The server updates the override and the client reloads the
document, triggering re-synthesis of affected sentences (from the F5 model) while unaffected
sentences resolve from disk cache.
