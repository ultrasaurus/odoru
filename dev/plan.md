## Planned improvements

### Authoring

#### Done
- Documents are editable — textarea editor with Edit/Preview toggle; auto-save on debounce; re-synth on Preview; see [editing.md](editing.md)
- URL-fetched docs are editable (correct imperfect scraping)
- Title and source URL editable for all docs
- `PATCH /documents/:id` supports `content`, `plain_text`, `title`, `source_url`, `authors`, `date`

#### Deferred
- Outline view for editor
- Transclusion authoring (paste-as-transclusion, refs.json resolution) — see [transclusion.md](transclusion.md)

### Small authoring bugs / improvements
- Open button in Documents panel: navigate to reader (or editor?) for that document
- upload text/markdown docs to synthesize

#### Open questions for authoring

- voice picker in reader: wait for more experience with real authoring


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

**Future:** a mark-and-sweep GC pass should scan `~/.odoru/audio/` for `invalid: true` entries
(and optionally entries older than a TTL) and delete the `.mp3` + `.json` pair. The `invalid_reason`
field leaves room for additional invalidation sources (`"manual"`, `"ttl"`).

**Mutable text and audio cache invalidation**

If the user edits a sentence, the cached audio for that sentence is stale.
The audio cache key is SHA-256(normalized_text + voice_cache_key) — it will
naturally miss on changed text. `voices.json` status moves to `stale` for all
voices when `PATCH /documents/:id` touches the `content` field; old audio remains
playable with a warning badge. Per-sentence dirty state is more precise but complex.
The future versioning vision (retaining original document) may change what
"invalidation" means entirely. Not needed for now — defer.


### Open questions / future work
- WS streaming doesn't persist to the audio disk cache (segments are in-memory only). Originally fine when WS was for short snippets, but now authors can seek into long documents via Preview and synthesize large spans that vanish on server restart if the bg job hasn't reached them yet. Consider having WS-synthesized segments also write to the disk cache.

### Polish / small bugs
- Error bar: currently only in Edit view; should be in a shared layout wrapper
- pause/play icons — easy to see state + what action will happen
- Synthesis time display: ~2161m 45s should be H:MM:SS
- Audio disk cache: no eviction — grows unbounded; needs a cleanup strategy (mark-and-sweep; entries already support `invalid: bool` / `invalid_reason` fields for this)

#### TTS improvements
- Abbreviation edge cases: `D. C.`, `pp.` not yet handled in sentence splitter

## Known issues
- Segfault on CLI exit when `--audio` is used (PyO3/tokio shutdown ordering)
  — all output written successfully before crash
- F5 voice switching reloads the full model (~30s penalty)
- lowercase roman numbers aren't spoken as such -- would need per document
  overrides for when they are sample data (as in authorship paper) or
  kisses (xx or xxx).