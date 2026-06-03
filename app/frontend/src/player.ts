import type { Segment } from './types'

interface SegmentMsg {
  index: number
  transcript: Segment
  audio: string // base64 f32le PCM @ 24000 Hz
  cached: boolean
  paragraph_end: boolean
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
  // AudioContext clock value when the first buffer of the current play/seek
  // was scheduled. Used to compute elapsed time since last seek.
  firstStartTime = 0

  constructor() {
    this.ctx = new AudioContext({ sampleRate: 24000 })
  }

  get currentTime() { return this.ctx.currentTime }

  enqueue(samples: Float32Array<ArrayBuffer>): void {
    const buf = this.ctx.createBuffer(1, samples.length, 24000)
    buf.copyToChannel(samples, 0)

    const src = this.ctx.createBufferSource()
    src.buffer = buf
    src.connect(this.ctx.destination)

    if (!this.started) {
      this.nextStartTime = this.ctx.currentTime + 0.05
      this.firstStartTime = this.nextStartTime
      this.started = true
    } else {
      this.nextStartTime = Math.max(this.nextStartTime, this.ctx.currentTime + 0.01)
    }

    src.start(this.nextStartTime)
    this.nextStartTime += buf.duration
  }

  resume() { return this.ctx.resume() }
  suspend() { return this.ctx.suspend() }
  get state() { return this.ctx.state }

  reset() {
    this.ctx.close()
    this.ctx = new AudioContext({ sampleRate: 24000 })
    this.ctx.suspend()
    this.nextStartTime = 0
    this.firstStartTime = 0
    this.started = false
  }
}

// ---------------------------------------------------------------------------
// Player
// ---------------------------------------------------------------------------

export class Player {
  private queue: AudioQueue
  private segments: Array<{
    transcript: Segment
    // Audio-relative positions in seconds — stable across seeks.
    // startTime = sum of durations of all preceding segments.
    // endTime   = startTime + this segment's duration.
    startTime: number
    endTime: number
    samples: Float32Array<ArrayBuffer>
    paragraphEnd: boolean
  }> = []
  private segmentEls: HTMLElement[] = []
  private activeIndex = -1
  private rafId = 0
  private container: HTMLElement
  private ws: WebSocket | null = null
  private timeUpdateCbs: Array<(t: number) => void> = []
  private endedCbs: Array<() => void> = []
  private onReadyCb: (() => void) | null = null
  private onErrorCb: ((msg: string) => void) | null = null

  private done = false  // true once the WS sends {done: true}
  // Seconds into the full audio where the current play session started.
  private seekOffset = 0
  // Pre-rendered gray spans supplied by caller; activated in place as audio arrives.
  private pendingSpans: HTMLElement[] = []

  constructor(container: HTMLElement) {
    this.container = container
    this.queue = new AudioQueue()
  }

  // ---------------------------------------------------------------------------
  // Public API
  // ---------------------------------------------------------------------------

  synthesize(text: string, voice?: string, pendingSpans?: HTMLElement[]): void {
    this.reset()
    this.pendingSpans = pendingSpans ?? []
    if (this.pendingSpans.length === 0) {
      this.container.innerHTML = '<div class="loading">Synthesizing…</div>'
    }

    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    this.ws = new WebSocket(`${proto}://${location.host}/ws`)

    this.ws.onopen = () => {
      const msg: Record<string, string> = { text }
      if (voice) msg.voice = voice
      this.ws!.send(JSON.stringify(msg))
    }

    this.ws.onmessage = (ev: MessageEvent) => {
      const msg: ServerMsg = JSON.parse(ev.data)

      if (isError(msg)) { this.onErrorCb?.(msg.error); return }
      if (isDone(msg))  { this.done = true; this.ws?.close(); return }

      if (isSegment(msg)) {
        const samples = decodeF32PCM(msg.audio)
        const duration = samples.length / 24000

        // Audio-relative start = end of previous segment (or 0)
        const prev = this.segments[this.segments.length - 1]
        const startTime = prev ? prev.endTime : 0
        const endTime = startTime + duration

        this.queue.enqueue(samples)
        this.segments.push({ transcript: msg.transcript, startTime, endTime, samples, paragraphEnd: msg.paragraph_end })
        this.renderSegment(msg.transcript, this.segments.length - 1)

        if (this.segments.length === 1) this.onReadyCb?.()
      }
    }

    this.ws.onerror = () => { this.onErrorCb?.('WebSocket error') }
  }

  onReady(cb: () => void): void                    { this.onReadyCb = cb }
  onError(cb: (msg: string) => void): void         { this.onErrorCb = cb }
  onEnded(cb: () => void): void                    { this.endedCbs.push(cb) }
  onTimeUpdate(cb: (t: number) => void): void      { this.timeUpdateCbs.push(cb) }

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

  // Total duration of all segments — stable, never changes with seeks.
  get duration(): number {
    const last = this.segments[this.segments.length - 1]
    return last ? last.endTime : 0
  }

  get hasAudio(): boolean { return this.segments.length > 0 }

  get synthesizedWordCount(): number {
    return this.segments
      .map(s => s.transcript.text.trim().split(/\s+/).filter(Boolean).length)
      .reduce((a, b) => a + b, 0)
  }

  downloadWav(filename: string): void {
    if (!this.hasAudio) return

    // Concatenate all segment samples
    const totalSamples = this.segments.reduce((n, s) => n + s.samples.length, 0)
    const pcm = new Float32Array(totalSamples)
    let offset = 0
    for (const seg of this.segments) {
      pcm.set(seg.samples, offset)
      offset += seg.samples.length
    }

    const wav = encodeWav(pcm, 24000)
    const blob = new Blob([wav], { type: 'audio/wav' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = filename
    a.click()
    URL.revokeObjectURL(url)
  }

  // Current playback position in seconds relative to the full audio.
  // seekOffset anchors us to the right place after a seek.
  get position(): number {
    const elapsed = Math.max(0, this.queue.currentTime - this.queue.firstStartTime)
    return Math.min(this.seekOffset + elapsed, this.duration)
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
    this.pendingSpans = []
    this.activeIndex = -1
    this.seekOffset = 0
    this.done = false
  }

  private renderSegment(transcript: Segment, index: number): void {
    const clickHandler = () => {
      this.stopTracking()
      if (this.activeIndex >= 0) {
        this.segmentEls[this.activeIndex]?.classList.remove('active')
      }
      this.activeIndex = -1
      this.seekOffset = this.segments[index].startTime
      this.queue.reset()
      for (let i = index; i < this.segments.length; i++) {
        this.queue.enqueue(this.segments[i].samples)
      }
      this.highlightSegment(index)
      this.startTracking()
    }

    const pending = this.pendingSpans[index]
    if (pending) {
      // Activate the pre-rendered gray span in place.
      pending.classList.remove('pending')
      pending.addEventListener('click', clickHandler)
      this.segmentEls.push(pending)
      return
    }

    // No pre-rendered span — build and append normally.
    if (index === 0 && this.pendingSpans.length === 0) this.container.innerHTML = ''

    const span = document.createElement('span')
    span.className = 'segment'
    span.textContent = transcript.text
    span.dataset.index = String(index)
    span.addEventListener('click', clickHandler)

    this.container.appendChild(span)
    const seg = this.segments[index]
    if (seg?.paragraphEnd) {
      const br = document.createElement('div')
      br.className = 'paragraph-break'
      this.container.appendChild(br)
    } else {
      this.container.appendChild(document.createTextNode(' '))
    }
    this.segmentEls.push(span)
  }

  private startTracking(): void {
    const lastEndedIndex = { value: -1 }

    const tick = () => {
      const pos = this.position
      this.timeUpdateCbs.forEach(cb => cb(pos))
      this.highlightCurrent()

      // Detect end of playback — only after synthesis is complete
      const last = this.segments[this.segments.length - 1]
      if (this.done && last && this.queue.currentTime >= this.queue.firstStartTime +
          (last.endTime - this.seekOffset) &&
          lastEndedIndex.value < this.segments.length - 1) {
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

  private highlightCurrent(): void {
    // Convert AudioContext clock to audio-relative position for segment lookup
    const pos = this.position
    let found = -1
    for (let i = 0; i < this.segments.length; i++) {
      const s = this.segments[i]
      if (pos >= s.startTime && pos < s.endTime) {
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

/// Encode mono Float32 PCM as a WAV file (IEEE float format).
function encodeWav(samples: Float32Array, sampleRate: number): ArrayBuffer {
  const bytesPerSample = 4 // float32
  const dataSize = samples.length * bytesPerSample
  const buffer = new ArrayBuffer(44 + dataSize)
  const view = new DataView(buffer)

  const write = (offset: number, value: number, size: number) =>
    size === 4 ? view.setUint32(offset, value, true) : view.setUint16(offset, value, true)
  const writeStr = (offset: number, s: string) => {
    for (let i = 0; i < s.length; i++) view.setUint8(offset + i, s.charCodeAt(i))
  }

  writeStr(0,  'RIFF')
  write(4,     36 + dataSize, 4)   // chunk size
  writeStr(8,  'WAVE')
  writeStr(12, 'fmt ')
  write(16,    16, 4)              // subchunk1 size
  write(20,    3, 2)              // audio format: IEEE float
  write(22,    1, 2)              // channels: mono
  write(24,    sampleRate, 4)
  write(28,    sampleRate * bytesPerSample, 4) // byte rate
  write(32,    bytesPerSample, 2) // block align
  write(34,    32, 2)             // bits per sample
  writeStr(36, 'data')
  write(40,    dataSize, 4)

  const pcmView = new Float32Array(buffer, 44)
  pcmView.set(samples)

  return buffer
}
