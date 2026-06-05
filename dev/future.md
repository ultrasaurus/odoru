# Future Design Notes

## Static Export

See [future-export.md](future-export.md) for the full static export / GitHub Pages design,
including directory structure, audio playback model, and scalability notes.

## Scalability

The dedup indexes (`source_url.json`, `content_hash.json`) are simple JSON files, fine for a personal tool with ~100s of articles. If odoru ever needs to handle many concurrent users or large article counts, these would need to move to a proper database or at minimum a single-writer queue. Not a concern now but worth knowing the boundary.
