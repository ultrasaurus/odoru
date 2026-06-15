## How source document tracks inbound references

The source document tracks inbound references.  Each document that has been 
transcluded (the source document) has a `refs.json` sidecar in its document 
store directory (`~/.odoru/documents/{uuid}/refs.json`). 
This is machine-maintained — updated whenever a transclusion of that 
document is pasted into any other document.

Each entry records:
- Word offset range in the source document (start/end in `sentence:char`
  format, resolved at paste time)
- The verbatim quoted snippet
- The referring document's ID and title
- The full citation string from the transclusion link

Example:
```json
[
  {
    "citation": "Bush, V., 'As We May Think.' The Atlantic Monthly, p. 101-108; July, 1945.",
    "cited-by": [
      {
        "referrer_title": "Augmenting Human Intellect",
        "snippet": "Consider a future device for individual use",
        "start": "33:0",
        "end": "33:46",       
        "referrer_id": "-b8tRS7h4TJ2Vt43Dp85v2A",
        "resolved_at": "2026-06-08"
      },
      {
        "referrer_title": "Augmenting Human Intellect",
        "start": "12:7",
        "end": "12:39",
        "snippet": "the growing mountain of research",
        "referrer_id": "-b8tRS7h4TJ2Vt43Dp85v2A",
        "resolved_at": "2026-06-08"
      },
      {
        "referrer_title": "Next generation hypertext",
        "snippet": "One cannot hope thus to equal the speed and flexibility with which the mind follows an associative trail, but it should be possible to beat the mind decisively in regard to the permanence and clarity of the items resurrected from storage.",
        "start": "132:0",
        "end": "132:238",
        "referrer_id": "-TzuJsil8RzSy_CEf8b9LFA",
        "resolved_at": "2026-06-15"
      }
    ]
  }
]
```

The top-level array is grouped by citation (the source document's bibliographic
identity). Within each group, `cited-by` is a flat array of individual
transclusion events — `referrer_id` and `referrer_title` repeat per ref, but
in practice most docs will have only one or two refs per citation. Grouping by
referrer in the UI is straightforward by filtering on `referrer_id`.

The `resolved_at` date provides the basis for future drift detection: if B's text later changes, the stored sentence range may no longer match.

For stage 1, transcluded text appears verbatim in the referencing document's
markdown. The blockquote content must exactly match a passage in the source
document.
