import type { Segment } from './types'

interface SegmentMsg {
  index: number
  transcript: Segment
  audio: string // base64 f32le PCM @ 24000 Hz
}

interface DoneMsg {
  done: boolean
}

interface ErrorMsg {
  error: string
}

type ServerMsg = SegmentMsg | DoneMsg | ErrorMsg

function isSegment(m: ServerMsg): m is SegmentMsg { return 'audio' in m }
function isDone(m: ServerMsg): m is DoneMsg       { return 'done' in m }
function isError(m: ServerMsg): m is ErrorMsg     { return 'error' in m }

// ---------------------------------------------------------------------------
// AudioQueue — chains AudioBufferSourceNodes for gapless playback
// ---------------------------------------------------------------------------

class AudioQueue {
  private ctx: AudioContext
  private nextStartTime = 0
  private started = false

  constructor() {
    this.ctx = new AudioContext({ sampleRate: 24000 })
  }

  get currentTime() { return this.ctx.currentTime }

  enqueue(samples: Float32Array<ArrayBuffer>): { startTime: number; endTime: number } {
    const buf = this.ctx.createBuffer(1, samples.length, 24000)
    buf.copyToChannel(samples, 0)

    const src = this.ctx.createBufferSource()
    src.buffer = buf
    src.connect(this.ctx.destination)

    if (!this.started) {
      // Small initial buffer to avoid glitches
      this.nextStartTime = this.ctx.currentTime + 0.05
      this.started = true
    }

    const startTime = this.nextStartTime
    src.start(startTime)
    this.nextStartTime += buf.duration

    return { startTime, endTime: this.nextStartTime }
  }

  resume() { return this.ctx.resume() }
  suspend() { return this.ctx.suspend() }
  get state() { return this.ctx.state }

  reset() {
    this.ctx.close()
    this.ctx = new AudioContext({ sampleRate: 24000 })
    this.nextStartTime = 0
    this.started = false
  }
}

// ---------------------------------------------------------------------------
// Player
// ---------------------------------------------------------------------------

export class Player {
  private queue: AudioQueue
  private segments: Array<{ transcript: Segment; startTime: number; endTime: number }> = []
  private segmentEls: HTMLElement[] = []
  private activeIndex = -1
  private rafId = 0
  private container: HTMLElement
  private ws: WebSocket | null = null
  private timeUpdateCbs: Array<(t: number) => void> = []
  private endedCbs: Array<() => void> = []
  private onReadyCb: (() => void) | null = null
  private onErrorCb: ((msg: string) => void) | null = null

  constructor(container: HTMLElement) {
    this.container = container
    this.queue = new AudioQueue()
  }

  // ---------------------------------------------------------------------------
  // Public API
  // ---------------------------------------------------------------------------

  synthesize(text: string): void {
    this.reset()
    this.container.innerHTML = '<div class="loading">Synthesizing…</div>'

    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    this.ws = new WebSocket(`${proto}://${location.host}/ws`)

    this.ws.onopen = () => {
      this.ws!.send(JSON.stringify({ text }))
    }

    this.ws.onmessage = async (ev: MessageEvent) => {
      const msg: ServerMsg = JSON.parse(ev.data)

      if (isError(msg)) {
        this.onErrorCb?.(msg.error)
        return
      }

      if (isDone(msg)) {
        this.ws?.close()
        return
      }

      if (isSegment(msg)) {
        const samples = decodeF32PCM(msg.audio)
        const { startTime, endTime } = this.queue.enqueue(samples)

        this.segments.push({ transcript: msg.transcript, startTime, endTime })
        this.renderSegment(msg.transcript, this.segments.length - 1)

        // Signal ready on first segment
        if (this.segments.length === 1) {
          this.onReadyCb?.()
        }
      }
    }

    this.ws.onerror = () => {
      this.onErrorCb?.('WebSocket error')
    }
  }

  onReady(cb: () => void): void   { this.onReadyCb = cb }
  onError(cb: (msg: string) => void): void { this.onErrorCb = cb }
  onEnded(cb: () => void): void   { this.endedCbs.push(cb) }
  onTimeUpdate(cb: (t: number) => void): void { this.timeUpdateCbs.push(cb) }

  async play(): Promise<void> {
    await this.queue.resume()
    this.startTracking()
  }

  async pause(): Promise<void> {
    await this.queue.suspend()
    this.stopTracking()
  }

  async toggle(): Promise<void> {
    if (this.queue.state === 'suspended') {
      await this.play()
    } else {
      await this.pause()
    }
  }

  get paused(): boolean { return this.queue.state !== 'running' }

  // Total duration based on queued segments so far
  get duration(): number {
    const last = this.segments[this.segments.length - 1]
    return last ? last.endTime - (this.segments[0]?.startTime ?? 0) : 0
  }

  // ---------------------------------------------------------------------------
  // Private
  // ---------------------------------------------------------------------------

  private reset(): void {
    this.ws?.close()
    this.stopTracking()
    this.queue.reset()
    this.segments = []
    this.segmentEls = []
    this.activeIndex = -1
  }

  private renderSegment(transcript: Segment, index: number): void {
    // Remove loading placeholder on first segment
    if (index === 0) this.container.innerHTML = ''

    const span = document.createElement('span')
    span.className = 'segment'
    span.textContent = transcript.text
    span.dataset.index = String(index)

    // Click to seek: resume from this segment's scheduled start time
    span.addEventListener('click', async () => {
      // AudioContext doesn't support seeking, so we reset and re-enqueue
      // from this segment onward — simple and reliable for short texts
      const fromIndex = index
      this.queue.reset()
      this.activeIndex = -1

      for (let i = fromIndex; i < this.segments.length; i++) {
        // We don't have samples cached — for now just highlight and play from start
        // TODO: cache samples per segment to enable mid-stream seeking
      }

      // For now: clicking a segment highlights it and plays from beginning
      // Full seeking requires caching Float32Array per segment (Phase 2)
      this.highlightSegment(fromIndex)
      await this.play()
    })

    this.container.appendChild(span)
    if (index < this.segments.length - 1 || transcript.text.endsWith('.')) {
      this.container.appendChild(document.createTextNode(' '))
    }
    this.segmentEls.push(span)
  }

  private startTracking(): void {
    const lastEndedIndex = { value: -1 }

    const tick = () => {
      const t = this.queue.currentTime
      this.timeUpdateCbs.forEach(cb => cb(t))
      this.highlightCurrent(t)

      // Check if all segments have finished playing
      const last = this.segments[this.segments.length - 1]
      if (last && t >= last.endTime && lastEndedIndex.value < this.segments.length - 1) {
        lastEndedIndex.value = this.segments.length - 1
        this.stopTracking()
        this.endedCbs.forEach(cb => cb())
        return
      }

      this.rafId = requestAnimationFrame(tick)
    }
    this.rafId = requestAnimationFrame(tick)
  }

  private stopTracking(): void {
    cancelAnimationFrame(this.rafId)
  }

  private highlightCurrent(t: number): void {
    let found = -1
    for (let i = 0; i < this.segments.length; i++) {
      const s = this.segments[i]
      if (t >= s.startTime && t < s.endTime) {
        found = i
        break
      }
    }
    this.highlightSegment(found)
  }

  private highlightSegment(index: number): void {
    if (index === this.activeIndex) return
    if (this.activeIndex >= 0) {
      this.segmentEls[this.activeIndex]?.classList.remove('active')
    }
    this.activeIndex = index
    if (index >= 0) {
      const el = this.segmentEls[index]
      el?.classList.add('active')
      el?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
    }
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function decodeF32PCM(b64: string): Float32Array<ArrayBuffer> {
  const binary = atob(b64)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
  return new Float32Array(bytes.buffer) as Float32Array<ArrayBuffer>
}
