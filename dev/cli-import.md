# Design: `dl import` — command surface

Companion to `dev/tts-backends/vibe-import.md` (the vibe-specific import
mechanics: sidecar parsing, document matching, cache-key scheme) and
`dev/tts-backends/vibe-playback.md` (player support for the gaps an
import can leave). This doc is one level up: the overall `dl import`
command surface and how the pieces of the fetch → edit → synthesize →
import workflow connect.

Status: design discussion, not yet implemented.

## Motivation

The recurring workflow this needs to support:

1. Fetch a document (`dl import fetch <url>`) so it exists in Odoru's
   document store.
2. Optionally edit it in the app — title, content, whatever.
3. Export its current text to a file (`dl export-text <doc-id> <path>`)
   so it can be handed to a non-deterministic synthesis source that
   reads from disk, not from Odoru's store directly.
4. Run that source (vibe today; possibly a human read-aloud recording
   later) to produce audio + alignment output.
5. Import the result back into Odoru's audio cache
   (`dl import vibe <basedir>`, designed in `vibe-import.md`).

## Command surface

`dl import` becomes a parent subcommand, one child per source:

- **`dl import fetch <url>`** — ensure a document exists in the store
  for this URL; thin and scriptable (e.g. `doc_id=$(dl import fetch
  <url>)`). See "Relationship to `dl fetch`" below for how this differs
  from the existing `dl fetch` command, which already stores fetched
  URLs as a side effect but is oriented around printing/rendering, not
  scripting.
- **`dl import vibe <basedir>`** — the command designed in
  `vibe-import.md`; nests under `import` rather than standing alone.
- **`dl import read-aloud <path>`** — reserved name only, not designed
  yet. Future: importing a human reading a document aloud. "Narration"
  was considered and rejected — it doesn't necessarily imply reading
  from the document's own text, which is exactly what this feature is.
  "Human" read oddly as a noun. "Read-aloud" is a placeholder, not
  final.
  - Worth noting for later, not designing now: this would face the
    exact same non-determinism problem vibe does (the same person
    reading the same sentence twice sounds different), so it would
    reuse the same per-document, per-sentence-index cache-key scheme
    from `vibe-import.md` unchanged — just a different `voice_id`
    prefix (e.g. `"human:<narrator>"` instead of `"vibevoice:default"`).
    That scheme was already designed generically; this isn't new
    design work, just a future consumer of it.

Outside the `import` tree, one new top-level command:

- **`dl export <doc-id> <path> [--format text|markdown]`** — write a
  document's current text to a file at `<path>`. Default `text`
  (`plain_text` — what vibe's segmenter needs); `--format markdown`
  writes `content` instead. This is the bridge vibe's segmenter needs:
  it reads from `odoru/data/<name>.txt` on disk, not from Odoru's
  document store. Deliberately a separate, explicit command rather
  than a side effect of `dl import fetch` or of editing — the moment
  you export is the moment you're committing to "this is the text I'm
  sending to vibe," which may happen well after fetch (you might
  fetch, then edit in the app, then export only once you're happy with
  the text).

## Relationship to `dl fetch`

`dl fetch <url>` already stores a fetched URL as a side effect of
`load_input` (`cli/src/main.rs` — `create_fetching` + `store_ready` on
the URL branch), but its primary job is rendering: it prints
markdown/text to stdout, optionally synthesizes `--audio` to an MP3
file, and is built for interactive use, not scripting.

`dl import fetch <url>` is the same underlying store-write, exposed as
its own command focused on the *id*, not the rendered output — quiet
by default, prints just the document id (or whatever shape makes it
easy to capture in a shell variable). Two thin layers over the same
`load_input`/store path, not a duplicated implementation.

## Output shape

- **`dl import fetch <url>`** — human-readable by default:
  ```
  doc id: <uuid>
  ```
  `--json` switches to a JSON object (e.g. `{"id": "<uuid>"}`) for
  scripting/tooling that wants to parse rather than scrape stdout.
  Same convention should extend to `dl export` and `dl import vibe`'s
  summary output for consistency, though only `import fetch` is
  decided here.

## Why editing between fetch and import is already safe

No new plumbing needed here — confirmed while designing `vibe-import.md`'s
document-matching step: editing a document's content
(`documents::update_content_in`) never touches the `content_hash`
field, and `vibe-import.md`'s matching logic doesn't rely on that
stored field anyway — it hashes the document's *current* `plain_text`
live, every time, against the sidecar's `source_sha256`. So if you
fetch, edit, then export-text and feed the edited text to vibe, the
resulting `source_sha256` reflects the edited text and matching works
correctly without any extra invalidation step. If you edit *after*
exporting but *before* importing, the import-time hash check in
`vibe-import.md` (verify `source_sha256` against the document) would
correctly flag the mismatch — that's existing behavior, not something
this doc needs to add.

## Single-user, disk-only store

Odoru's document store is plain files on disk (`~/.odoru/documents/`),
and there's currently one user. This means the matching/linear-scan
approach in `vibe-import.md` doesn't need to worry about concurrent
writers racing a fetch against an import, or about the store living
somewhere other than local disk. Noting this as a current constraint,
not a permanent one — if that changes, the matching approach may need
revisiting, but it's not a problem to solve now.

## Open items

- `read-aloud` is entirely undesigned — reserved as a name/slot in the
  command tree, nothing else decided.
- Whether `--json` should extend to `dl export` and `dl import vibe`'s
  summary output too, for consistency with `dl import fetch`.
