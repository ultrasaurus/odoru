import { describe, expect, it } from 'vitest'
import { renderMarkdown, stripSilent } from './markdown'

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
