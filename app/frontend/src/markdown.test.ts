import { describe, expect, it } from 'vitest'
import { stripSilent, renderMarkdownFromEntries, type ExportSentenceEntry } from './markdown'
import { renderMarkdown } from './markdown-live'

function render(content: string, plainText: string) {
  const container = document.createElement('div')
  const result = renderMarkdown(content, plainText, container)
  return { container, ...result }
}

describe('stripSilent', () => {
  it('removes a fully-silent inline line entirely', () => {
    const input = 'Before\n[Doug Engelbart]<!--silent-->\nAfter'
    expect(stripSilent(input)).toBe('Before\nAfter')
  })

  it('removes a fully-silent heading entirely', () => {
    const input = '# [Navigation]<!--silent-->\n\nBody text'
    expect(stripSilent(input)).toBe('\nBody text')
  })

  it('leaves normal text and blank-line paragraph spacing untouched', () => {
    const input = 'First paragraph.\n\nSecond paragraph.'
    expect(stripSilent(input)).toBe(input)
  })

  it('strips a mid-line silent span but keeps the rest of the line', () => {
    const input = 'Some text [Doug Engelbart]<!--silent--> continues here.'
    expect(stripSilent(input)).toBe('Some text  continues here.')
  })
})

describe('renderMarkdown — silent text', () => {
  it('renders a fully-silent heading with no spans and unchanged globalIdx', () => {
    const content = '# [Navigation]<!--silent-->\n\nFirst sentence. Second sentence.'
    const plainText = 'First sentence. Second sentence.'
    const { container, pendingSpans, headings } = render(content, plainText)

    const h1 = container.querySelector('h1')!
    expect(h1.classList.contains('silent')).toBe(true)
    expect(h1.textContent).toBe('[Navigation]')
    expect(h1.querySelector('.segment')).toBeNull()

    // Heading consumed no sentences — first real sentence is still index 0.
    expect(headings[0].sentenceIndex).toBe(0)
    expect(pendingSpans).toHaveLength(2)
    expect(pendingSpans[0].textContent).toBe('First sentence.')
  })

  it('renders a fully-silent paragraph with no spans woven', () => {
    const content = '[Doug Engelbart]<!--silent-->\n\nReal paragraph text here.'
    const plainText = 'Real paragraph text here.'
    const { container, pendingSpans } = render(content, plainText)

    const silentP = container.querySelector('p.silent')!
    expect(silentP.textContent).toBe('[Doug Engelbart]')
    expect(silentP.querySelector('.segment')).toBeNull()
    expect(pendingSpans).toHaveLength(1)
    expect(pendingSpans[0].textContent).toBe('Real paragraph text here.')
  })
})

describe('renderMarkdown — soft breaks', () => {
  it('does not inflate sentence count when a paragraph is soft-wrapped', () => {
    // Regression: a single intra-paragraph newline must not be treated as an
    // extra sentence boundary on the client when the server collapses it to
    // a space (see tts/src/markdown.rs to_plain_text).
    const content = 'This paragraph has a soft break\nright here, mid-sentence.'
    const plainText = 'This paragraph has a soft break right here, mid-sentence.'
    const { pendingSpans } = render(content, plainText)

    expect(pendingSpans).toHaveLength(1)
    expect(pendingSpans[0].textContent).toBe(plainText)
  })

  it('keeps later blocks aligned with server sentence indices after a soft-wrapped paragraph', () => {
    const content =
      'First sentence wraps\nacross two lines. Second sentence, same paragraph.\n\n' +
      'Third sentence in its own paragraph.'
    const plainText =
      'First sentence wraps across two lines. Second sentence, same paragraph.\n\n' +
      'Third sentence in its own paragraph.'
    const { pendingSpans } = render(content, plainText)

    expect(pendingSpans).toHaveLength(3)
    expect(pendingSpans[2].textContent).toBe('Third sentence in its own paragraph.')
  })

  it('renders bold/italic markers correctly across a soft-wrapped sentence', () => {
    const content = 'This has **bold text**\nwrapped across a soft break.'
    const plainText = 'This has bold text wrapped across a soft break.'
    const { pendingSpans } = render(content, plainText)

    expect(pendingSpans).toHaveLength(1)
    expect(pendingSpans[0].innerHTML).toContain('<strong>bold text</strong>')
  })
})

describe('renderMarkdown — hard breaks', () => {
  it('renders a trailing-double-space hard break as <br>, collapsing to one sentence', () => {
    const content = 'Roses are red,  \nviolets are blue,  \nthis line ends the poem.'
    const plainText = 'Roses are red, violets are blue, this line ends the poem.'
    const { pendingSpans } = render(content, plainText)

    expect(pendingSpans).toHaveLength(1)
    expect(pendingSpans[0].innerHTML).toContain('<br>')
    // Two hard breaks in the source -> two <br> in the rendered span.
    expect(pendingSpans[0].innerHTML.match(/<br>/g)).toHaveLength(2)
  })

  it('renders a backslash hard break as <br>', () => {
    const content = 'Line one ends here\\\nand line two continues the poem.'
    const plainText = 'Line one ends here and line two continues the poem.'
    const { pendingSpans } = render(content, plainText)

    expect(pendingSpans).toHaveLength(1)
    expect(pendingSpans[0].innerHTML).toContain('<br>')
  })

  it('keeps multi-sentence index alignment when a later block follows a hard-break poem', () => {
    const content =
      'Roses are red,  \nviolets are blue.\n\n' +
      'A normal paragraph with two sentences. Right here.'
    const plainText =
      'Roses are red, violets are blue.\n\n' +
      'A normal paragraph with two sentences. Right here.'
    const { pendingSpans } = render(content, plainText)

    expect(pendingSpans).toHaveLength(3)
    expect(pendingSpans[1].textContent).toBe('A normal paragraph with two sentences.')
    expect(pendingSpans[2].textContent).toBe('Right here.')
  })
})

describe('renderMarkdown — no-alpha segment merging', () => {
  it('keeps a closing quote after a spaced ellipsis attached to its sentence, not dropped', () => {
    // Regression: Intl.Segmenter (unlike the Rust unicode_segmentation crate)
    // correctly returns the closing quote+period as their own segment here
    // rather than dropping them — but splitLines used to filter out any
    // no-letter segment entirely, silently losing it. Must now match the
    // server (splitter.rs's recover_dropped_chars), which keeps the quote
    // attached to the sentence it closes.
    const text =
      'and then we would say, "But wait, there\'s more . . . ". ' +
      'And then we would play peek-a-boo.'
    const { pendingSpans } = render(text, text)

    expect(pendingSpans).toHaveLength(2)
    expect(pendingSpans[0].textContent).toBe(
      'and then we would say, "But wait, there\'s more . . . ".'
    )
    expect(pendingSpans[1].textContent).toBe('And then we would play peek-a-boo.')
  })
})

describe('renderMarkdown — plain_text with no blank lines', () => {
  it('still resolves paragraph boundaries when plain_text has single newlines only', () => {
    // Odoru's plain_text: one paragraph per line, zero blank lines at all
    // (no \n\n anywhere) — must still split into separate paragraphs/sentences,
    // not collapse into one.
    const content = 'First paragraph.\n\nSecond paragraph. Has two sentences.'
    const plainText = 'First paragraph.\nSecond paragraph. Has two sentences.'
    const { pendingSpans } = render(content, plainText)

    expect(pendingSpans).toHaveLength(3)
    expect(pendingSpans[0].textContent).toBe('First paragraph.')
    expect(pendingSpans[1].textContent).toBe('Second paragraph.')
    expect(pendingSpans[2].textContent).toBe('Has two sentences.')
  })
})

describe('renderMarkdownFromEntries — SPA export path', () => {
  function renderFromEntries(content: string, entries: ExportSentenceEntry[], blockLengths: number[]) {
    const container = document.createElement('div')
    const result = renderMarkdownFromEntries(content, entries, blockLengths, container)
    return { container, ...result }
  }

  it('weaves spans from precomputed entries with no splitting', () => {
    const content = '# Title\n\nFirst **bold** sentence. Second sentence.'
    const entries: ExportSentenceEntry[] = [
      { text: 'Title', markdown_text: 'Title' },
      { text: 'First bold sentence.', markdown_text: 'First **bold** sentence.' },
      { text: 'Second sentence.', markdown_text: 'Second sentence.' },
    ]
    const { pendingSpans, headings } = renderFromEntries(content, entries, [1, 2])

    expect(pendingSpans).toHaveLength(3)
    expect(pendingSpans[0].textContent).toBe('Title')
    expect(pendingSpans[1].innerHTML).toBe('First <strong>bold</strong> sentence.')
    expect(pendingSpans[2].textContent).toBe('Second sentence.')
    expect(headings).toHaveLength(1)
    expect(headings[0].sentenceIndex).toBe(0)
  })

  it('respects fully-silent headings without consuming an entry', () => {
    const content = '# [Navigation]<!--silent-->\n\nFirst sentence.'
    const entries: ExportSentenceEntry[] = [
      { text: 'First sentence.', markdown_text: 'First sentence.' },
    ]
    const { pendingSpans, headings } = renderFromEntries(content, entries, [1])

    expect(pendingSpans).toHaveLength(1)
    expect(pendingSpans[0].textContent).toBe('First sentence.')
    expect(headings[0].text).toBe('[Navigation]')
    expect(headings[0].sentenceIndex).toBe(0)
  })
})
