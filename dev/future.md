# Future Design Notes

## Multiple Auhors (hosted server environment)

### Authentication

- evaluate UI surface for APIs that need additonal guards (e.g. delete, patch). 
- Likely need concept of document owner and admin role.

#### Static export
- See [export.md](export.md) for current implementation & CLI usage, meets primary use case of demo deployed via github pages
- Export UI in authoring is expected to be needed when there are multiple users who want to post their projects as static web pages, preconditions:
  - decide if public fetched URLs are shared across users
  - separate document stores per user for orginal in-progress works
  - if public documents are shared, publish choices still need to be per user
- UI design
  - probably a button in the reader
  - Consider a warning for incomplete audio
- Each user needs their own artifact, so zip-download seems the right approach

## Scalability

The dedup indexes (`source_url.json`, `content_hash.json`) are simple JSON files, fine for a personal tool with ~100s of articles. If odoru ever needs to handle many concurrent users or large article counts, these would need to move to a proper database or at minimum a single-writer queue. Not a concern now but worth knowing the boundary.
