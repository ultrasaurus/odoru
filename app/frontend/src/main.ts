import './style.css'
import { Player } from './player'

const app = document.getElementById('app')!

app.innerHTML = `
  <div class="layout">
    <header class="header">
      <div class="logo">▶ ko-odoru</div>
    </header>

    <main class="main">
      <div class="card">
        <div class="input-area">
          <textarea
            id="text-input"
            class="text-input"
            placeholder="Paste or type text to synthesize…"
            rows="4"
          ></textarea>
          <button id="synth-btn" class="synth-btn">Synthesize</button>
        </div>

        <div id="transcript-container" class="transcript-container">
          <div class="placeholder">Enter text above and press Synthesize.</div>
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
        </div>
      </div>
    </main>
  </div>
`

const synthBtn   = document.getElementById('synth-btn')   as HTMLButtonElement
const textInput  = document.getElementById('text-input')  as HTMLTextAreaElement
const playBtn    = document.getElementById('play-btn')    as HTMLButtonElement
const playIcon   = playBtn.querySelector('.play-icon')    as HTMLSpanElement
const progressFill = document.getElementById('progress-fill') as HTMLDivElement
const timeCurrent  = document.getElementById('time-current')  as HTMLSpanElement
const timeTotal    = document.getElementById('time-total')    as HTMLSpanElement
const transcriptContainer = document.getElementById('transcript-container') as HTMLDivElement

function fmt(s: number): string {
  const m = Math.floor(s / 60)
  const sec = Math.floor(s % 60)
  return `${m}:${sec.toString().padStart(2, '0')}`
}

const player = new Player(transcriptContainer)

player.onReady(() => {
  playBtn.disabled = false
  playIcon.textContent = '▶'
  // Auto-play when first segment is ready
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

player.onEnded(() => {
  playIcon.textContent = '▶'
  progressFill.style.width = '100%'
  synthBtn.disabled = false
})

synthBtn.addEventListener('click', () => {
  const text = textInput.value.trim()
  if (!text) return

  synthBtn.disabled = true
  playBtn.disabled = true
  playIcon.textContent = '▶'
  progressFill.style.width = '0%'
  timeCurrent.textContent = '0:00'
  timeTotal.textContent = '0:00'

  player.synthesize(text)
})

playBtn.addEventListener('click', () => {
  player.toggle()
  playIcon.textContent = player.paused ? '▶' : '⏸'
})

// Allow Ctrl+Enter to trigger synthesis
textInput.addEventListener('keydown', (e: KeyboardEvent) => {
  if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
    synthBtn.click()
  }
})
