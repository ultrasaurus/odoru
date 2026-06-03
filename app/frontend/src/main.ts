import './style.css'
import { Player } from './player'

interface VoiceInfo {
  id: string          // prefixed, e.g. "f5:sarah" or "kokoro:am_puck"
  name: string        // display name, e.g. "sarah"
  backend: string     // "f5" or "kokoro"
  description: string
}

interface VoicesResponse {
  voices: VoiceInfo[]
}

// Approximate generation seconds per word for each backend.
// Kokoro: ~0.2 sec/word (measured: 143 words in 26s)
// F5:     ~3.0 sec/word (measured: 143 words in 410s)
const SECS_PER_WORD: Record<string, number> = {
  kokoro: 0.2,
  f5: 3.0,
}

const ARTICLE_URL   = 'https://www.dougengelbart.org/content/view/148'
const ARTICLE_VOICE = 'f5:sarah'

const ARTICLES = [
  { title: 'Authorship Provisions in Augment', url: ARTICLE_URL, live: true },
  { title: 'As We May Think' },
  { title: 'A File Structure for the Complex, the Changing, and the Indeterminate' },
  { title: 'Augmenting Human Intellect' },
  { title: 'Intermedia: The Architecture and Construction of an Object-Oriented Hypermedia System and Applications Framework' },
  { title: "Hypertext '87 Keynote Address" },
  { title: 'Hypertext: An Introduction and Survey' },
]

const app = document.getElementById('app')!

// ── Shared helpers ────────────────────────────────────────────────────────────

function fmt(s: number): string {
  const m = Math.floor(s / 60)
  const sec = Math.floor(s % 60)
  return `${m}:${sec.toString().padStart(2, '0')}`
}

function wireControls(
  player: Player,
  playBtn: HTMLButtonElement,
  downloadBtn: HTMLButtonElement,
  progressFill: HTMLDivElement,
  timeCurrent: HTMLSpanElement,
  timeTotal: HTMLSpanElement,
  filename: string,
) {
  const playIcon = playBtn.querySelector('.play-icon') as HTMLSpanElement

  player.onReady(() => {
    playBtn.disabled = false
  })

  player.onTimeUpdate(t => {
    timeCurrent.textContent = fmt(t)
    const dur = player.duration
    const pct = dur > 0 ? (t / dur) * 100 : 0
    progressFill.style.width = `${Math.min(pct, 100)}%`
    timeTotal.textContent = fmt(dur)
  })

  player.onEnded(() => {
    playIcon.textContent = '▶'
    progressFill.style.width = '100%'
    downloadBtn.disabled = false
  })

  playBtn.addEventListener('click', () => {
    player.toggle()
    playIcon.textContent = player.paused ? '▶' : '⏸'
  })

  downloadBtn.addEventListener('click', () => {
    player.downloadWav(filename)
  })
}

function controlsHtml(): string {
  return `
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
  `
}

function grabControlEls() {
  return {
    playBtn:      document.getElementById('play-btn')      as HTMLButtonElement,
    downloadBtn:  document.getElementById('download-btn')  as HTMLButtonElement,
    progressFill: document.getElementById('progress-fill') as HTMLDivElement,
    timeCurrent:  document.getElementById('time-current')  as HTMLSpanElement,
    timeTotal:    document.getElementById('time-total')    as HTMLSpanElement,
  }
}

// ── Reader view ───────────────────────────────────────────────────────────────

function showReader() {
  const listHtml = ARTICLES.map((a, i) => `
    <div class="article-item${i === 0 ? ' selected' : ''}${a.live ? '' : ' disabled'}" data-index="${i}">
      ${a.title}
    </div>
  `).join('')

  app.innerHTML = `
    <div class="reader-layout">
      <nav class="article-sidebar">
        <div class="sidebar-top">
          <button class="new-btn" id="new-btn">New</button>
        </div>
        <div class="article-list">${listHtml}</div>
      </nav>
      <div class="reader-main">
        <div class="reader-header">
          <h1 class="article-title">Authorship Provisions in Augment</h1>
        </div>
        <div id="transcript-container" class="transcript-container">
          <div class="loading">Loading…</div>
        </div>
        ${controlsHtml()}
      </div>
    </div>
  `

  document.getElementById('new-btn')!.addEventListener('click', showNew)

  const transcriptContainer = document.getElementById('transcript-container')!
  const { playBtn, downloadBtn, progressFill, timeCurrent, timeTotal } = grabControlEls()

  const player = new Player(transcriptContainer)

  player.onError(msg => {
    transcriptContainer.innerHTML = `<div class="error">Error: ${msg}</div>`
    playBtn.disabled = true
  })

  wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal,
    'authorship-provisions-in-augment.wav')

  fetch(`/doc?url=${encodeURIComponent(ARTICLE_URL)}&voice=${encodeURIComponent(ARTICLE_VOICE)}`)
    .then(res => res.json())
    .then(data => {
      const audioReady = !!data.cached?.audio
      transcriptContainer.innerHTML = audioReady
        ? '<div class="loading">Ready — press play</div>'
        : '<div class="loading">Synthesizing…</div>'
      player.synthesize(data.plain_text, ARTICLE_VOICE)
    })
    .catch(() => {
      transcriptContainer.innerHTML = '<div class="error">Failed to load article.</div>'
    })
}

// ── New view ──────────────────────────────────────────────────────────────────

function showNew() {
  let voices: VoiceInfo[] = []
  let selectedVoice: string | null = null  // stores prefixed id, e.g. "f5:sarah"
  let synthStart = 0

  app.innerHTML = `
    <div class="layout">
      <header class="header">
        <a class="back-link" id="back-link">← Articles</a>
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

            ${controlsHtml()}
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

  document.getElementById('back-link')!.addEventListener('click', showReader)

  const synthBtn    = document.getElementById('synth-btn')    as HTMLButtonElement
  const textInput   = document.getElementById('text-input')   as HTMLTextAreaElement
  const timeEstimate = document.getElementById('time-estimate') as HTMLDivElement
  const urlInput    = document.getElementById('url-input')    as HTMLInputElement
  const fetchStatus = document.getElementById('fetch-status') as HTMLDivElement
  const voiceList        = document.getElementById('voice-list')        as HTMLDivElement
  const voiceDescription = document.getElementById('voice-description') as HTMLDivElement
  const transcriptContainer = document.getElementById('transcript-container')!
  const { playBtn, downloadBtn, progressFill, timeCurrent, timeTotal } = grabControlEls()

  const player = new Player(transcriptContainer)

  player.onError(msg => {
    transcriptContainer.innerHTML = `<div class="error">Error: ${msg}</div>`
    synthBtn.disabled = false
    playBtn.disabled = true
  })

  wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal,
    downloadFilename())

  player.onEnded(() => {
    synthBtn.disabled = false
    if (synthStart > 0) {
      const elapsed = ((Date.now() - synthStart) / 1000).toFixed(0)
      const words = player.synthesizedWordCount
      timeEstimate.textContent = `Synthesized ${words} words in ${elapsed}s`
      synthStart = 0
    }
  })

  // Voice picker
  function renderVoices() {
    if (voices.length === 0) {
      voiceList.innerHTML = '<div class="voice-loading">No voices available.</div>'
      return
    }
    voiceList.innerHTML = ''
    let lastBackend = ''
    for (const v of voices) {
      if (v.backend !== lastBackend) {
        const hdr = document.createElement('div')
        hdr.className = 'voice-group-header'
        hdr.textContent = v.backend.toUpperCase()
        voiceList.appendChild(hdr)
        lastBackend = v.backend
      }
      const row = document.createElement('button')
      row.className = 'voice-row' + (v.id === selectedVoice ? ' selected' : '')
      row.textContent = v.name
      row.addEventListener('click', () => selectVoice(v.id))
      voiceList.appendChild(row)
    }
  }

  function selectVoice(id: string) {
    selectedVoice = id
    const v = voices.find(v => v.id === id)
    voiceDescription.textContent = v?.description ?? ''
    renderVoices()
  }

  async function loadVoices() {
    try {
      const res = await fetch('/voices')
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const data: VoicesResponse = await res.json()
      voices = data.voices
      if (voices.length > 0 && !selectedVoice) selectVoice(voices[0].id)
      else renderVoices()
      updateEstimate(textInput.value)
    } catch {
      voiceList.innerHTML = '<div class="voice-loading error">Failed to load voices.</div>'
    }
  }

  loadVoices()

  // Time estimate
  function fmtDuration(secs: number): string {
    if (secs < 60) return `~${Math.round(secs)}s`
    const m = Math.floor(secs / 60)
    const s = Math.round(secs % 60)
    return s > 0 ? `~${m}m ${s}s` : `~${m}m`
  }

  function updateEstimate(text: string) {
    const words = text.trim().split(/\s+/).filter(Boolean).length
    if (words === 0) { timeEstimate.textContent = ''; return }
    const backend = selectedVoice?.split(':')[0] ?? 'kokoro'
    const rate = SECS_PER_WORD[backend] ?? 0.2
    const secs = words * rate
    timeEstimate.textContent = `${fmtDuration(secs)} to synthesize (${words} words)`
  }

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

  synthBtn.addEventListener('click', () => {
    const text = textInput.value.trim()
    if (!text) return
    synthBtn.disabled = true
    playBtn.disabled = true
    downloadBtn.disabled = true
    progressFill.style.width = '0%'
    timeCurrent.textContent = '0:00'
    timeTotal.textContent = '0:00'
    synthStart = Date.now()
    player.synthesize(text, selectedVoice ?? undefined)
  })

  textInput.addEventListener('input', () => updateEstimate(textInput.value))

  textInput.addEventListener('keydown', (e: KeyboardEvent) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) synthBtn.click()
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
      const cached = data.cached?.content ? ' (cached)' : ''
      const title = data.title ?? url
      fetchStatus.textContent = `✔ ${title}${cached}`
      fetchStatus.className = 'fetch-status success'
    } catch {
      fetchStatus.textContent = 'Network error'
      fetchStatus.className = 'fetch-status error'
    } finally {
      urlInput.disabled = false
      urlInput.focus()
    }
  })
}

// ── Boot ──────────────────────────────────────────────────────────────────────

showReader()
