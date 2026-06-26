import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ReaderCore, formatByline } from './reader-core'
import { type HeadingEntry } from './markdown'

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn()
})

describe('formatByline', () => {
  it('formats authors only', () => {
    expect(formatByline(['Ada Lovelace'])).toBe('by Ada Lovelace')
  })

  it('formats multiple authors', () => {
    expect(formatByline(['Ada Lovelace', 'Charles Babbage'])).toBe('by Ada Lovelace, Charles Babbage')
  })

  it('formats date only', () => {
    expect(formatByline([], '2024-03-05')).toBe('March 5, 2024')
  })

  it('formats authors and date together', () => {
    expect(formatByline(['Ada Lovelace'], '2024-03-05')).toBe('by Ada Lovelace, March 5, 2024')
  })

  it('returns empty string when neither is present', () => {
    expect(formatByline([])).toBe('')
  })
})

function makeSpan(): HTMLElement {
  const span = document.createElement('span')
  span.className = 'segment pending'
  return span
}

describe('ReaderCore spans', () => {
  it('loadSpans with interactive=true removes pending and wires click-to-seek', () => {
    const core = new ReaderCore(document.createElement('div'), document.createElement('div'))
    const spans = [makeSpan(), makeSpan()]
    const onSeek = vi.fn()
    core.loadSpans(spans, true, onSeek)

    expect(spans[0].classList.contains('pending')).toBe(false)
    spans[1].click()
    expect(onSeek).toHaveBeenCalledWith(1)
  })

  it('loadSpans with interactive=false leaves pending class and no click handler', () => {
    const core = new ReaderCore(document.createElement('div'), document.createElement('div'))
    const spans = [makeSpan()]
    const onSeek = vi.fn()
    core.loadSpans(spans, false, onSeek)

    expect(spans[0].classList.contains('pending')).toBe(true)
    spans[0].click()
    expect(onSeek).not.toHaveBeenCalled()
  })

  it('activateSpan marks the span active and removes pending', () => {
    const core = new ReaderCore(document.createElement('div'), document.createElement('div'))
    const spans = [makeSpan(), makeSpan()]
    core.loadSpans(spans, false)
    core.activateSpan(1)

    expect(spans[1].classList.contains('active')).toBe(true)
    expect(spans[1].classList.contains('pending')).toBe(false)
  })

  it('deactivateAll clears the currently active span', () => {
    const core = new ReaderCore(document.createElement('div'), document.createElement('div'))
    const spans = [makeSpan()]
    core.loadSpans(spans, false)
    core.activateSpan(0)
    core.deactivateAll()

    expect(spans[0].classList.contains('active')).toBe(false)
  })
})

function makeHeading(depth: number, text: string, sentenceIndex: number): HeadingEntry {
  return { depth, text, element: document.createElement(`h${depth}`), sentenceIndex }
}

describe('ReaderCore outline', () => {
  it('renders "No headings" when there are none', () => {
    const outlineEl = document.createElement('div')
    const core = new ReaderCore(document.createElement('div'), outlineEl)
    core.renderOutline([], vi.fn())

    expect(outlineEl.textContent).toContain('No headings')
  })

  it('renders outline items with depth relative to the shallowest heading', () => {
    const outlineEl = document.createElement('div')
    const core = new ReaderCore(document.createElement('div'), outlineEl)
    const headings = [makeHeading(2, 'Section A', 0), makeHeading(3, 'Subsection', 5)]
    core.renderOutline(headings, vi.fn())

    const items = outlineEl.querySelectorAll('.outline-item')
    expect(items).toHaveLength(2)
    expect((items[0] as HTMLElement).dataset.depth).toBe('0')
    expect((items[1] as HTMLElement).dataset.depth).toBe('1')
  })

  it('clicking an outline item calls onSeek with the heading sentenceIndex', () => {
    const outlineEl = document.createElement('div')
    const core = new ReaderCore(document.createElement('div'), outlineEl)
    const onSeek = vi.fn()
    const headings = [makeHeading(1, 'Intro', 0), makeHeading(1, 'Body', 7)]
    core.renderOutline(headings, onSeek)

    const items = outlineEl.querySelectorAll('.outline-item')
    ;(items[1] as HTMLElement).click()

    expect(onSeek).toHaveBeenCalledWith(7)
  })

  it('updateOutlineActive activates the last heading at or before position', () => {
    const outlineEl = document.createElement('div')
    const core = new ReaderCore(document.createElement('div'), outlineEl)
    const headings = [makeHeading(1, 'Intro', 0), makeHeading(1, 'Body', 5), makeHeading(1, 'Conclusion', 10)]
    core.renderOutline(headings, vi.fn())

    const times = [0, 2, 4]
    core.updateOutlineActive(3, i => times[i] ?? null)

    const items = outlineEl.querySelectorAll('.outline-item')
    expect(items[0].classList.contains('active')).toBe(true)
    expect(items[1].classList.contains('active')).toBe(false)
  })

  it('updateOutlineActive treats null times as not-yet-reached and stops scanning', () => {
    const outlineEl = document.createElement('div')
    const core = new ReaderCore(document.createElement('div'), outlineEl)
    const headings = [makeHeading(1, 'Intro', 0), makeHeading(1, 'Body', 5)]
    core.renderOutline(headings, vi.fn())

    core.updateOutlineActive(100, () => null)

    const items = outlineEl.querySelectorAll('.outline-item')
    expect(items[0].classList.contains('active')).toBe(false)
    expect(items[1].classList.contains('active')).toBe(false)
  })
})
