import { marked, type Token } from 'marked'

// ---------------------------------------------------------------------------
// Shared markdown rendering — no wasm dependency, used by both the live app
// (via markdown-live.ts's renderMarkdown) and the SPA export
// (renderMarkdownFromEntries below). Keeping this module wasm-free lets the
// export's Vite entry point skip bundling the wasm splitter entirely, which
// matters because the export can be opened via `file://` (no fetch of the
// .wasm asset works there) — see markdown-live.ts for the wasm-backed path.
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

// Strip inline markdown markers to get plain text. Only needs to handle what
// trafilatura produces: bold, italic, code, links. Exported for
// markdown-live.ts's wasm-backed sentence provider.
export function stripInline(text: string): string {
  return text
    .replace(/\*\*(.*?)\*\*/gs, '$1')
    .replace(/__(.*?)__/gs, '$1')
    .replace(/\*(.*?)\*/gs, '$1')
    .replace(/_(.*?)_/gs, '$1')
    .replace(/`(.*?)`/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
}

// ---------------------------------------------------------------------------
// renderMarkdownFromEntries — export-only entry point. (Live app's
// renderMarkdown lives in markdown-live.ts.)
//
// Parses `content` (trafilatura markdown) and appends rendered HTML into
// `container`. Sentence spans are woven into each block element so the
// Player can activate them in place as audio arrives.
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

// The SPA export already knows exact sentence boundaries and per-sentence
// markdown text, computed once at export time by `tts::markdown::split_for_export`
// (no wasm needed). `blockLengths` gives the sentence count for each block in
// the same document-walk order `marked.lexer` produces, so blocks can be
// matched up positionally without re-splitting anything client-side.
export interface ExportSentenceEntry {
  text: string
  markdown_text: string
}

export function renderMarkdownFromEntries(
  content: string,
  entries: ExportSentenceEntry[],
  blockLengths: number[],
  container: HTMLElement,
): RenderResult {
  return renderWithProvider(content, entriesSentenceProvider(entries, blockLengths), container)
}

// Used by markdown-live.ts's renderMarkdown for the live app's wasm-backed
// path — keeps the marked.lexer block walk in one place.
export function renderWithProvider(
  content: string,
  provider: SentenceProvider,
  container: HTMLElement,
): RenderResult {
  const pendingSpans: HTMLElement[] = []
  const headings: HeadingEntry[] = []
  let globalIdx = 0
  const fragment = document.createDocumentFragment()
  const tokens = marked.lexer(content)
  for (const token of tokens) {
    globalIdx = renderToken(token, fragment, provider, globalIdx, pendingSpans, headings)
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
  provider: SentenceProvider,
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
        globalIdx = provider.weave(token.text, el, globalIdx, pendingSpans)
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
        globalIdx = provider.weave(token.text, el, globalIdx, pendingSpans)
      }
      container.appendChild(el)
      break
    }
    case 'blockquote': {
      const el = document.createElement('blockquote')
      el.className = 'md-blockquote'
      for (const child of (token as any).tokens ?? []) {
        globalIdx = renderToken(child, el, provider, globalIdx, pendingSpans, headings)
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
        globalIdx = provider.weave(item.text, li, globalIdx, pendingSpans)
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
        globalIdx = provider.weave(text, el, globalIdx, pendingSpans)
        container.appendChild(el)
      }
    }
  }
  return globalIdx
}

// ---------------------------------------------------------------------------
// Sentence span weaving
//
// renderToken doesn't split text into sentences itself — it delegates to a
// SentenceProvider, since the live app and the SPA export get their
// per-sentence text from different places (wasm split at render time vs.
// a precomputed export payload). Both providers append the same `.segment`
// spans in the same shape; only how they find sentence text differs.
// ---------------------------------------------------------------------------

export interface SentenceProvider {
  // Weaves spans for one block's sentences into `container`, returns the
  // updated globalIdx.
  weave(rawText: string, container: HTMLElement, globalIdx: number, pendingSpans: HTMLElement[]): number
}

// SPA export: sentence counts and markdown text were already computed at
// export time (see `tts::markdown::split_for_export`), so this provider
// only needs to consume `blockLengths`/`entries` in document order — no
// splitting, no wasm.
function entriesSentenceProvider(entries: ExportSentenceEntry[], blockLengths: number[]): SentenceProvider {
  const lengthsQueue = [...blockLengths]
  return {
    weave(_rawText, container, globalIdx, pendingSpans) {
      const count = lengthsQueue.shift() ?? 0
      for (let i = 0; i < count; i++) {
        const entry = entries[globalIdx]
        const span = document.createElement('span')
        span.className = 'segment pending'
        span.innerHTML = marked.parseInline(entry?.markdown_text ?? '') as string
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
