# Future Design Notes

## Scalability

The dedup indexes (`source_url.json`, `content_hash.json`) are simple JSON files, fine for a personal tool with ~100s of articles. If odoru ever needs to handle many concurrent users or large article counts, these would need to move to a proper database or at minimum a single-writer queue. Not a concern now but worth knowing the boundary.
