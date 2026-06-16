## Planned improvements

### Authoring

# Done
- Documents are editable — textarea editor with Edit/Preview toggle; auto-save on debounce; re-synth on Preview; see [editing.md](editing.md)
- URL-fetched docs are editable (correct imperfect scraping)
- Title and source URL editable for all docs
- `PATCH /documents/:id` supports `content`, `plain_text`, `title`, `source_url`, `authors`, `date`

# Done?
- Error bar: currently only in Edit view; should be in a shared layout wrapper

# Todo

### Authoring
- build incrementally to support word-level timestamps, pre-req for transclusion
- author may highlight portions of text for their own use (not published)
- highlighted text may be played back crossing sentence boundaries
  - requires word-level timestamps
- Open button in Documents panel: navigate to reader (or editor?) for that document
- upload text/markdown docs to synthesize
- voice picker in reader: wait for more experience with real authoring

#### Deferred
- Outline view for editor
- Transclusion authoring (paste-as-transclusion, refs.json resolution) — see [transclusion.md](transclusion.md)

### Polish / small bugs
- pause/play icons — easy to see state + what action will happen
- Synthesis time display: ~2161m 45s should be H:MM:SS
- Audio disk cache: no eviction — grows unbounded; needs a cleanup strategy (mark-and-sweep; entries already support `invalid: bool` / `invalid_reason` fields for this)

### TTS improvements
- Abbreviation edge cases: `D. C.`, `pp.` not yet handled in sentence splitter

### Open questions / future work
- WS streaming doesn't persist to the audio disk cache (segments are in-memory only). Originally fine when WS was for short snippets, but now authors can seek into long documents via Preview and synthesize large spans that vanish on server restart if the bg job hasn't reached them yet. Consider having WS-synthesized segments also write to the disk cache.

**Future:** a mark-and-sweep GC pass should scan `~/.odoru/audio/` for `invalid: true` entries
(and optionally entries older than a TTL) and delete the `.mp3` + `.json` pair. The `invalid_reason`
field leaves room for additional invalidation sources (`("manual"`, `"ttl"`).

**Mutable text and audio cache invalidation**

If the user edits a sentence, the cached audio for that sentence is stale.
The audio cache key is SHA-256(normalized_text + voice_cache_key) — it will
naturally miss on changed text. `voices.json` status moves to `stale` for all
voices when `PATCH /documents/:id` touches the `content` field; old audio remains
playable with a warning badge. Per-sentence dirty state is more precise but complex.
The future versioning vision (retaining original document) may change what
"invalidation" means entirely. Not needed for now — defer.


---

## Known Issues
- Segfault on CLI exit when `--audio` is used (PyO3/tokio shutdown ordering)
  — all output written successfully before crash
- F5 voice switching reloads the full model (~30s penalty)
- lowercase roman numbers aren't spoken as such -- would need per document
  overrides for when they are sample data (as in authorship paper) or
  kisses (xx or xxx)
