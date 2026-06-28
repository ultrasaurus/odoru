import type { Player } from './player'
import type { VoiceEntry } from './document'

// ── Shared types ──────────────────────────────────────────────────────────────

export interface VoiceInfo {
  id: string
  name: string
  backend: string
  description: string
}

export interface VoicesResponse {
  voices: VoiceInfo[]
}

export interface JobInfo {
  id: string
  voice: string
  text_preview: string
  document_id?: string
  status: string
  total_sentences: number
  completed_sentences: number
  created_at: string
  error?: string
}

// Approximate generation seconds per word for each backend.
export const SECS_PER_WORD: Record<string, number> = {
  kokoro: 0.2,
  f5: 3.0,
}

// Pick the best voice from a document's voices map.
// Priority: published → first ready → first stale → first any.
export function pickVoice(voices: Record<string, VoiceEntry>): string | null {
  for (const [id, v] of Object.entries(voices)) {
    if (v.published) return id
  }
  for (const [id, v] of Object.entries(voices)) {
    if (v.status === 'ready') return id
  }
  for (const [id, v] of Object.entries(voices)) {
    if (v.status === 'stale') return id
  }
  const keys = Object.keys(voices)
  return keys.length > 0 ? keys[0] : null
}

// ── DOM helpers ───────────────────────────────────────────────────────────────

export function makeEl(tag: string, className: string, text: string): HTMLElement {
  const el = document.createElement(tag)
  el.className = className
  el.textContent = text
  return el
}

export function setError(container: HTMLElement, msg: string): void {
  container.innerHTML = ''
  container.appendChild(makeEl('div', 'error', msg))
}

export function setStatus(container: HTMLElement, className: string, msg: string): void {
  container.innerHTML = ''
  container.appendChild(makeEl('span', className, msg))
}

export function fmt(s: number): string {
  const m = Math.floor(s / 60)
  const sec = Math.floor(s % 60)
  return `${m}:${sec.toString().padStart(2, '0')}`
}

export function wireControls(
  player: Player,
  playBtn: HTMLButtonElement,
  downloadBtn: HTMLButtonElement,
  progressFill: HTMLDivElement,
  timeCurrent: HTMLSpanElement,
  timeTotal: HTMLSpanElement,
  getFilename: () => string,
): void {
  const playIcon = playBtn.querySelector('.play-icon') as HTMLSpanElement

  player.onReady(() => {
    playBtn.disabled = false
  })

  player.onSynthDone(() => {
    downloadBtn.disabled = false
    // duration is exact as soon as synthesis/replay finishes (gaps already
    // backfilled), but timeTotal otherwise only updates from the playback
    // tick loop, which doesn't run until the user presses play.
    timeTotal.textContent = fmt(player.duration)
  })

  player.onTimeUpdate(t => {
    timeCurrent.textContent = fmt(t)
    const dur = player.duration
    const pct = dur > 0 ? (t / dur) * 100 : 0
    progressFill.style.width = `${Math.min(pct, 100)}%`
    timeTotal.textContent = fmt(dur)
    playIcon.textContent = player.paused ? '▶' : '⏸'
  })

  player.onEnded(() => {
    playIcon.textContent = '▶'
    progressFill.style.width = '100%'
  })

  playBtn.addEventListener('click', async () => {
    await player.toggle()
    playIcon.textContent = player.paused ? '▶' : '⏸'
  })

  downloadBtn.addEventListener('click', () => {
    player.downloadWav(getFilename())
  })
}

export function controlsHtml(): string {
  return `
    <div class="controls">
      <div id="voice-label" class="voice-label"></div>
      <div class="controls-row">
        <div id="player-controls" class="player-controls" style="display:none">
          <div class="player-row">
            <button id="play-btn" class="play-btn" disabled>
              <span class="play-icon">▶</span>
            </button>
            <div class="progress-bar">
              <div id="progress-fill" class="progress-fill"></div>
            </div>
            <button id="download-btn" class="download-btn" disabled title="Download WAV">↓</button>
          </div>
          <div class="time-row">
            <span id="time-current" class="time">0:00</span>
            <span id="time-total" class="time">0:00</span>
          </div>
          <div id="seek-status" class="seek-status" style="display:none">Waiting for audio to arrive…</div>
        </div>
        <div class="synth-buttons">
          <button id="synth-btn" class="synth-btn" style="display:none">Synthesize</button>
        </div>
      </div>
      <div id="time-estimate" class="time-estimate"></div>
    </div>
  `
}

export function grabControlEls() {
  return {
    playBtn:      document.getElementById('play-btn')      as HTMLButtonElement,
    downloadBtn:  document.getElementById('download-btn')  as HTMLButtonElement,
    progressFill: document.getElementById('progress-fill') as HTMLDivElement,
    timeCurrent:  document.getElementById('time-current')  as HTMLSpanElement,
    timeTotal:    document.getElementById('time-total')    as HTMLSpanElement,
  }
}
