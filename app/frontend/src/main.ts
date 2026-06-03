import './style.css'
import { Player } from './player'

interface VoiceInfo {
  name: string
  description: string
}

interface VoicesResponse {
  backend: string
  voices: VoiceInfo[]
}

// Approximate generation seconds per word for each backend.
// Kokoro: ~0.2 sec/word (measured: 143 words in 26s)
// F5:     ~3.0 sec/word (measured: 143 words in 410s)
const SECS_PER_WORD: Record<string, number> = {
  kokoro: 0.2,
  f5: 3.0,
}

// ── State ─────────────────────────────────────────────────────────────────

let voices: VoiceInfo[] = []
let selectedVoice: string | null = null
let activeBackend: string = 'kokoro'

// ── DOM ───────────────────────────────────────────────────────────────────

const app = document.getElementById('app')!

app.innerHTML = `
  <div class="layout">
    <header class="header">
      <div class="logo">▶ odoru</div>
    </header>

    <main class="main">
      <div class="workspace">
        <div class="card">
          <div class="url-area">
            <input
              id="url-input"
              class="url-input"
              type="url"
              placeholder="Paste a URL and press Enter…"
            />
            <div id="fetch-status" class="fetch-status"></div>
          </div>

          <div class="input-area">
            <textarea
              id="text-input"
              class="text-input"
              placeholder="…or paste text here directly, then press Synthesize"
              rows="4"
            ></textarea>
            <div class="synth-row">
              <div id="time-estimate" class="time-estimate"></div>
              <button id="synth-btn" class="synth-btn">Synthesize</button>
            </div>
          </div>

          <div id="transcript-container" class="transcript-container">
            <div class="placeholder">Fetch a URL or enter text above, then press Synthesize.</div>
          </div>

          <div class="controls">
            <button id="play-btn" class="play-btn" disabled>
              <span class="play-icon">▶</span>
            </button>
            <div class="progress-wrap">
              <div class="progress-bar">
                <div id="progress-fill" class="progress-fill"></div>
              </div>
              <div class="time-row">
                <span id="time-current" class="time">0:00</span>
                <span id="time-total" class="time">0:00</span>
              </div>
            </div>
            <button id="download-btn" class="download-btn" disabled title="Download WAV">↓</button>
          </div>
        </div>

        <aside class="sidebar">
          <div class="sidebar-section">
            <div class="sidebar-label">Voice</div>
            <div id="voice-list" class="voice-list">
              <div class="voice-loading">Loading voices…</div>
            </div>
            <div id="voice-description" class="voice-description"></div>
          </div>
        </aside>
      </div>
    </main>
  </div>
`

const synthBtn    = document.getElementById('synth-btn')    as HTMLButtonElement
const textInput   = document.getElementById('text-input')   as HTMLTextAreaElement
const timeEstimate = document.getElementById('time-estimate') as HTMLDivElement
const urlInput    = document.getElementById('url-input')    as HTMLInputElement
const fetchStatus = document.getElementById('fetch-status') as HTMLDivElement
const playBtn     = document.getElementById('play-btn')     as HTMLButtonElement
const playIcon    = playBtn.querySelector('.play-icon')     as HTMLSpanElement
const downloadBtn = document.getElementById('download-btn') as HTMLButtonElement
const progressFill     = document.getElementById('progress-fill')     as HTMLDivElement
const timeCurrent      = document.getElementById('time-current')      as HTMLSpanElement
const timeTotal        = document.getElementById('time-total')        as HTMLSpanElement
const transcriptContainer = document.getElementById('transcript-container') as HTMLDivElement
const voiceList        = document.getElementById('voice-list')        as HTMLDivElement
const voiceDescription = document.getElementById('voice-description') as HTMLDivElement

// ── Voice picker ───────────────────────────────────────────────────────────

function renderVoices() {
  if (voices.length === 0) {
    voiceList.innerHTML = '<div class="voice-loading">No voices available.</div>'
    return
  }

  voiceList.innerHTML = ''
  for (const v of voices) {
    const row = document.createElement('button')
    row.className = 'voice-row' + (v.name === selectedVoice ? ' selected' : '')
    row.textContent = v.name
    row.addEventListener('click', () => selectVoice(v.name))
    voiceList.appendChild(row)
  }
}

function selectVoice(name: string) {
  selectedVoice = name
  const v = voices.find(v => v.name === name)
  voiceDescription.textContent = v?.description ?? ''
  renderVoices()
}

async function loadVoices() {
  try {
    const res = await fetch('/voices')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data: VoicesResponse = await res.json()
    voices = data.voices
    activeBackend = data.backend
    if (voices.length > 0 && !selectedVoice) {
      selectVoice(voices[0].name)
    } else {
      renderVoices()
    }
    // Refresh estimate now that we know the backend
    updateEstimate(textInput.value)
  } catch (err) {
    voiceList.innerHTML = '<div class="voice-loading error">Failed to load voices.</div>'
  }
}

loadVoices()

// ── Time estimate ─────────────────────────────────────────────────────────

function fmtDuration(secs: number): string {
  if (secs < 60) return `~${Math.round(secs)}s`
  const m = Math.floor(secs / 60)
  const s = Math.round(secs % 60)
  return s > 0 ? `~${m}m ${s}s` : `~${m}m`
}

function updateEstimate(text: string) {
  const words = text.trim().split(/\s+/).filter(Boolean).length
  if (words === 0) { timeEstimate.textContent = ''; return }
  const rate = SECS_PER_WORD[activeBackend] ?? 0.2
  const secs = words * rate
  timeEstimate.textContent = `${fmtDuration(secs)} to synthesize (${words} words)`
}

// ── Player ─────────────────────────────────────────────────────────────────

function downloadFilename(): string {
  const url = urlInput.value.trim()
  if (!url) return 'odoru.wav'
  try {
    const u = new URL(url)
    const slug = (u.hostname + u.pathname)
      .replace(/[^a-z0-9]+/gi, '-')
      .replace(/^-+|-+$/g, '')
      .toLowerCase()
    return `${slug}.wav`
  } catch {
    return 'odoru.wav'
  }
}

function fmt(s: number): string {
  const m = Math.floor(s / 60)
  const sec = Math.floor(s % 60)
  return `${m}:${sec.toString().padStart(2, '0')}`
}

const player = new Player(transcriptContainer)

player.onReady(() => {
  playBtn.disabled = false
  playIcon.textContent = '▶'
  player.play()
  playIcon.textContent = '⏸'
})

player.onError(msg => {
  transcriptContainer.innerHTML = `<div class="error">Error: ${msg}</div>`
  synthBtn.disabled = false
  playBtn.disabled = true
})

player.onTimeUpdate(t => {
  timeCurrent.textContent = fmt(t)
  const dur = player.duration
  const pct = dur > 0 ? (t / dur) * 100 : 0
  progressFill.style.width = `${Math.min(pct, 100)}%`
  timeTotal.textContent = fmt(dur)
})

let synthStart = 0

player.onEnded(() => {
  playIcon.textContent = '▶'
  progressFill.style.width = '100%'
  synthBtn.disabled = false
  downloadBtn.disabled = false
  if (synthStart > 0) {
    const elapsed = ((Date.now() - synthStart) / 1000).toFixed(0)
    const words = player.synthesizedWordCount
    timeEstimate.textContent = `Synthesized ${words} words in ${elapsed}s`
    synthStart = 0
  }
})

synthBtn.addEventListener('click', () => {
  const text = textInput.value.trim()
  if (!text) return

  synthBtn.disabled = true
  playBtn.disabled = true
  downloadBtn.disabled = true
  playIcon.textContent = '▶'
  progressFill.style.width = '0%'
  timeCurrent.textContent = '0:00'
  timeTotal.textContent = '0:00'
  synthStart = Date.now()

  player.synthesize(text, selectedVoice ?? undefined)
})

playBtn.addEventListener('click', () => {
  player.toggle()
  playIcon.textContent = player.paused ? '▶' : '⏸'
})

downloadBtn.addEventListener('click', () => {
  player.downloadWav(downloadFilename())
})

textInput.addEventListener('input', () => updateEstimate(textInput.value))

textInput.addEventListener('keydown', (e: KeyboardEvent) => {
  if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
    synthBtn.click()
  }
})

urlInput.addEventListener('keydown', async (e: KeyboardEvent) => {
  if (e.key !== 'Enter') return
  const url = urlInput.value.trim()
  if (!url) return

  fetchStatus.textContent = 'Fetching…'
  fetchStatus.className = 'fetch-status loading'
  urlInput.disabled = true

  try {
    const res = await fetch(`/doc?url=${encodeURIComponent(url)}`)
    const data = await res.json()

    if (!res.ok) {
      fetchStatus.textContent = data.error ?? 'Fetch failed'
      fetchStatus.className = 'fetch-status error'
      return
    }

    textInput.value = data.plain_text
    updateEstimate(data.plain_text)
    const cached = data.cached ? ' (cached)' : ''
    const title = data.title ?? url
    fetchStatus.textContent = `✔ ${title}${cached}`
    fetchStatus.className = 'fetch-status success'
  } catch (err) {
    fetchStatus.textContent = 'Network error'
    fetchStatus.className = 'fetch-status error'
  } finally {
    urlInput.disabled = false
    urlInput.focus()
  }
})
