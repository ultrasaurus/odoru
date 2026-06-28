import { marked, type Token } from 'marked'
import init, { split as wasmSplit } from './wasm/splitter_wasm.js'

// Top-level await: blocks ESM evaluation for every importer of this module
// until the wasm splitter is ready, so renderMarkdown (and splitLines) can
// stay synchronous below — reader-author.ts relies on renderMarkdown
// returning synchronously so onSegment callbacks can't race setPendingSpans.
await init()

// ---------------------------------------------------------------------------
// Strip inline markdown markers to get plain text for sentence splitting.
// Only needs to handle what trafilatura produces: bold, italic, code, links.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Silent text — bracketed spans followed by a `<!--silent-->` comment, e.g.
// `[Doug Engelbart]<!--silent-->`. Displayed (brackets kept) but excluded from
// TTS and playback. See dev/silent-text.md. Mirrors strip_silent in
// tts/src/markdown.rs.
// ---------------------------------------------------------------------------

const SILENT_SPAN = /\[[^\]]*\]\s*<!--\s*silent\s*-->/g

// True when a whole block (heading/paragraph) is nothing but a single silent
// span — the only case handled in this first pass (mid-sentence inline silent
// is deferred).
const FULLY_SILENT = /^\s*\[[^\]]*\]\s*<!--\s*silent\s*-->\s*$/

function isFullySilent(text: string): boolean {
  return FULLY_SILENT.test(text)
}

// The bracketed display text for a silent span, with the comment stripped and
// the brackets kept (the editorial-insertion convention).
function silentDisplayText(text: string): string {
  return text.replace(/<!--\s*silent\s*-->/g, '').trim()
}

// Outline label for a heading: bracketed display text if silent, otherwise
// the inline-stripped plain text.
function silentOrPlain(text: string): string {
  return isFullySilent(text) ? silentDisplayText(text) : stripInline(text)
}

// Remove silent spans to derive the plain text fed to TTS. Drops a line that
// became empty (or only heading `#` markers) because of the stripping;
// preserves originally-blank lines so paragraph boundaries survive.
export function stripSilent(markdown: string): string {
  const out: string[] = []
  for (const line of markdown.split('\n')) {
    const removed = line.replace(SILENT_SPAN, '')
    const trimmed = removed.trim()
    if (removed !== line && (trimmed === '' || /^#+$/.test(trimmed))) continue
    out.push(removed)
  }
  return out.join('\n')
}

function stripInline(text: string): string {
  return text
    .replace(/\*\*(.*?)\*\*/gs, '$1')
    .replace(/__(.*?)__/gs, '$1')
    .replace(/\*(.*?)\*/gs, '$1')
    .replace(/_(.*?)_/gs, '$1')
    .replace(/`(.*?)`/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
}

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
// renderMarkdown
//
// Parses `content` (trafilatura markdown) and appends rendered HTML into
// `container`. Sentence spans are woven into each block element so the
// Player can activate them in place as audio arrives.
//
// `plainText` is the server's plain-text version of the article — used as
// the source of truth for sentence splitting so client indices match server
// synthesis indices exactly.
//
// Returns `pendingSpans` in synthesis order, and `headings` for the outline.
// ---------------------------------------------------------------------------

export interface HeadingEntry {
  depth: number
  text: string          // plain text, for display in the outline
  element: HTMLElement  // the hN element in the DOM — scroll target
  sentenceIndex: number // global sentence index of the heading's first sentence
}

export interface RenderResult {
  pendingSpans: HTMLElement[]
  headings: HeadingEntry[]
}

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

  const pendingSpans: HTMLElement[] = []
  const headings: HeadingEntry[] = []
  let globalIdx = 0
  const fragment = document.createDocumentFragment()
  const tokens = marked.lexer(content)
  for (const token of tokens) {
    globalIdx = renderToken(token, fragment, allSentences, globalIdx, pendingSpans, headings)
  }
  container.appendChild(fragment)
  return { pendingSpans, headings }
}

// ---------------------------------------------------------------------------
// Block rendering
// ---------------------------------------------------------------------------

// Returns the updated globalIdx after consuming sentences for this token.
function renderToken(
  token: Token,
  container: HTMLElement | DocumentFragment,
  allSentences: string[],
  globalIdx: number,
  pendingSpans: HTMLElement[],
  headings: HeadingEntry[],
): number {
  switch (token.type) {
    case 'heading': {
      const el = document.createElement(`h${token.depth}`)
      el.className = 'md-heading'
      const sentenceIndex = globalIdx
      if (isFullySilent(token.text)) {
        // Display-only navigation heading: shown in body + outline, never
        // spoken. No span woven, globalIdx unchanged, so it points at the
        // next real sentence — the natural scroll target.
        el.classList.add('silent')
        el.textContent = silentDisplayText(token.text)
      } else {
        globalIdx = weaveSpans(token.text, el, allSentences, globalIdx, pendingSpans)
      }
      container.appendChild(el)
      headings.push({ depth: token.depth, text: silentOrPlain(token.text), element: el, sentenceIndex })
      break
    }
    case 'paragraph': {
      const el = document.createElement('p')
      el.className = 'md-paragraph'
      if (isFullySilent(token.text)) {
        el.classList.add('silent')
        el.textContent = silentDisplayText(token.text)
      } else {
        globalIdx = weaveSpans(token.text, el, allSentences, globalIdx, pendingSpans)
      }
      container.appendChild(el)
      break
    }
    case 'blockquote': {
      const el = document.createElement('blockquote')
      el.className = 'md-blockquote'
      for (const child of (token as any).tokens ?? []) {
        globalIdx = renderToken(child, el, allSentences, globalIdx, pendingSpans, headings)
      }
      container.appendChild(el)
      break
    }
    case 'list': {
      const el = document.createElement(token.ordered ? 'ol' : 'ul')
      el.className = 'md-list'
      for (const item of token.items) {
        const li = document.createElement('li')
        li.className = 'md-list-item'
        globalIdx = weaveSpans(item.text, li, allSentences, globalIdx, pendingSpans)
        el.appendChild(li)
      }
      container.appendChild(el)
      break
    }
    case 'code': {
      const pre = document.createElement('pre')
      pre.className = 'md-code'
      const code = document.createElement('code')
      code.textContent = token.text
      pre.appendChild(code)
      container.appendChild(pre)
      break
    }
    case 'hr': {
      container.appendChild(document.createElement('hr'))
      break
    }
    case 'space':
      break
    default: {
      const text = (token as any).text as string | undefined
      if (text?.trim()) {
        const el = document.createElement('p')
        el.className = 'md-paragraph'
        globalIdx = weaveSpans(text, el, allSentences, globalIdx, pendingSpans)
        container.appendChild(el)
      }
    }
  }
  return globalIdx
}

// ---------------------------------------------------------------------------
// Sentence span weaving
//
// For each block, we split the raw markdown text into sentences to get the
// renderable (inline-formatted) version of each sentence, and split the
// stripped plain text to know how many sentences this block contributes.
// Those counts should match — if they do, we render spans using the raw
// markdown sentences (so bold/italic display correctly). If they diverge
// (unexpected edge case), we fall back to plain-text sentences from the
// global list so synthesis indices stay aligned.
// ---------------------------------------------------------------------------

function weaveSpans(
  rawText: string,
  container: HTMLElement,
  allSentences: string[],
  globalIdx: number,
  pendingSpans: HTMLElement[],
): number {
  // Count sentences via stripped plain text — matches what the server sees.
  // Intra-block newlines are soft/hard breaks (collapsed to a space), not
  // sentence boundaries — see splitBlockText.
  const plainSentences = splitBlockText(stripInline(rawText))
  const count = plainSentences.length

  // Raw markdown sentences for inline rendering. Should be the same count.
  // Hard breaks are preserved as <br> (see collapseHardBreaksToBr) so
  // poem-style line breaks still render; only soft breaks collapse to a
  // space here.
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
}
