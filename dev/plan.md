# Planned improvements

## Authoring
build incrementally to support word-level timestamps, pre-req for transclusion
- √ author may highlight portions of text for their own use (not published)
  see [annotation.md](annotation.md)
- √ highlighted text may be played back crossing sentence boundaries
  (equires word-level timestamps)


## Polish / small bugs
- Synthesis time display: ~2161m 45s should be H:MM:SS
- Error bar: currently only in Edit view; should be in a shared layout wrapper
- Outline view for editor

## Later (or as needed to streamline testing or authoring of demo)
- Transclusion (paste-as-transclusion), waiting on dependencies — see [transclusion.md](transclusion.md)
- voice picker in reader: wait for more experience with real authoring
- upload text/markdown docs to synthesize
- Open button in Documents panel: navigate to reader (or editor?) for that document

## TTS improvements
- Abbreviation edge cases: `D. C.`, `pp.` not yet handled in sentence splitter
- Move common code into forced-alignment crate (rename audio-text):
  - `tts` crate has its own sentence splitter (also implemented client-side),
    which is more document-aware. The client-side could be address by
    compilation to WASM. Would be nice to consolidate this code in one place
    that might also receive future attention from open source developers.
    Deferred until implementation is more mature.
  - normalize is another candidate for audio-text crate. Deferred until we have
    per document normalization, so we move cruft out of `tts_overrides.txt`

# Open questions / future work

## trafilatura - imperfect HTML => Markdown
Long-term would like this to be native in Rust. For now collecting un-handled
cases that were found in testing, not docs that matter to me
- import https://www.theblogstarter.com/html-for-beginners/ fails to produce correct markdown.
  Original page:  
  `If we add &lt;b&gt;This is some text.&lt;/b&gt; to our HTML file`  
  Incorrect markdown:  
  `If we add <b>This is some text.</b> to our HTML file`


## Mutable text and audio cache invalidation
See [tts-backend/cache.md](tts-backend/cache.md) for cache details.

If the user edits a sentence, the cached audio for that sentence is stale.
The audio cache key is SHA-256(normalized_text + voice_cache_key) — it will
naturally miss on changed text. `voices.json` status moves to `stale` for all
voices when `PATCH /documents/:id` touches the `content` field;  Per-sentence
dirty state is more precise but complex.
The future versioning vision (retaining original document) may change what
"invalidation" means entirely. Not needed for now — defer.

# Known Issues
- Segfault on CLI exit when `--audio` is used (PyO3/tokio shutdown ordering)
  — all output written successfully before crash
- F5 voice switching reloads the full model (~30s penalty)
- lowercase roman numbers aren't spoken as such -- would need per document
  overrides for when they are sample data (as in authorship paper) or
  kisses (xx or xxx)
