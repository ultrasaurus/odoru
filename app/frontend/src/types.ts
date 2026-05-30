export interface Segment {
  start: number
  end: number
  text: string
  words: Word[]
}

export interface Word {
  word: string
  start?: number
  end?: number
  score?: number
}
