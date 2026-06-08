import './style.css'
import { renderMarkdown } from './markdown'
import { ReaderCore, formatByline } from './reader-core'

// ---------------------------------------------------------------------------
// Types — injected by CLI at export time via window.__ODORU__
// ---------------------------------------------------------------------------

interface ManifestEntry {
  title: string
  slug: string
  authors?: string[]
  description?: string
  date?: string
  source_url?: string
  has_audio: boolean
}

interface TranscriptEntry {
  index: number
  text: string
  start: number
  end: number
  paragraph_end: boolean
}

interface DocumentContent {
  content: string     // markdown
  plain_text: string  // plain text for sentence splitting
}

interface OdoruExport {
  manifest: ManifestEntry[]
  transcripts: Record<string, TranscriptEntry[]>
  documents: Record<string, DocumentContent>
}

declare global {
  interface Window {
    __ODORU__?: OdoruExport
  }
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

const data: OdoruExport = window.__ODORU__ ?? { manifest: [], transcripts: {}, documents: {} }

// ---------------------------------------------------------------------------
// Player state
// ---------------------------------------------------------------------------

interface PlayerState {
  slug: string
  transcript: TranscriptEntry[]
  audioEls: HTMLAudioElement[]       // one per sentence, populated by prefetch
  currentIndex: number
  playing: boolean
  abortController: AbortController | null
  prefetchOffset: number             // next sentence index to prefetch
}

const PREFETCH_WINDOW = 15

let player: PlayerState | null = null
let core: ReaderCore | null = null

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

function render() {
  const app = document.getElementById('app')!
  app.innerHTML = `
    <div class="reader-layout">
      <nav class="article-sidebar">
        <div class="sidebar-top">
          <div class="sidebar-tabs">
            <button class="sidebar-tab active" id="tab-documents">Documents</button>
            <button class="sidebar-tab" id="tab-outline">Outline</button>
          </div>
        </div>
        <div class="article-list" id="article-list"></div>
        <div class="outline-list" id="outline-list" style="display:none"></div>
      </nav>
      <div class="reader-main">
        <div class="reader-header" id="reader-header" style="display:none">
          <h1 class="article-title" id="article-title"></h1>
          <div class="article-byline" id="article-byline"></div>
          <div class="article-source-url" id="article-source-url"></div>
        </div>
        <div id="transcript-container" class="transcript-container">
          <div class="loading">${data.manifest.length === 0 ? 'No published documents.' : 'Select a document.'}</div>
        </div>
        <div class="controls">
          <button id="play-btn" class="play-btn" disabled><span id="play-icon" class="play-icon">▶</span></button>
          <div class="progress-wrap">
            <div class="progress-bar"><div id="progress-fill" class="progress-fill"></div></div>
            <div class="time-row">
              <span id="time-current" class="time">0:00</span>
              <span id="time-total" class="time">0:00</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  `

  core = new ReaderCore(
    document.getElementById('transcript-container')!,
    document.getElementById('outline-list')!,
  )
  populateSidebar()
  wireTabSwitcher()
  wirePlayButton()
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

function populateSidebar() {
  const list = document.getElementById('article-list')!
  if (data.manifest.length === 0) {
    list.innerHTML = '<div class="outline-loading">No documents.</div>'
    return
  }
  const sorted = [...data.manifest].sort((a, b) =>
    a.title.localeCompare(b.title, undefined, { sensitivity: 'base' }))

  list.innerHTML = sorted.map((entry, i) => `
    <div class="article-item live${i === 0 ? ' selected' : ''}"
         data-slug="${entry.slug}"
         title="${entry.description ?? ''}">
      ${entry.title}
    </div>
  `).join('')

  list.querySelectorAll<HTMLElement>('.article-item').forEach(el => {
    el.addEventListener('click', () => {
      list.querySelectorAll('.article-item').forEach(x => x.classList.remove('selected'))
      el.classList.add('selected')
      loadDocument(el.dataset.slug!)
    })
  })

  if (sorted.length > 0) loadDocument(sorted[0].slug)
}

// ---------------------------------------------------------------------------
// Document loading
// ---------------------------------------------------------------------------

function loadDocument(slug: string) {
  stopPlayer()

  const entry = data.manifest.find(e => e.slug === slug)
  if (!entry) return

  // Header
  const header = document.getElementById('reader-header')!
  header.style.display = ''
  document.getElementById('article-title')!.textContent = entry.title
  document.getElementById('article-byline')!.textContent =
    formatByline(entry.authors ?? [], entry.date)


  const sourceUrlEl = document.getElementById('article-source-url')!
  sourceUrlEl.innerHTML = ''
  if (entry.source_url) {
    const a = document.createElement('a')
    a.href = entry.source_url
    a.textContent = entry.source_url
    a.title = entry.source_url
    a.target = '_blank'
    a.rel = 'noopener noreferrer'
    sourceUrlEl.appendChild(a)
  }

  // Render via markdown.ts — shares classes and styles with main reader
  const container = document.getElementById('transcript-container')!
  container.innerHTML = ''
  const doc = data.documents[slug]
  const transcript = data.transcripts[slug] ?? []

  let spans: HTMLElement[] = []
  let headings: ReturnType<typeof renderMarkdown>['headings'] = []
  if (doc?.content) {
    const result = renderMarkdown(doc.content, doc.plain_text, container)
    spans = result.pendingSpans
    headings = result.headings
  } else if (transcript.length > 0) {
    // No markdown content — fall back to plain sentence spans
    spans = transcript.map(seg => {
      const span = document.createElement('span')
      span.className = 'segment pending'
      span.textContent = seg.text + ' '
      container.appendChild(span)
      return span
    })
  } else {
    container.innerHTML = '<div class="loading">No content available.</div>'
  }

  // Player
  const playBtn = document.getElementById('play-btn') as HTMLButtonElement
  if (entry.has_audio && transcript.length > 0) {
    core!.loadSpans(spans, true, index => {
      if (!player) return
      player.playing = true
      const icon = document.getElementById('play-icon')
      if (icon) icon.textContent = '⏸'
      playSentence(index)
    })
    core!.renderOutline(headings, index => {
      if (!player) return
      if (player.playing) {
        playSentence(index)
      } else {
        player.currentIndex = index
        core!.deactivateAll()
        core!.activateSpan(index)
      }
    })
    initPlayer(slug, transcript)
    playBtn.disabled = false
  } else {
    core!.loadSpans(spans, false)
    core!.renderOutline(headings, () => {})
    playBtn.disabled = true
  }

  updateTimeDisplay(0, totalDuration(transcript))
}

// ---------------------------------------------------------------------------
// Player
// ---------------------------------------------------------------------------

function totalDuration(transcript: TranscriptEntry[]): number {
  if (transcript.length === 0) return 0
  return transcript[transcript.length - 1].end
}

function initPlayer(slug: string, transcript: TranscriptEntry[]) {
  player = {
    slug,
    transcript,
    audioEls: new Array(transcript.length).fill(null),
    currentIndex: 0,
    playing: false,
    abortController: null,
    prefetchOffset: 0,
  }
  startPrefetch(0)
}

function stopPlayer() {
  if (!player) return
  player.abortController?.abort()
  player.audioEls.forEach(el => {
    if (!el) return
    el.pause()
    el.src = ''
  })
  player = null
  const playBtn = document.getElementById('play-btn') as HTMLButtonElement | null
  if (playBtn) { playBtn.disabled = true }
  const icon = document.getElementById('play-icon')
  if (icon) icon.textContent = '▶'
}

function audioPath(slug: string, index: number): string {
  return `documents/${slug}/audio/${String(index).padStart(4, '0')}.mp3`
}

function startPrefetch(fromIndex: number) {
  if (!player) return
  player.abortController?.abort()
  const ac = new AbortController()
  player.abortController = ac
  player.prefetchOffset = fromIndex

  const prefetchNext = () => {
    if (!player || ac.signal.aborted) return
    const i = player.prefetchOffset
    if (i >= player.transcript.length) return
    if (i >= player.currentIndex + PREFETCH_WINDOW) return

    if (!player.audioEls[i]) {
      const audio = new Audio(audioPath(player.slug, i))
      audio.preload = 'auto'
      player.audioEls[i] = audio
    }
    player.prefetchOffset++
    prefetchNext()
  }
  prefetchNext()
}

function playSentence(index: number) {
  if (!player) return
  if (index >= player.transcript.length) {
    // Playback complete
    player.playing = false
    const icon = document.getElementById('play-icon')
    if (icon) icon.textContent = '▶'
    return
  }

  // Pause whatever was playing before switching to the new sentence
  player.audioEls[player.currentIndex]?.pause()

  player.currentIndex = index
  startPrefetch(Math.max(0, index))  // reset window from seek point

  deactivateAllSpans()
  activateSpan(index)

  const audio = player.audioEls[index] ?? new Audio(audioPath(player.slug, index))
  player.audioEls[index] = audio
  audio.currentTime = 0

  const seg = player.transcript[index]

  audio.ontimeupdate = () => {
    if (!player) return
    updateProgress(seg.start + audio.currentTime, totalDuration(player.transcript))
  }

  audio.onended = () => {
    if (!player || !player.playing) return
    playSentence(index + 1)
  }

  audio.play().catch(() => {/* autoplay blocked — user will click play again */})
}

function wirePlayButton() {
  const btn = document.getElementById('play-btn') as HTMLButtonElement
  btn.addEventListener('click', () => {
    if (!player) return
    if (player.playing) {
      player.playing = false
      player.audioEls[player.currentIndex]?.pause()
      btn.querySelector('#play-icon')!.textContent = '▶'
    } else {
      player.playing = true
      btn.querySelector('#play-icon')!.textContent = '⏸'
      // Resume current or start from beginning
      const audio = player.audioEls[player.currentIndex]
      if (audio && audio.paused && audio.currentTime > 0) {
        audio.play().catch(() => {})
      } else {
        playSentence(player.currentIndex)
      }
    }
  })
}

// ---------------------------------------------------------------------------
// Span highlighting — delegates to ReaderCore
// ---------------------------------------------------------------------------

function activateSpan(index: number) { core?.activateSpan(index) }
function deactivateAllSpans() { core?.deactivateAll() }

// ---------------------------------------------------------------------------
// Progress display
// ---------------------------------------------------------------------------

function formatTime(s: number): string {
  const m = Math.floor(s / 60)
  const sec = Math.floor(s % 60)
  return `${m}:${String(sec).padStart(2, '0')}`
}

function updateProgress(current: number, total: number) {
  const fill = document.getElementById('progress-fill')
  const cur = document.getElementById('time-current')
  const tot = document.getElementById('time-total')
  if (fill) fill.style.width = total > 0 ? `${(current / total) * 100}%` : '0%'
  if (cur) cur.textContent = formatTime(current)
  if (tot) tot.textContent = formatTime(total)
}

function updateTimeDisplay(current: number, total: number) {
  updateProgress(current, total)
}

// ---------------------------------------------------------------------------
// Tab switcher
// ---------------------------------------------------------------------------

function wireTabSwitcher() {
  const tabDocs = document.getElementById('tab-documents')!
  const tabOutline = document.getElementById('tab-outline')!
  const articleList = document.getElementById('article-list')!
  const outlineList = document.getElementById('outline-list')!

  tabDocs.addEventListener('click', () => {
    tabDocs.classList.add('active'); tabOutline.classList.remove('active')
    articleList.style.display = ''; outlineList.style.display = 'none'
  })
  tabOutline.addEventListener('click', () => {
    tabOutline.classList.add('active'); tabDocs.classList.remove('active')
    outlineList.style.display = ''; articleList.style.display = 'none'
  })
}

render()
