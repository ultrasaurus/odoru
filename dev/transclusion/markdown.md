## Markdown syntax

### Links
Document links are composed of a dash "-" followed by a Base64-encoded UUIDv4.
This is compatible with standard markdown as a relative link for graceful degradation. In an odoru site, all the links created within the system would be UUIDs and so other links would be preceded by a URL schema which must start with a letter.

### Block transclusion

Used for standalone quoted passages (blockquotes).

```markdown
<!-- transclude -->
> Consider a future device for individual use, which is a sort of
> mechanized private file and library...It is an enlarged intimate
> supplement to his memory.
[(1, 106-7)](-pTbltZDkQTGC4QAAyGz4Gw "Bush, V., 'As We May Think.' The Atlantic Monthly, p. 101-108; July, 1945.")
```

- `<!-- transclude -->` immediately before a blockquote marks it as a transclusion (not a regular blockquote)
- The blockquote contains the verbatim quoted text
- The citation link immediately after the blockquote provides:
  - **link text** — page/section reference (e.g. `(1, 106-7)`)
  - **href** — relative link identifier of the source document
  - **title** — full bibliographic citation string

### Inline transclusion

Used for quoted passages embedded mid-paragraph. The format of the link 
indicates that it is a transclusion.

```markdown
He summarized the situation: ["the growing mountain of research...square-rigged ships"](-pTbltZDkQTGC4QAAyGz4Gw "Bush, V., 'As We May Think.' The Atlantic Monthly, p. 101-108; July, 1945.")
```

