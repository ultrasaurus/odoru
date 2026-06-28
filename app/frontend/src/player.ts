import type { Segment } from './types'
import * as Ws from './ws'

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
  // All source nodes scheduled since the last reset() — tracked so
  // scheduleStopAt() can cut them off at a precise AudioContext time.
  private sources: AudioBufferSourceNode[] = []
  // Active stop time, if any — applied to every node *already* in
  // `sources` when scheduleStopAt() is called, and to every node enqueued
  // *afterwards* too. Segments keep streaming in live via a separate WS
  // handler while synthesis is in progress (`enqueue()` is called for each
  // as it arrives), so a node that didn't exist yet at scheduleStopAt()
  // time still needs the same cutoff applied when it's created — otherwise
  // it plays through uninterrupted, which only showed up in practice for
  // annotations listened to while synthesis was still catching up.
  private scheduledStopAt: number | null = null

  constructor() {
    this.ctx = new AudioContext({ sampleRate: 24000 })
  }

  get currentTime() { return this.ctx.currentTime }

  async decodeAudioData(data: ArrayBuffer): Promise<Float32Array<ArrayBuffer>> {
    const audioBuffer = await this.ctx.decodeAudioData(data)
    return audioBuffer.getChannelData(0) as Float32Array<ArrayBuffer>
  }

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
    this.sources.push(src)
    if (this.scheduledStopAt !== null) {
      try { src.stop(this.scheduledStopAt) } catch { /* already past */ }
    }
    this.nextStartTime += buf.duration
  }

  /**
   * Schedule every currently-enqueued source node — and any enqueued
   * later, until cleared — to stop at `ctxTime` (an
   * `AudioContext.currentTime`-relative time). Sample-accurate, on the
   * audio hardware clock, not subject to the main thread or
   * requestAnimationFrame being throttled (e.g. when the tab loses focus).
   * A node already past its natural end, or already stopped, is a no-op.
   * A node whose scheduled start is after `ctxTime` simply never plays.
   */
  scheduleStopAt(ctxTime: number): void {
    this.scheduledStopAt = ctxTime
    for (const src of this.sources) {
      try { src.stop(ctxTime) } catch { /* already stopped */ }
    }
  }

  // Stop applying a previously-scheduled cutoff to newly-enqueued nodes —
  // call when the stop has been reached, or when starting unrelated
  // playback that shouldn't inherit a stale listenTo() cutoff.
  clearScheduledStop(): void {
    this.scheduledStopAt = null
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
    this.sources = []
    this.scheduledStopAt = null
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
  private timeUpdateCbs: Array<(t: number) => void> = []
  private endedCbs: Array<() => void> = []
  private onReadyCb: (() => void) | null = null
  private onSynthDoneCb: (() => void) | null = null
  private onErrorCb: ((msg: string) => void) | null = null
  private onWaitingCb: (() => void) | null = null
  private onSeekReadyCb: (() => void) | null = null

  autoScroll = false    // set by caller; scrolls active segment into view when true
  private done = false  // true once the WS sends {done: true}
  private knownDurationSecs: number | null = null
  // Serialises async segment processing so decodes complete in arrival order.
  private decodeChain: Promise<void> = Promise.resolve()
  // Seconds into the full audio where the current play session started.
  private seekOffset = 0
  // Pre-rendered gray spans supplied by caller; activated in place as audio arrives.
  private pendingSpans: HTMLElement[] = []
  // Index to seek to once that segment arrives, or -1 if no pending seek.
  private pendingSeekIndex = -1
  private pendingSeekWasPlaying = false
  private stopAt: number | null = null
  // Incremented by reset(). Captured at the start of each synthesize() call and
  // checked after every decodeAudioData() await. If the user switches documents
  // while a decode is in flight, reset() fires and bumps the generation before
  // the await resolves — the callback detects the mismatch and discards the
  // decoded samples rather than enqueuing them into the new session's AudioQueue.
  // Note: stream_id filtering in ws.ts stops *new* WS frames from reaching
  // onSegment after cancel, but frames that already entered the decodeChain
  // before cancel are still in flight — this guard is what catches those.
  private generation = 0

  constructor(container: HTMLElement) {
    this.container = container
    this.queue = new AudioQueue()
  }

  // ---------------------------------------------------------------------------
  // Public API
  // ---------------------------------------------------------------------------

  synthesize(text: string, voice?: string, pendingSpans?: HTMLElement[], documentId?: string): void {
    this.reset()
    this.pendingSpans = pendingSpans ?? []
    if (this.pendingSpans.length === 0) {
      this.container.innerHTML = '<div class="loading">Synthesizing…</div>'
    }

    const gen = this.generation
    let receivedAny = false
    Ws.sendSynth(text, voice ?? '', documentId, {
      onSegment: (msg) => {
        // Activate pre-rendered pending span immediately as segment arrives,
        // before the decode chain — so cached audio lights up spans fast.
        // Indexed by the segment's own real sentence index, not arrival
        // order — sentences can be missing mid-stream (an imported voice's
        // gap, or even today's existing live-synthesis skip of a blank/
        // symbol-only sentence in engine.rs), so "the Nth one to arrive"
        // and "sentence N" are not the same thing once any gap exists.
        const pending = this.pendingSpans[msg.index]
        if (pending) {
          pending.classList.remove('pending')
          pending.addEventListener('click', () => this.seekTo(msg.index))
        }

        this.decodeChain = this.decodeChain.then(async () => {
          const samples = await this.queue.decodeAudioData(msg.audioData)
          // Guard: reset() increments generation, so a mismatch here means the
          // user switched away while this decode was in flight. Discard the
          // result — enqueuing stale audio into the new session's AudioQueue
          // would corrupt it. (Stream_id filtering in ws.ts already stopped new
          // frames from arriving; this catches frames that entered the chain
          // before cancel fired.)
          if (this.generation !== gen) return
          const duration = samples.length / 24000
          // Safe even with earlier gaps still unresolved: segments are only
          // ever assigned at a strictly increasing index (the server emits
          // indices in order), so segments.length - 1 is always the last
          // *real* entry, never a gap — gaps only ever leave holes at lower
          // indices, never at the tail.
          const prev = this.segments[this.segments.length - 1]
          const startTime = prev ? prev.endTime : 0
          const endTime = startTime + duration

          this.queue.enqueue(samples)
          this.segments[msg.index] = { transcript: msg.transcript, startTime, endTime, samples, paragraphEnd: msg.paragraph_end }

          if (pending) {
            // Span already brightened; just register it for highlight tracking.
            this.segmentEls[msg.index] = pending
          } else {
            this.renderSegment(msg.transcript, msg.index)
          }

          if (!receivedAny) { receivedAny = true; this.onReadyCb?.() }

          if (this.pendingSeekIndex >= 0 && msg.index === this.pendingSeekIndex) {
            this._doSeek(this.pendingSeekIndex, this.pendingSeekWasPlaying)
            this.pendingSeekIndex = -1
            this.pendingSeekWasPlaying = false
            this.onSeekReadyCb?.()
          }
        })
      },
      onDone: async () => {
        // Every segment that actually arrived is only written into
        // `this.segments` once its turn in the serialized decodeChain comes
        // up — and 'done' can arrive over the wire before that chain has
        // caught up to the last segment, especially for a fast cached
        // replay. Awaiting it here first means fillRemainingGaps only ever
        // backfills indices that truly never arrived, not ones still
        // in-flight through decode.
        const chainAtDone = this.decodeChain
        await chainAtDone
        if (this.generation !== gen) return
        this.done = true
        this.fillRemainingGaps()
        if (this.pendingSeekIndex >= 0) {
          // Never resolved during live arrival — either a confirmed gap
          // (now backfilled with a zero-duration placeholder by
          // fillRemainingGaps, so this seeks to the next real audio after
          // it) or some other reason the exact-match check above never
          // fired. Resolve now rather than leaving the seek parked forever.
          this._doSeek(this.pendingSeekIndex, this.pendingSeekWasPlaying)
          this.pendingSeekIndex = -1
          this.pendingSeekWasPlaying = false
          this.onSeekReadyCb?.()
        }
        this.onSynthDoneCb?.()
      },
      onError: (msg) => {
        this.onErrorCb?.(msg)
      },
    })
  }

  setPendingSpans(spans: HTMLElement[]): void       { this.pendingSpans = spans }
  onReady(cb: () => void): void                    { this.onReadyCb = cb }
  /** Fires when all audio has been received (safe to download). */
  onSynthDone(cb: () => void): void                { this.onSynthDoneCb = cb }
  onError(cb: (msg: string) => void): void         { this.onErrorCb = cb }
  onEnded(cb: () => void): void                    { this.endedCbs.push(cb) }
  onTimeUpdate(cb: (t: number) => void): void      { this.timeUpdateCbs.push(cb) }
  /** Fires when seekTo targets a segment not yet received. */
  onWaiting(cb: () => void): void                  { this.onWaitingCb = cb }
  /** Fires when a pending seek completes as the target segment arrives. */
  onSeekReady(cb: () => void): void                { this.onSeekReadyCb = cb }

  seekTo(index: number, autoPlay?: boolean): void {
    const wasPlaying = autoPlay ?? this.queue.state === 'running'
    if (this.segments[index]) {
      this._doSeek(index, wasPlaying)
      this.pendingSeekIndex = -1
    } else {
      // Not yet received — park and wait. If this index turns out to be a
      // confirmed gap, onDone's fillRemainingGaps()/resolution step will
      // pick this up rather than leaving it parked forever.
      this.pendingSeekIndex = index
      this.pendingSeekWasPlaying = wasPlaying
      this.onWaitingCb?.()
    }
  }

  /**
   * `trimStartSecs` skips that many seconds off the front of `index`'s own
   * samples before enqueuing (used by `listenTo` to start mid-segment, e.g.
   * at the start of an annotated phrase rather than the sentence start).
   * Later segments are unaffected.
   */
  private _doSeek(index: number, wasPlaying: boolean, trimStartSecs = 0): void {
    this.stopTracking()
    if (this.activeIndex >= 0) {
      this.segmentEls[this.activeIndex]?.classList.remove('active')
    }
    this.activeIndex = -1
    const target = this.segments[index]
    if (!target) return  // shouldn't happen — callers already check
    this.seekOffset = target.startTime + trimStartSecs
    this.queue.reset()
    for (let i = index; i < this.segments.length; i++) {
      const seg = this.segments[i]
      // A hole (gap not yet backfilled, mid-stream) or a backfilled
      // zero-duration gap placeholder — nothing to enqueue, continue to
      // whatever comes after it.
      if (!seg || seg.samples.length === 0) continue
      let samples = seg.samples
      if (i === index && trimStartSecs > 0) {
        const skipSamples = Math.round(trimStartSecs * 24000)
        samples = samples.subarray(Math.min(skipSamples, samples.length))
      }
      this.queue.enqueue(samples)
    }
    this.highlightSegment(index)
    if (wasPlaying) {
      this.play()
    } else {
      this.startTracking()
    }
  }

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
    // Prefer the caller-supplied total (e.g. voices[voice].duration from the
    // document, computed once when that voice finished synthesizing) over
    // the segment-derived value — the latter can read low if `onDone` fires
    // before the (separately serialized) decodeChain has caught up to every
    // segment that already arrived, see setKnownDuration.
    if (this.knownDurationSecs !== null) return this.knownDurationSecs
    const last = this.segments[this.segments.length - 1]
    return last ? last.endTime : 0
  }

  /**
   * Supplies an authoritative total duration (seconds), bypassing the
   * segment-derived calculation entirely for `duration`/progress-bar/seek
   * math. Call this whenever the true total is already known up front
   * (e.g. from `voices[voice].duration`) rather than relying on it being
   * derived correctly from in-flight segment arrival.
   */
  setKnownDuration(secs: number | null): void { this.knownDurationSecs = secs }

  get hasAudio(): boolean { return this.segments.length > 0 }

  /** Start time (seconds) of the segment at `index`, or null if not yet received. */
  segmentStartTime(index: number): number | null {
    return this.segments[index]?.startTime ?? null
  }

  /** Index of a segment DOM element in the internal array, or -1 if not found. */
  segmentIndexForEl(el: HTMLElement): number {
    return this.segmentEls.indexOf(el)
  }

  /**
   * Seek into `segIndex` — at `startOffsetSecs` seconds in (default 0, i.e.
   * the segment's own start) — and play, stopping automatically when
   * `endOffsetSecs` seconds into `endSegIndex` (default `segIndex`, i.e.
   * the same segment) have elapsed. `endSegIndex` can be a later segment,
   * for an annotation that spans multiple sentences.
   * No-op if audio is not loaded or either segment is out of range.
   *
   * The actual audio cutoff is scheduled natively on the Web Audio nodes
   * (`AudioQueue.scheduleStopAt`) rather than relying solely on the
   * `tick()` polling loop below — `requestAnimationFrame` can be throttled
   * by the browser (e.g. when the tab loses focus/visibility), which let
   * playback audibly run on past `stopAt` by a variable amount. The native
   * schedule is sample-accurate and unaffected by that. `tick()` still
   * handles the surrounding state sync (highlighting, calling `pause()` to
   * update `queue.state`) — if that lags, the audio is already silent by
   * then regardless.
   */
  listenTo(segIndex: number, endOffsetSecs: number, startOffsetSecs = 0, endSegIndex = segIndex): void {
    const endSeg = this.segments[endSegIndex]
    if (!this.hasAudio || !this.segments[segIndex] || !endSeg) return
    this.stopAt = endSeg.startTime + endOffsetSecs
    this._doSeek(segIndex, true, startOffsetSecs)
    const ctxStopTime = this.queue.firstStartTime + (this.stopAt - this.seekOffset)
    this.queue.scheduleStopAt(ctxStopTime)
  }

  get synthesizedWordCount(): number {
    return this.segments
      .map(s => s.transcript.text.trim().split(/\s+/).filter(Boolean).length)
      .reduce((a, b) => a + b, 0)
  }

  stop(): void { this.reset() }

  downloadWav(filename: string): void {
    if (!this.hasAudio) return

    // Concatenate all segment samples. `seg` can be a hole if this is
    // somehow called before synthesis finishes (gaps aren't backfilled
    // with zero-length placeholders until onDone) — skip rather than crash.
    const totalSamples = this.segments.reduce((n, s) => n + (s?.samples.length ?? 0), 0)
    const pcm = new Float32Array(totalSamples)
    let offset = 0
    for (const seg of this.segments) {
      if (!seg) continue
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
    ++this.generation
    Ws.cancelSynth()
    this.stopTracking()
    this.queue.reset()
    this.segments = []
    this.segmentEls = []
    this.pendingSpans = []
    this.activeIndex = -1
    this.seekOffset = 0
    this.done = false
    this.decodeChain = Promise.resolve()
    this.pendingSeekIndex = -1
    this.pendingSeekWasPlaying = false
    this.stopAt = null
    this.knownDurationSecs = null
  }

  private renderSegment(transcript: Segment, index: number): void {
    const clickHandler = () => { this.seekTo(index) }

    const pending = this.pendingSpans[index]
    if (pending) {
      // Activate the pre-rendered gray span in place.
      pending.classList.remove('pending')
      pending.addEventListener('click', clickHandler)
      this.segmentEls[index] = pending
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
    this.segmentEls[index] = span
  }

  /**
   * Called once on `done` (no more arrivals expected this session). Any
   * index up to the known total sentence count that's still a hole is a
   * confirmed gap — no cached audio for an imported voice, or (pre-existing,
   * unrelated to import) a blank/symbol-only sentence engine.rs skips
   * entirely. Backfill each with a zero-duration placeholder so every
   * downstream consumer (`_doSeek`, `highlightCurrent`, `downloadWav`, …)
   * can keep treating `segments` as dense rather than needing gap-awareness
   * of its own. `_doSeek` skips zero-length samples, so seeking to a gap
   * just continues to whatever real audio comes after it.
   *
   * `pendingSpans.length` is the total — known up front from the rendered
   * markdown, independent of audio arrival. If it's empty (the fresh,
   * unrendered-text synthesis path, which never reaches imported voices in
   * practice), there's no a-priori total to backfill against; leave as is.
   */
  private fillRemainingGaps(): void {
    const expectedTotal = this.pendingSpans.length
    if (expectedTotal === 0) return
    for (let i = 0; i < expectedTotal; i++) {
      if (this.segments[i]) continue
      const prev = this.segments[i - 1]
      const t = prev ? prev.endTime : 0
      this.segments[i] = {
        transcript: { text: this.pendingSpans[i]?.textContent ?? '', start: t, end: t, words: [] },
        startTime: t,
        endTime: t,
        samples: new Float32Array(0),
        paragraphEnd: false,
      }
      if (!this.segmentEls[i]) this.segmentEls[i] = this.pendingSpans[i]
    }
  }

  private startTracking(): void {
    this.stopTracking()
    const lastEndedIndex = { value: -1 }

    const tick = () => {
      const pos = this.position
      this.timeUpdateCbs.forEach(cb => cb(pos))
      this.highlightCurrent()

      if (this.stopAt !== null && pos >= this.stopAt) {
        this.stopAt = null
        this.queue.clearScheduledStop()  // don't let later, unrelated segments inherit this cutoff
        this.pause()
        return
      }

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
      // A hole at a lower index whose gap hasn't been backfilled yet
      // (mid-stream, before onDone) — never matches, keep scanning.
      if (!s) continue
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
      if (this.autoScroll) el?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
    }
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
