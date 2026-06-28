import { describe, expect, it } from 'vitest'
import { findAnnotationWordRange, findAnnotationWordRangeByContext, type WordEntry } from './annotations'

describe('findAnnotationWordRange', () => {
  it('finds a literal match and stops just before the next word', () => {
    const words: WordEntry[] = [
      { word: 'Hello', start: 0.0, end: 0.3 },
      { word: 'world', start: 0.4, end: 0.7 },
      { word: 'today', start: 0.8, end: 1.1 },
    ]
    const range = findAnnotationWordRange('world', words)
    expect(range).not.toBeNull()
    expect(range!.start).toBeCloseTo(0.33, 2)  // pre-roll, clamped to not overlap "Hello"
    expect(range!.end).toBeCloseTo(0.77, 2)  // stop just before "today" onset
  })

  it('returns null when the annotated text is not in the words at all', () => {
    const words: WordEntry[] = [
      { word: 'Steve', start: 0.0, end: 0.3 },
      { word: "Feiner's", start: 0.4, end: 0.7 },
      { word: 'P', start: 0.8, end: 0.9 },
      { word: '.', start: 0.95, end: 1.0 },
      { word: '.', start: 1.05, end: 1.1 },
      { word: 'dissertation', start: 1.2, end: 1.8 },
    ]
    // Real-world case: forced alignment mis-transcribed "Ph.D." as "P . .".
    expect(findAnnotationWordRange('Ph.D.', words)).toBeNull()
  })
})

describe('findAnnotationWordRangeByContext', () => {
  it('finds the surrounding words and plays the span between them, even though the annotation text itself never appears in the alignment', () => {
    // Same garbled alignment as the literal-match test above — "Ph.D." was
    // mis-transcribed as "P . .", but "Feiner's" and "dissertation" (its
    // neighbors in the actual sentence text) aligned correctly.
    const words: WordEntry[] = [
      { word: 'Steve', start: 0.49, end: 0.79 },
      { word: "Feiner's", start: 0.85, end: 1.27 },
      { word: 'P', start: 1.37, end: 1.39 },
      { word: '.', start: 1.71, end: 1.75 },
      { word: '.', start: 1.85, end: 1.87 },
      { word: 'dissertation', start: 2.07, end: 2.75 },
    ]
    const sentence = "Steve Feiner's Ph.D. dissertation and his work since then describes such automated authoring."
    const range = findAnnotationWordRangeByContext(sentence, 'Ph.D.', words)
    expect(range).not.toBeNull()
    expect(range!.start).toBeCloseTo(1.27, 2)  // end of "Feiner's"
    expect(range!.end).toBeCloseTo(2.07, 2)    // start of "dissertation"
  })

  it('pads with a flat buffer when the annotation is at the very start of the sentence (no before-word)', () => {
    const words: WordEntry[] = [
      { word: 'Whatever', start: 0.0, end: 0.0 },  // garbled stand-in for the annotated word
      { word: 'world', start: 0.5, end: 0.8 },
    ]
    const sentence = 'Whatever world'
    const range = findAnnotationWordRangeByContext(sentence, 'Whatever', words)
    expect(range).not.toBeNull()
    expect(range!.end).toBeCloseTo(0.5, 2)  // start of "world"
    expect(range!.start).toBeCloseTo(0.35, 2)  // 0.5 - FLAT_BUFFER
  })

  it('pads with a flat buffer when the annotation is at the very end of the sentence (no after-word)', () => {
    const words: WordEntry[] = [
      { word: 'Hello', start: 0.0, end: 0.3 },
      { word: 'Whatever', start: 0.0, end: 0.0 },  // garbled stand-in for the annotated word
    ]
    const sentence = 'Hello Whatever'
    const range = findAnnotationWordRangeByContext(sentence, 'Whatever', words)
    expect(range).not.toBeNull()
    expect(range!.start).toBeCloseTo(0.3, 2)  // end of "Hello"
    expect(range!.end).toBeCloseTo(0.45, 2)  // 0.3 + FLAT_BUFFER
  })

  it('returns null when the annotated text is not even in the sentence text', () => {
    const words: WordEntry[] = [{ word: 'Hello', start: 0.0, end: 0.3 }]
    expect(findAnnotationWordRangeByContext('Hello world.', 'goodbye', words)).toBeNull()
  })

  it('returns null when neither neighbor can be located in the words', () => {
    const words: WordEntry[] = [{ word: 'Unrelated', start: 0.0, end: 0.3 }]
    const sentence = 'Before Ph.D. after.'
    expect(findAnnotationWordRangeByContext(sentence, 'Ph.D.', words)).toBeNull()
  })
})
