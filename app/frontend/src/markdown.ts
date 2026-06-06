import { marked, type Token } from 'marked'

// ---------------------------------------------------------------------------
// Abbreviation protection — mirrors server-side splitter.rs ABBREVS list.
// Replaces the trailing period of each abbreviation with a placeholder so
// the sentence segmenter doesn't treat it as a sentence boundary.
// ---------------------------------------------------------------------------

const PLACEHOLDER = '￾'

const ABBREVS = [
  // Titles
  'Mr', 'Mrs', 'Ms', 'Miss', 'Dr', 'Prof', 'Rev', 'Sr', 'Jr',
  // Geographic
  'St', 'Ave', 'Blvd', 'Rd', 'Mt', 'Dept',
  // Latin
  'vs', 'etc', 'e.g', 'i.e', 'et al',
  // Months
  'Jan', 'Feb', 'Mar', 'Apr', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
  // Corporate
  'Corp', 'Inc', 'Ltd', 'Est',
]

function protectAbbrevs(text: string): string {
  let out = text
  for (const abbrev of ABBREVS) {
    out = out.replaceAll(`${abbrev}.`, `${abbrev}${PLACEHOLDER}`)
  }
  return out
}

function restorePlaceholders(text: string): string {
  return text.replaceAll(PLACEHOLDER, '.')
}

// ---------------------------------------------------------------------------
// Strip inline markdown markers to get plain text for sentence splitting.
// Only needs to handle what trafilatura produces: bold, italic, code, links.
// ---------------------------------------------------------------------------

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
// match) and raw markdown (for inline rendering). Mirrors split_paragraph
// in splitter.rs: single newlines are hard breaks, Unicode sentence
// boundaries are found within each line, abbreviations are protected.
// ---------------------------------------------------------------------------

function splitLines(text: string): string[] {
  const sentences: string[] = []
  for (const line of text.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed) continue
    const protected_ = protectAbbrevs(trimmed)
    if (segmenter) {
      for (const { segment } of segmenter.segment(protected_)) {
        const s = restorePlaceholders((segment as string).trim())
        if (s) sentences.push(s)
      }
    } else {
      protected_.split(/(?<=[.!?])\s+/).forEach(s => {
        const r = restorePlaceholders(s.trim())
        if (r) sentences.push(r)
      })
    }
  }
  return sentences
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
  // Server splits on \n\n for paragraphs, then single \n + unicode_sentences
  // within each paragraph. Mirror that here.
  const allSentences: string[] = []
  for (const para of plainText.split(/\n\n+/).map(p => p.trim()).filter(Boolean)) {
    allSentences.push(...splitLines(para))
  }

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
      globalIdx = weaveSpans(token.text, el, allSentences, globalIdx, pendingSpans)
      container.appendChild(el)
      headings.push({ depth: token.depth, text: stripInline(token.text), element: el, sentenceIndex })
      break
    }
    case 'paragraph': {
      const el = document.createElement('p')
      el.className = 'md-paragraph'
      globalIdx = weaveSpans(token.text, el, allSentences, globalIdx, pendingSpans)
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

const segmenter: Intl.Segmenter | null =
  typeof Intl !== 'undefined' && 'Segmenter' in Intl
    ? new (Intl as any).Segmenter('en', { granularity: 'sentence' })
    : null

function weaveSpans(
  rawText: string,
  container: HTMLElement,
  allSentences: string[],
  globalIdx: number,
  pendingSpans: HTMLElement[],
): number {
  // Count sentences via stripped plain text — matches what the server sees.
  const plainSentences = splitLines(stripInline(rawText))
  const count = plainSentences.length

  // Raw markdown sentences for inline rendering. Should be the same count.
  const rawSentences = splitLines(rawText)

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
