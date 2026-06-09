# Editing

Documents are editable in the Edit view. Both URL-fetched and text-pasted documents can be edited after creation.

## UI fields

Always visible below the URL/Text tabs:
- **Title** — editable text input; auto-saved on 4s debounce (metadata PATCH, no re-synth)
- **Source URL** — editable URL input; for URL docs pre-populated from the fetched URL; for text docs optional provenance; same 4s debounce save
- **UUID** — small selectable text shown below the card once a document ID is known; useful for debugging

## Edit / Preview toggle

A two-state toggle button replaces Synthesize once a document is loaded:

- **Preview** (default when a doc is loaded): article area visible with rendered markdown and sentence spans; textarea hidden
- **Edit**: textarea visible with raw markdown; article area hidden; player stops immediately on entering Edit

Clicking the **Text** tab while a doc is loaded also enters Edit mode (shows the textarea with current content).

## Auto-save (content only)

While in Edit mode, content is saved to the server automatically:
- **Immediately** when the user types `.`, `?`, or `!` (sentence-ending signals)
- **4 seconds** after the last keystroke otherwise

Auto-save calls `PATCH /documents/:id` with `content` + `plain_text`. It does **not** start a synthesis job — that only happens on the Preview toggle.

Title and source URL changes use their own 4s debounce and call `PATCH /documents/:id` with only the metadata fields (no `content`/`plain_text`, no re-synth).

## Preview toggle → re-synth

Clicking Edit → Preview (or clicking Synthesize on the text tab with no doc loaded yet) triggers the full synthesis flow **only if the textarea content has changed** since the last render (`lastRenderedContent` guard):

1. Strip markdown to `plain_text` (via `marked` + tag removal)
2. Re-render article area with `renderMarkdown(raw, plain_text, ...)` so span-to-sentence alignment is correct
3. Cancel any active/pending jobs for this document (`DELETE /jobs/:id` for each)
4. `PATCH /documents/:id` — saves content, marks voices `stale` on the server
5. Restart WS stream — `player.synthesize(plain, voice, spans, docId)` — new spans get audio wired up immediately
6. `POST /jobs` — bg job synthesizes to disk cache for persistence

If content is **unchanged**, toggling Edit → Preview simply shows the article area with the existing live spans and audio intact.

## New document creation (text tab)

When the textarea has content but no doc ID yet, the first save trigger calls `POST /documents` instead of `PATCH`. The returned UUID is stored as `currentDocId` and displayed below the card. All subsequent saves use `PATCH`.

## Voice stale state

`PATCH /documents/:id` with `content` + `plain_text` calls `update_content` server-side, which marks all `ready`/`in_progress` voices as `stale` in `voices.json`. Old audio remains playable (shown with a warning badge in the Documents panel). The new WS stream and bg job re-synthesize with the currently selected voice.

## Server API

See [protocol.md](protocol.md) for the full API. Editing uses `PATCH /documents/:id` (content + metadata) and `POST /documents` (creation); both accept `source_url`.
