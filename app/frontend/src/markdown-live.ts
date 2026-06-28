import { marked } from 'marked'
import init, { split as wasmSplit } from './wasm/splitter_wasm.js'
import { stripInline, renderWithProvider, type RenderResult, type SentenceProvider } from './markdown'

// Top-level await: blocks ESM evaluation for every importer of this module
// until the wasm splitter is ready, so renderMarkdown (and splitLines) can
// stay synchronous below — reader-author.ts relies on renderMarkdown
// returning synchronously so onSegment callbacks can't race setPendingSpans.
//
// Only the live app (edit.ts/reader-author.ts) imports this module — the
// SPA export uses markdown.ts's renderMarkdownFromEntries directly, which
// has no wasm dependency, so its Vite bundle never pulls in this file or the
// wasm asset. See dev/export.md.
await init()

// ---------------------------------------------------------------------------
// splitLines — shared sentence splitting used for both plain-text (server
// match) and raw markdown (for inline rendering). Calls into the wasm build
// of splitter.rs (splitter-wasm), the same Rust code the server runs, so
// sentence and paragraph boundaries are identical by construction rather
// than by parallel maintenance.
// ---------------------------------------------------------------------------

// Splits a markdown block's text into sentences, treating intra-block single
// newlines as soft breaks (collapsed to a space) rather than hard sentence
// breaks — mirrors the server's CommonMark handling in tts/src/markdown.rs
// (`Event::SoftBreak | Event::HardBreak => current.push(' ')`). Blocks never
// contain blank lines internally, so any `\n` here is a soft/hard break, not
// a paragraph boundary.
function splitBlockText(text: string): string[] {
  return splitLines(text.replace(/\n+/g, ' '))
}

// Same sentence-boundary logic as splitBlockText, but for text that will be
// rendered (not just counted): a CommonMark hard break (line ending in 2+
// spaces, or a backslash) keeps its visual line break, converted to a
// literal <br> so marked.parseInline renders it; a soft break still
// collapses to a space. Used only for rawSentences in weaveSpans — never
// for the count itself, so it must always tokenize into the same number of
// sentences as splitBlockText.
function collapseHardBreaksToBr(text: string): string {
  return text
    .replace(/ {2,}\n/g, ' <br>')
    .replace(/\\\n/g, '<br>')
    .replace(/\n/g, ' ')
}

function splitBlockTextForRender(text: string): string[] {
  return splitLines(collapseHardBreaksToBr(text))
}

function splitLines(text: string): string[] {
  return wasmSplit(text).map(s => s.text)
}

// ---------------------------------------------------------------------------
// renderMarkdown — live app only (editor + author-side reader).
//
// `plainText` is the server's plain-text version of the article — used as
// the source of truth for sentence splitting so client indices match server
// synthesis indices exactly.
// ---------------------------------------------------------------------------

export function renderMarkdown(
  content: string,
  plainText: string,
  container: HTMLElement,
): RenderResult {
  // Split plain_text into sentences — ground truth that matches the server.
  // wasmSplit *is* the server's splitter.rs split(), so paragraph and
  // sentence boundaries (including outline-label merging and abbreviation
  // protection) come out identical by construction.
  const allSentences = wasmSplit(plainText).map(s => s.text)
  return renderWithProvider(content, wasmSentenceProvider(allSentences), container)
}

// Split the block's text into sentences via the wasm splitter.
//
// For each block, we split the raw markdown text into sentences to get the
// renderable (inline-formatted) version of each sentence, and split the
// stripped plain text to know how many sentences this block contributes.
// Those counts should match — if they do, we render spans using the raw
// markdown sentences (so bold/italic display correctly). If they diverge
// (unexpected edge case), we fall back to plain-text sentences from the
// global list so synthesis indices stay aligned.
function wasmSentenceProvider(allSentences: string[]): SentenceProvider {
  return {
    weave(rawText, container, globalIdx, pendingSpans) {
      // Count sentences via stripped plain text — matches what the server
      // sees. Intra-block newlines are soft/hard breaks (collapsed to a
      // space), not sentence boundaries — see splitBlockText.
      const plainSentences = splitBlockText(stripInline(rawText))
      const count = plainSentences.length

      // Raw markdown sentences for inline rendering. Should be the same
      // count. Hard breaks are preserved as <br> (see
      // collapseHardBreaksToBr) so poem-style line breaks still render;
      // only soft breaks collapse to a space here.
      const rawSentences = splitBlockTextForRender(rawText)

      for (let i = 0; i < count; i++) {
        const span = document.createElement('span')
        span.className = 'segment pending'
        if (rawSentences.length === count) {
          // Counts match — render with inline formatting.
          span.innerHTML = marked.parseInline(rawSentences[i]) as string
        } else {
          // Fallback — use plain text from the global list.
          span.textContent = allSentences[globalIdx] ?? plainSentences[i]
        }
        pendingSpans.push(span)
        container.appendChild(span)
        if (i < count - 1) {
          container.appendChild(document.createTextNode(' '))
        }
        globalIdx++
      }

      return globalIdx
    },
  }
}
