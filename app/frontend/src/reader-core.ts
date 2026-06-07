/**
 * reader-core.ts — shared highlighting, outline, and byline logic used by
 * both the authoring app (main.ts) and the static SPA export (export-reader.ts).
 *
 * IMPORTANT: changes here affect BOTH the live reader and exported SPAs.
 * If you change this file, or anything it calls (markdown.ts, style.css
 * .segment/.pending/.active rules), verify both paths still work.
 */

import { type HeadingEntry } from './markdown'

// ---------------------------------------------------------------------------
// Byline formatting
// ---------------------------------------------------------------------------

export function formatByline(authors: string[], date?: string): string {
  const authorStr = (authors ?? []).length > 0 ? `by ${authors.join(', ')}` : ''
  const dateStr = date
    ? new Date(date + 'T12:00:00').toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' })
    : ''
  if (authorStr && dateStr) return `${authorStr}, ${dateStr}`
  return authorStr || dateStr
}

// ---------------------------------------------------------------------------
// ReaderCore
// ---------------------------------------------------------------------------

export class ReaderCore {
  private spans: HTMLElement[] = []
  private activeSpanIdx = -1
  private readonly outlineEl: HTMLElement

  // Outline state — populated by renderOutline, used by updateOutlineActive
  private headings: HeadingEntry[] = []
  private outlineEls: HTMLElement[] = []
  private activeOutlineIdx = -1

  constructor(_transcriptEl: HTMLElement, outlineEl: HTMLElement) {
    this.outlineEl = outlineEl
  }

  /**
   * Register the current document's sentence spans.
   * Pass `interactive = true` for audio-enabled documents: removes the
   * `.pending` class (which disables pointer events) and wires click-to-seek.
   * Pass a seek callback that will be called with the sentence index on click.
   */
  loadSpans(spans: HTMLElement[], interactive: boolean, onSeek?: (index: number) => void): void {
    this.spans = spans
    this.activeSpanIdx = -1
    if (interactive) {
      spans.forEach((span, index) => {
        span.classList.remove('pending')
        span.style.cursor = 'pointer'
        span.addEventListener('click', () => onSeek?.(index))
      })
    }
  }

  activateSpan(index: number): void {
    const span = this.spans[index]
    if (!span) return
    span.classList.remove('pending')
    span.classList.add('active')
    this.activeSpanIdx = index
    span.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }

  deactivateAll(): void {
    if (this.activeSpanIdx >= 0) {
      this.spans[this.activeSpanIdx]?.classList.remove('active')
      this.activeSpanIdx = -1
    }
  }

  /**
   * Build the outline list from document headings.
   * `onSeek` is called with the heading's sentence index when the user clicks
   * an outline item — the caller decides what that means (seek live synthesis
   * vs seek pre-built audio).
   */
  renderOutline(headings: HeadingEntry[], onSeek: (index: number) => void): void {
    this.headings = headings
    this.outlineEls = []
    this.activeOutlineIdx = -1
    this.outlineEl.innerHTML = ''

    if (headings.length === 0) {
      this.outlineEl.innerHTML = '<div class="outline-loading">No headings</div>'
      return
    }

    const minDepth = Math.min(...headings.map(h => h.depth))
    for (const h of headings) {
      const el = document.createElement('div')
      el.className = 'outline-item'
      el.dataset.depth = String(h.depth - minDepth)
      el.textContent = h.text
      el.addEventListener('click', () => {
        h.element.scrollIntoView({ behavior: 'instant', block: 'start' })
        onSeek(h.sentenceIndex)
      })
      this.outlineEl.appendChild(el)
      this.outlineEls.push(el)
    }
  }

  /**
   * Highlight the outline item whose heading precedes `position` (in seconds).
   * `getTime(sentenceIndex)` should return the start time of that sentence, or
   * null if it is not yet synthesized. Called on every player time-update tick.
   */
  updateOutlineActive(position: number, getTime: (sentenceIndex: number) => number | null): void {
    let found = -1
    for (let i = 0; i < this.headings.length; i++) {
      const t = getTime(this.headings[i].sentenceIndex)
      if (t !== null && t <= position) found = i
      else if (t !== null) break
    }
    if (found === this.activeOutlineIdx) return
    if (this.activeOutlineIdx >= 0) this.outlineEls[this.activeOutlineIdx]?.classList.remove('active')
    this.activeOutlineIdx = found
    if (found >= 0) this.outlineEls[found]?.classList.add('active')
  }
}
