import './style.css'
import { Player } from './player'
import { renderMarkdown, type HeadingEntry } from './markdown'
import { Document, type DocumentState, type VoiceEntry } from './document'

interface VoiceInfo {
  id: string          // prefixed, e.g. "f5:sarah" or "kokoro:am_puck"
  name: string        // display name, e.g. "sarah"
  backend: string     // "f5" or "kokoro"
  description: string
}

interface VoicesResponse {
  voices: VoiceInfo[]
}

interface JobInfo {
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

// DocumentState and VoiceEntry imported from ./document

// Approximate generation seconds per word for each backend.
// Kokoro: ~0.2 sec/word (measured: 143 words in 26s)
// F5:     ~3.0 sec/word (measured: 143 words in 410s)
const SECS_PER_WORD: Record<string, number> = {
  kokoro: 0.2,
  f5: 3.0,
}

// Pick the best voice from a document's voices map.
// Priority: published → first ready → first stale → first any.
function pickVoice(voices: Record<string, VoiceEntry>): string | null {
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

const app = document.getElementById('app')!

// Module-level cleanup — stops any timers belonging to the current view
// before the next view replaces the DOM.
let viewCleanup: (() => void) | null = null

// ── Shared helpers ────────────────────────────────────────────────────────────

// Safe alternative to innerHTML interpolation for single-element status messages.
function makeEl(tag: string, className: string, text: string): HTMLElement {
  const el = document.createElement(tag)
  el.className = className
  el.textContent = text
  return el
}

function setError(container: HTMLElement, msg: string): void {
  container.innerHTML = ''
  container.appendChild(makeEl('div', 'error', msg))
}

function setStatus(container: HTMLElement, className: string, msg: string): void {
  container.innerHTML = ''
  container.appendChild(makeEl('span', className, msg))
}

function formatByline(authors: string[], date?: string): string {
  const authorStr = (authors ?? []).length > 0 ? `by ${authors.join(', ')}` : ''
  const dateStr = date
    ? new Date(date + 'T12:00:00').toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' })
    : ''
  if (authorStr && dateStr) return `${authorStr}, ${dateStr}`
  return authorStr || dateStr
}

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
  getFilename: () => string,   // evaluated at click time, not at init
) {
  const playIcon = playBtn.querySelector('.play-icon') as HTMLSpanElement

  player.onReady(() => {
    playBtn.disabled = false
  })

  // Enable download as soon as all audio is received — no need to wait
  // until the end of playback.
  player.onSynthDone(() => {
    downloadBtn.disabled = false
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
  })

  playBtn.addEventListener('click', () => {
    player.toggle()
    playIcon.textContent = player.paused ? '▶' : '⏸'
  })

  downloadBtn.addEventListener('click', () => {
    player.downloadWav(getFilename())
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
        <div id="seek-status" class="seek-status" style="display:none">Waiting for audio to arrive…</div>
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
  viewCleanup?.()

  app.innerHTML = `
    <div class="reader-layout">
      <nav class="article-sidebar">
        <div class="sidebar-top">
          <button class="new-btn" id="new-btn">Edit</button>
          <div class="sidebar-tabs">
            <button class="sidebar-tab" id="tab-articles">Documents</button>
            <button class="sidebar-tab active" id="tab-outline">Outline</button>
          </div>
        </div>
        <div class="article-list" id="article-list" style="display:none">
          <div class="outline-loading">Loading…</div>
        </div>
        <div class="outline-list" id="outline-list">
          <div class="outline-loading">Loading…</div>
        </div>
      </nav>
      <div class="reader-main">
        <div class="reader-header">
          <h1 class="article-title" id="article-title">Loading…</h1>
          <div class="article-byline" id="article-byline"></div>
          <div class="article-source-url" id="article-source-url"></div>
          <div class="reader-header-row">
            <div id="job-area" class="job-area"></div>
            <label class="autoscroll-label">
              <input type="checkbox" id="autoscroll-cb" class="autoscroll-cb">
              Auto-scroll
            </label>
          </div>
        </div>
        <div id="transcript-container" class="transcript-container">
          <div class="loading">Loading…</div>
        </div>
        ${controlsHtml()}
      </div>
    </div>
  `

  document.getElementById('new-btn')!.addEventListener('click', showEdit)

  // ── Sidebar tabs ───────────────────────────────────────────────────────────
  const tabArticles  = document.getElementById('tab-articles')!
  const tabOutline   = document.getElementById('tab-outline')!
  const articleList  = document.getElementById('article-list')!
  const outlineList  = document.getElementById('outline-list')!

  function showTab(tab: 'articles' | 'outline') {
    const isArticles = tab === 'articles'
    tabArticles.classList.toggle('active', isArticles)
    tabOutline.classList.toggle('active', !isArticles)
    articleList.style.display = isArticles ? '' : 'none'
    outlineList.style.display = isArticles ? 'none' : ''
  }

  tabArticles.addEventListener('click', () => showTab('articles'))
  tabOutline.addEventListener('click',  () => showTab('outline'))

  const articleTitleEl      = document.getElementById('article-title')!
  const articleBylineEl     = document.getElementById('article-byline')!
  const articleSourceUrlEl  = document.getElementById('article-source-url')!
  const transcriptContainer = document.getElementById('transcript-container')!
  const jobArea             = document.getElementById('job-area')!
  const autoscrollCb        = document.getElementById('autoscroll-cb') as HTMLInputElement
  const { playBtn, downloadBtn, progressFill, timeCurrent, timeTotal } = grabControlEls()
  const seekStatus = document.getElementById('seek-status') as HTMLDivElement

  const player = new Player(transcriptContainer)

  autoscrollCb.checked = true
  player.autoScroll = true
  autoscrollCb.addEventListener('change', () => { player.autoScroll = autoscrollCb.checked })

  player.onError(msg => {
    setError(transcriptContainer, `Error: ${msg}`)
    playBtn.disabled = true
  })

  player.onWaiting(() => {
    playBtn.disabled = true
    seekStatus.style.display = ''
  })

  player.onSeekReady(() => {
    playBtn.disabled = false
    seekStatus.style.display = 'none'
  })

  let currentDoc: DocumentState | null = null
  wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal,
    () => (currentDoc?.title ?? currentDoc?.source_url ?? 'document').replace(/[^a-z0-9]+/gi, '-').toLowerCase() + '.wav')

  // ── Outline ────────────────────────────────────────────────────────────────
  let headings: HeadingEntry[] = []
  let outlineEls: HTMLElement[] = []
  let activeOutlineIdx = -1

  function renderOutline(hs: HeadingEntry[]) {
    headings = hs
    outlineEls = []
    activeOutlineIdx = -1
    outlineList.innerHTML = ''

    if (hs.length === 0) {
      outlineList.innerHTML = '<div class="outline-loading">No headings</div>'
      return
    }

    const minDepth = Math.min(...hs.map(h => h.depth))
    for (const h of hs) {
      const el = document.createElement('div')
      el.className = 'outline-item'
      el.dataset.depth = String(h.depth - minDepth)
      el.textContent = h.text
      el.addEventListener('click', () => {
        h.element.scrollIntoView({ behavior: 'instant', block: 'start' })
        player.seekTo(h.sentenceIndex)
      })
      outlineList.appendChild(el)
      outlineEls.push(el)
    }
  }

  function updateOutlineActive(position: number) {
    let found = -1
    for (let i = 0; i < headings.length; i++) {
      const t = player.segmentStartTime(headings[i].sentenceIndex)
      if (t !== null && t <= position) found = i
      else if (t !== null) break
    }
    if (found === activeOutlineIdx) return
    if (activeOutlineIdx >= 0) outlineEls[activeOutlineIdx]?.classList.remove('active')
    activeOutlineIdx = found
    if (found >= 0) outlineEls[found]?.classList.add('active')
  }

  // ── Job polling ────────────────────────────────────────────────────────────
  let pollTimer: ReturnType<typeof setTimeout> | null = null

  function stopPolling() {
    if (pollTimer !== null) { clearTimeout(pollTimer); pollTimer = null }
  }

  viewCleanup = () => { stopPolling(); player.stop() }

  function pollJob(jobId: string, total: number, onDone?: () => void) {
    stopPolling()
    pollTimer = setTimeout(async () => {
      try {
        const res = await fetch(`/jobs/${jobId}`)
        if (!res.ok) {
          setStatus(jobArea, 'job-status error', 'Job not found — server may have restarted')
          return
        }
        const job: JobInfo = await res.json()
        if (job.status === 'done') {
          setStatus(jobArea, 'job-status done', '✓ Audio ready')
          onDone?.()
          return
        }
        if (job.status === 'error') {
          setStatus(jobArea, 'job-status error', `Synthesis error: ${job.error ?? ''}`)
          return
        }
        const pct = total > 0 ? Math.round((job.completed_sentences / total) * 100) : 0
        setStatus(jobArea, 'job-status running', `Synthesizing… ${job.completed_sentences}/${total} (${pct}%)`)
        pollJob(jobId, total, onDone)
      } catch {
        pollJob(jobId, total, onDone) // retry silently on network blip
      }
    }, 4000)
  }

  async function startJob(plainText: string, documentId: string, voice: string) {
    setStatus(jobArea, 'job-status running', 'Queuing…')
    try {
      const res = await fetch('/jobs', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ text: plainText, voice, document_id: documentId }),
      })
      const job: JobInfo = await res.json()
      if (!res.ok) {
        setStatus(jobArea, 'job-status error', job.error ?? 'Failed to queue')
        return
      }
      if (job.status === 'done') {
        setStatus(jobArea, 'job-status done', '✓ Audio ready')
        return
      }
      pollJob(job.id, job.total_sentences)
    } catch {
      setStatus(jobArea, 'job-status error', 'Could not reach server')
    }
  }

  // ── Load document ──────────────────────────────────────────────────────────
  function loadDocument(doc: DocumentState) {
    currentDoc = doc
    stopPolling()
    jobArea.innerHTML = ''
    playBtn.disabled = true
    downloadBtn.disabled = true
    progressFill.style.width = '0%'
    timeCurrent.textContent = '0:00'
    timeTotal.textContent = '0:00'
    transcriptContainer.innerHTML = '<div class="loading">Loading…</div>'
    articleTitleEl.textContent = doc.title ?? doc.source_url ?? doc.id
    articleBylineEl.textContent = formatByline(doc.authors, doc.date)
    articleSourceUrlEl.innerHTML = ''
    if (doc.source_url) {
      const a = document.createElement('a')
      a.href = doc.source_url
      a.textContent = doc.source_url
      a.title = doc.source_url
      a.target = '_blank'
      a.rel = 'noopener noreferrer'
      articleSourceUrlEl.appendChild(a)
    }

    Document.load(doc.id)
      .then(d => { d.destroy(); return d.current })
      .then((data: DocumentState) => {
        if (data.status === 'error') {
          setError(transcriptContainer, `Failed to load: ${data.error ?? 'unknown error'}`)
          return
        }
        if (!data.content || !data.plain_text) {
          setError(transcriptContainer, 'Document content not yet available.')
          return
        }

        const voice = pickVoice(data.voices)
        const voiceEntry = voice ? data.voices[voice] : undefined
        // Any non-error voice entry means there's audio to stream (full, partial, or stale).
        const audioReady = !!voiceEntry && voiceEntry.status !== 'error'

        if (voiceEntry?.duration) {
          timeTotal.textContent = fmt(voiceEntry.duration)
        }

        // When audio is ready, send the synth request FIRST so the server
        // starts loading from disk while we do DOM work below. JS is
        // single-threaded so no onSegment callbacks fire until renderMarkdown
        // returns and we call setPendingSpans — spans are always ready in time.
        if (audioReady) {
          player.synthesize(data.plain_text!, voice!, [], doc.id)
        }

        // Always render transcript (used for reading even without audio).
        transcriptContainer.innerHTML = ''
        const { pendingSpans, headings: hs } = renderMarkdown(data.content, data.plain_text, transcriptContainer)
        renderOutline(hs)
        player.onTimeUpdate(t => updateOutlineActive(t))

        // Hand the spans to the player now that they exist.
        if (audioReady) {
          player.setPendingSpans(pendingSpans)
        }

        function synthesizeNow() {
          // Called when a background job finishes — transcript already rendered.
          player.synthesize(data.plain_text!, voice!, pendingSpans, doc.id)
        }

        function showSynthButton() {
          if (!voice) return  // no voice available; user must synthesize from queue view
          const btn = document.createElement('button')
          btn.className = 'job-btn'
          btn.textContent = 'Synthesize in background'
          btn.addEventListener('click', () => {
            btn.remove()
            startJob(data.plain_text!, doc.id, voice)
          })
          jobArea.appendChild(btn)
        }

        if (audioReady) {
          setStatus(jobArea, 'job-status done', '✓ Audio ready')
        } else {
          // Check for an existing active job before showing the button.
          fetch('/jobs')
            .then(res => res.ok ? res.json() : [])
            .then((jobs: JobInfo[]) => {
              const active = jobs.find(j =>
                j.document_id === doc.id &&
                (j.status === 'pending' || j.status === 'in_progress')
              )
              if (active) {
                const pct = active.total_sentences > 0
                  ? Math.round((active.completed_sentences / active.total_sentences) * 100) : 0
                setStatus(jobArea, 'job-status running',
                  `Synthesizing… ${active.completed_sentences}/${active.total_sentences} (${pct}%)`)
                pollJob(active.id, active.total_sentences, synthesizeNow)
              } else {
                showSynthButton()
              }
            })
            .catch(showSynthButton)
        }
      })
      .catch(() => {
        setError(transcriptContainer, 'Failed to load document.')
        stopPolling()
      })
  }

  // ── Fetch document list + load first ──────────────────────────────────────
  fetch('/documents')
    .then(res => res.json())
    .then((all: DocumentState[]) => {
      const docs = all.filter(d => d.publish)
        .sort((a, b) => (a.title ?? a.source_url ?? a.id)
          .localeCompare(b.title ?? b.source_url ?? b.id, undefined, { sensitivity: 'base' }))
      articleList.innerHTML = ''
      if (docs.length === 0) {
        articleList.innerHTML = '<div class="outline-loading">No documents.</div>'
        transcriptContainer.innerHTML = '<div class="loading">No documents found.</div>'
        articleTitleEl.textContent = ''
        return
      }
      const itemEls: HTMLElement[] = []
      docs.forEach((doc, i) => {
        const el = document.createElement('div')
        el.className = 'article-item' + (i === 0 ? ' selected' : '')
        el.textContent = doc.title ?? doc.source_url ?? doc.id
        el.addEventListener('click', () => {
          itemEls.forEach(e => e.classList.remove('selected'))
          el.classList.add('selected')
          loadDocument(doc)
        })
        articleList.appendChild(el)
        itemEls.push(el)
      })
      loadDocument(docs[0])
    })
    .catch(() => {
      articleList.innerHTML = '<div class="outline-loading">Failed to load documents.</div>'
      setError(transcriptContainer, 'Failed to load document list.')
      articleTitleEl.textContent = ''
    })
}

// ── New view ──────────────────────────────────────────────────────────────────

function showEdit() {
  viewCleanup?.()

  let voices: VoiceInfo[] = []
  let selectedVoice: string | null = null  // stores prefixed id, e.g. "f5:sarah"
  let synthStart = 0
  let fetchedDocument: Document | null = null

  app.innerHTML = `
    <div class="layout">
      <div id="error-bar" class="error-bar" style="display:none">
        <span id="error-bar-msg" class="error-bar-msg"></span>
        <button id="error-bar-retry" class="error-bar-retry">Retry</button>
      </div>
      <header class="header">
        <a class="back-link" id="back-link">← Documents</a>
        <div class="logo">▶ odoru</div>
      </header>
      <!-- TODO: generalize error-bar into shared layout wrapper -->

      <main class="main">
        <div class="workspace">
          <div class="card-column">
          <div id="queue-section" class="queue-section">
            <div class="queue-header">Documents</div>
            <div id="queue-list" class="queue-list"></div>
          </div>

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
                placeholder="Fetched text will appear here…"
                rows="4"
                readonly
              ></textarea>
              <div class="synth-row">
                <div id="time-estimate" class="time-estimate"></div>
                <label class="bg-synth-label">
                  <input type="checkbox" id="bg-synth-cb" class="bg-synth-cb">
                  Synthesize in background
                </label>
                <button id="synth-btn" class="synth-btn">Synthesize</button>
              </div>
            </div>

            <div id="transcript-container" class="transcript-container">
              <div class="placeholder">Fetch a URL above, then press Synthesize.</div>
            </div>

            ${controlsHtml()}
          </div>
          </div><!-- end card-column -->

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

  const queueList = document.getElementById('queue-list')!

  // ── Documents panel ────────────────────────────────────────────────────────
  let queuePollTimer: ReturnType<typeof setTimeout> | null = null
  let bgPollTimer:    ReturnType<typeof setTimeout> | null = null
  const openMetaForms = new Set<string>()  // doc IDs with metadata form expanded

  function stopQueuePoll() {
    if (queuePollTimer !== null) { clearTimeout(queuePollTimer); queuePollTimer = null }
  }
  function stopBgPoll() {
    if (bgPollTimer !== null) { clearTimeout(bgPollTimer); bgPollTimer = null }
  }

  viewCleanup = () => { stopQueuePoll(); stopBgPoll() }

  function jobStatusLabel(status: string): string {
    return ({
      pending:     '⏳ Pending',
      in_progress: '⚙ Running',
      done:        '✓ Ready',
      error:       '✕ Error',
      cancelled:   '— Cancelled',
    } as Record<string, string>)[status] ?? status
  }

  function renderQueue(docs: DocumentState[], jobs: JobInfo[]) {
    queueList.innerHTML = ''
    if (docs.length === 0) {
      const empty = document.createElement('div')
      empty.className = 'queue-empty'
      empty.textContent = 'No documents yet.'
      queueList.appendChild(empty)
      return
    }

    // Build document_id → best job map (prefer active > done > others, then newest)
    const jobMap = new Map<string, JobInfo>()
    for (const job of jobs) {
      if (!job.document_id) continue
      const existing = jobMap.get(job.document_id)
      if (!existing) { jobMap.set(job.document_id, job); continue }
      const rank = (s: string) => s === 'in_progress' ? 0 : s === 'pending' ? 1 : s === 'done' ? 2 : 3
      const better = rank(job.status) < rank(existing.status) ||
        (rank(job.status) === rank(existing.status) && job.created_at > existing.created_at)
      if (better) jobMap.set(job.document_id, job)
    }

    // Check if any voice is ready in a document's voices map
    const hasReadyVoice = (doc: DocumentState) =>
      Object.values(doc.voices).some(v => !!v.duration)

    // Assign sort rank
    const sortRank = (doc: DocumentState) => {
      const job = jobMap.get(doc.id)
      if (job?.status === 'in_progress') return 0
      if (job?.status === 'pending')     return 1
      if (job?.status === 'done')        return 2
      if (hasReadyVoice(doc))            return 3
      return 4
    }

    const sorted = [...docs].sort((a, b) => {
      const dr = sortRank(a) - sortRank(b)
      if (dr !== 0) return dr
      return (b.cached_at ?? '').localeCompare(a.cached_at ?? '')
    })

    for (const doc of sorted) {
      const job    = jobMap.get(doc.id)
      const active = job?.status === 'pending' || job?.status === 'in_progress'
      const pct    = job && job.total_sentences > 0
        ? Math.round((job.completed_sentences / job.total_sentences) * 100) : 0

      // Determine status label + voice name for display
      let statusText = ''
      let statusClass = ''
      let displayVoiceName = ''

      if (job) {
        statusText  = jobStatusLabel(job.status)
        statusClass = job.status
        // Show voice name in meta line only while active — picker handles it when ready
        if (active) displayVoiceName = voices.find(v => v.id === job.voice)?.name ?? job.voice
      } else if (hasReadyVoice(doc)) {
        statusText  = '✓ Ready'
        statusClass = 'done'
      }

      const row = document.createElement('div')
      row.className = 'queue-row'

      // Top line: title + status badge
      const top = document.createElement('div')
      top.className = 'queue-row-top'

      const titleEl = document.createElement('span')
      titleEl.className = 'queue-title'
      titleEl.textContent = doc.title ?? doc.source_url ?? doc.id
      top.appendChild(titleEl)

      if (statusText) {
        const statusEl = document.createElement('span')
        statusEl.className = `queue-status ${statusClass}`
        statusEl.textContent = statusText
        top.appendChild(statusEl)
      }

      // Delete button — far right of top row
      const deleteBtn = document.createElement('button')
      deleteBtn.className = 'queue-delete-btn'
      deleteBtn.textContent = '🗑'
      deleteBtn.title = 'Delete document'

      deleteBtn.addEventListener('click', () => {
        // Replace trash btn with inline confirm
        deleteBtn.replaceWith(confirmEl)
      })

      const confirmEl = document.createElement('span')
      confirmEl.className = 'queue-delete-confirm'

      const confirmLabel = document.createElement('span')
      confirmLabel.className = 'queue-delete-label'
      confirmLabel.textContent = 'Delete?'

      const confirmYes = document.createElement('button')
      confirmYes.className = 'queue-confirm-yes'
      confirmYes.textContent = '✓'
      confirmYes.addEventListener('click', async () => {
        row.remove()
        const res = await fetch(`/documents/${doc.id}`, { method: 'DELETE' })
        if (!res.ok) {
          queueList.appendChild(row)
          confirmEl.replaceWith(deleteBtn)
        }
        pollQueue()
      })

      const confirmNo = document.createElement('button')
      confirmNo.className = 'queue-confirm-no'
      confirmNo.textContent = '✗'
      confirmNo.addEventListener('click', () => {
        confirmEl.replaceWith(deleteBtn)
      })

      confirmEl.append(confirmLabel, confirmYes, confirmNo)
      top.appendChild(deleteBtn)

      row.appendChild(top)

      // Bottom line: voice + progress (only if there's something to show)
      if (displayVoiceName || active) {
        const meta = document.createElement('div')
        meta.className = 'queue-row-meta'

        if (displayVoiceName) {
          const voiceEl = document.createElement('span')
          voiceEl.className = 'queue-voice'
          voiceEl.textContent = displayVoiceName
          meta.appendChild(voiceEl)
        }

        if (active && job) {
          const bar = document.createElement('div')
          bar.className = 'queue-progress-bar'
          const fill = document.createElement('div')
          fill.className = 'queue-progress-fill'
          fill.style.width = `${pct}%`
          bar.appendChild(fill)
          meta.appendChild(bar)

          const pctEl = document.createElement('span')
          pctEl.className = 'queue-progress'
          pctEl.textContent = `${pct}%`
          meta.appendChild(pctEl)

          const cancelBtn = document.createElement('button')
          cancelBtn.className = 'queue-cancel-btn'
          cancelBtn.textContent = '✕'
          cancelBtn.addEventListener('click', async () => {
            await fetch(`/jobs/${job.id}`, { method: 'DELETE' })
            pollQueue()
          })
          meta.appendChild(cancelBtn)
        } else if (job?.status === 'done') {
          const countEl = document.createElement('span')
          countEl.className = 'queue-progress'
          countEl.textContent = `${job.total_sentences} sentences`
          meta.appendChild(countEl)
        }

        row.appendChild(meta)
      }

      // Publish controls — shown for any fetched document (status: ready)
      // Voice picker shown alongside only when ready/stale voices exist
      const readyVoices = Object.entries(doc.voices)
        .filter(([, v]) => !!v.duration)
      if (doc.status === 'ready') {
        const pub = document.createElement('div')
        pub.className = 'queue-row-publish'

        const cb = document.createElement('input')
        cb.type = 'checkbox'
        cb.className = 'queue-publish-cb'
        cb.checked = doc.publish
        cb.id = `pub-${doc.id}`

        const label = document.createElement('label')
        label.htmlFor = cb.id
        label.className = 'queue-publish-label'
        label.textContent = 'Publish'

        const select = document.createElement('select')
        select.className = 'queue-voice-select'
        for (const [vid, ve] of readyVoices) {
          const opt = document.createElement('option')
          opt.value = vid
          opt.textContent = voices.find(v => v.id === vid)?.name ?? vid
          opt.selected = !!ve.published
          select.appendChild(opt)
        }

        const patch = async () => {
          await fetch(`/documents/${doc.id}`, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ publish: cb.checked, published_voice: select.value || undefined }),
          })
        }

        cb.addEventListener('change', patch)
        if (readyVoices.length > 0) select.addEventListener('change', patch)

        // Pencil button to toggle metadata edit form
        const editBtn = document.createElement('button')
        editBtn.className = 'queue-edit-btn'
        editBtn.title = 'Edit metadata'
        editBtn.textContent = '✎'

        pub.append(cb, label)
        if (readyVoices.length > 0) pub.appendChild(select)
        pub.appendChild(editBtn)
        row.appendChild(pub)

        // Metadata edit form — hidden until pencil clicked
        const metaForm = document.createElement('div')
        metaForm.className = 'queue-meta-form'
        metaForm.style.display = 'none'

        const titleInput = document.createElement('input')
        titleInput.type = 'text'
        titleInput.className = 'queue-meta-input'
        titleInput.value = doc.title ?? ''

        const authorsInput = document.createElement('input')
        authorsInput.type = 'text'
        authorsInput.className = 'queue-meta-input'
        authorsInput.value = (doc.authors ?? []).join(', ')

        const dateInput = document.createElement('input')
        dateInput.type = 'date'
        dateInput.className = 'queue-meta-input'
        dateInput.value = doc.date ?? ''

        function makeMetaRow(labelText: string, input: HTMLInputElement): HTMLDivElement {
          const row = document.createElement('div')
          row.className = 'queue-meta-row'
          const lbl = document.createElement('label')
          lbl.className = 'queue-meta-label'
          lbl.textContent = labelText
          row.append(lbl, input)
          return row
        }

        const formActions = document.createElement('div')
        formActions.className = 'queue-meta-actions'

        const saveBtn = document.createElement('button')
        saveBtn.className = 'queue-meta-save'
        saveBtn.textContent = 'Save'

        const cancelBtn = document.createElement('button')
        cancelBtn.className = 'queue-meta-cancel'
        cancelBtn.textContent = 'Cancel'

        formActions.append(saveBtn, cancelBtn)
        metaForm.append(
          makeMetaRow('title:', titleInput),
          makeMetaRow('author(s):', authorsInput),
          makeMetaRow('date:', dateInput),
          formActions,
        )
        row.appendChild(metaForm)

        // Restore open state if this form was open before a re-render
        if (openMetaForms.has(doc.id)) {
          metaForm.style.display = ''
          editBtn.classList.add('active')
        }

        editBtn.addEventListener('click', () => {
          const open = metaForm.style.display !== 'none'
          metaForm.style.display = open ? 'none' : ''
          editBtn.classList.toggle('active', !open)
          if (open) openMetaForms.delete(doc.id)
          else openMetaForms.add(doc.id)
        })

        cancelBtn.addEventListener('click', () => {
          metaForm.style.display = 'none'
          editBtn.classList.remove('active')
          openMetaForms.delete(doc.id)
          if (openMetaForms.size === 0) pollQueue()
        })

        saveBtn.addEventListener('click', async () => {
          const authors = authorsInput.value.split(',').map(s => s.trim()).filter(Boolean)
          await fetch(`/documents/${doc.id}`, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              title: titleInput.value.trim() || undefined,
              authors,
              date: dateInput.value || undefined,
            }),
          })
          metaForm.style.display = 'none'
          editBtn.classList.remove('active')
          openMetaForms.delete(doc.id)
          // Update displayed title in this row immediately
          titleEl.textContent = titleInput.value.trim() || (doc.source_url ?? doc.id)
          pollQueue()
        })
      }

      queueList.appendChild(row)
    }
  }

  async function pollQueue() {
    stopQueuePoll()
    try {
      const [docsRes, jobsRes] = await Promise.all([
        fetch('/documents'),
        fetch('/jobs'),
      ])
      if (docsRes.ok && jobsRes.ok && openMetaForms.size === 0) {
        const allDocs: DocumentState[] = await docsRes.json()
        renderQueue(allDocs.filter(d => d.status !== 'error'), await jobsRes.json())
      }
    } catch { /* silent */ }
    queuePollTimer = setTimeout(pollQueue, 10_000)
  }

  pollQueue()

  // Error bar helpers
  const errorBar      = document.getElementById('error-bar')!
  const errorBarMsg   = document.getElementById('error-bar-msg')!
  const errorBarRetry = document.getElementById('error-bar-retry') as HTMLButtonElement

  function showErrorBar(msg: string) {
    errorBarMsg.textContent = msg
    errorBar.style.display = 'flex'
  }
  function hideErrorBar() {
    errorBar.style.display = 'none'
  }
  errorBarRetry.addEventListener('click', () => loadVoices())

  const synthBtn     = document.getElementById('synth-btn')    as HTMLButtonElement
  const bgSynthCb    = document.getElementById('bg-synth-cb')  as HTMLInputElement
  const textInput    = document.getElementById('text-input')   as HTMLTextAreaElement
  const timeEstimate = document.getElementById('time-estimate') as HTMLDivElement
  const urlInput     = document.getElementById('url-input')    as HTMLInputElement
  const fetchStatus  = document.getElementById('fetch-status') as HTMLDivElement
  const voiceList        = document.getElementById('voice-list')        as HTMLDivElement
  const voiceDescription = document.getElementById('voice-description') as HTMLDivElement
  const transcriptContainer = document.getElementById('transcript-container')!
  const { playBtn, downloadBtn, progressFill, timeCurrent, timeTotal } = grabControlEls()

  const player = new Player(transcriptContainer)

  player.onError(msg => {
    setError(transcriptContainer, `Error: ${msg}`)
    synthBtn.disabled = false
    playBtn.disabled = true
  })

  // downloadFilename is passed as a function so it's evaluated at click time.
  wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal,
    downloadFilename)

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
      hideErrorBar()
      if (voices.length > 0 && !selectedVoice) {
        const preferred = voices.find(v => v.id === 'kokoro:af_heart')
          ?? voices.find(v => v.id.startsWith('kokoro:'))
          ?? voices[0]
        selectVoice(preferred.id)
      }
      else renderVoices()
      updateEstimate(textInput.value)
    } catch {
      voiceList.innerHTML = '<div class="voice-loading">—</div>'
      showErrorBar('Could not reach server. Is it running?')
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

  // Fetch a URL via Document.fetch (POST /documents + WS watch).
  async function fetchDocument(url: string): Promise<boolean> {
    fetchStatus.textContent = 'Fetching…'
    fetchStatus.className = 'fetch-status loading'
    urlInput.disabled = true
    synthBtn.disabled = true
    fetchedDocument?.destroy()
    fetchedDocument = null
    textInput.value = ''

    try {
      const doc = await Document.fetch(url)
      const state = doc.current
      const wasDedup = !state.cached_at || Date.now() - new Date(state.cached_at).getTime() > 5000
      fetchedDocument = doc
      textInput.value = state.plain_text ?? ''
      updateEstimate(state.plain_text ?? '')
      const suffix = wasDedup ? ' (previously fetched)' : ''
      fetchStatus.textContent = `✔ ${state.title ?? url}${suffix}`
      fetchStatus.className = 'fetch-status success'
      return true
    } catch (e: any) {
      fetchStatus.textContent = e?.message ?? 'Fetch failed'
      fetchStatus.className = 'fetch-status error'
      return false
    } finally {
      urlInput.disabled = false
      synthBtn.disabled = false
    }
  }

  function startSynth(text: string) {
    synthBtn.disabled = true
    playBtn.disabled = true
    downloadBtn.disabled = true
    progressFill.style.width = '0%'
    timeCurrent.textContent = '0:00'
    timeTotal.textContent = '0:00'
    synthStart = Date.now()
    player.synthesize(text, selectedVoice ?? undefined, undefined, fetchedDocument?.current.id)
  }

  // ── Background job ─────────────────────────────────────────────────────────

  function pollBgJob(jobId: string, total: number) {
    stopBgPoll()
    bgPollTimer = setTimeout(async () => {
      try {
        const res = await fetch(`/jobs/${jobId}`)
        if (!res.ok) {
          setError(transcriptContainer, `Job not found (${res.status}) — server may have restarted`)
          synthBtn.disabled = false
          return
        }
        const job: JobInfo = await res.json()
        if (job.status === 'done') {
          transcriptContainer.innerHTML = '<div class="loading">✓ Background synthesis complete — press Synthesize to play</div>'
          synthBtn.disabled = false
          return
        }
        if (job.status === 'error') {
          setError(transcriptContainer, `Synthesis error: ${job.error ?? ''}`)
          synthBtn.disabled = false
          return
        }
        if (job.status === 'cancelled') {
          transcriptContainer.innerHTML = '<div class="loading">Job cancelled.</div>'
          synthBtn.disabled = false
          return
        }
        const pct = total > 0 ? Math.round((job.completed_sentences / total) * 100) : 0
        transcriptContainer.innerHTML =
          `<div class="loading">Background synthesis: ${job.completed_sentences}/${total} sentences (${pct}%)</div>`
        pollBgJob(jobId, total)
      } catch {
        pollBgJob(jobId, total) // retry silently on network blip
      }
    }, 4000)
  }

  async function startBgJob(text: string, documentId?: string) {
    stopBgPoll()
    synthBtn.disabled = true
    transcriptContainer.innerHTML = '<div class="loading">Queuing background job…</div>'
    try {
      const body: Record<string, string> = { text }
      if (selectedVoice) body.voice = selectedVoice
      if (documentId) body.document_id = documentId
      const res = await fetch('/jobs', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      const job: JobInfo = await res.json()
      if (!res.ok) {
        setError(transcriptContainer, job.error ?? 'Failed to queue job')
        synthBtn.disabled = false
        return
      }
      if (job.status === 'done') {
        // Audio already cached — play it directly via the live synthesis path.
        synthBtn.disabled = false
        startSynth(text)
        return
      }
      transcriptContainer.innerHTML =
        `<div class="loading">Background synthesis: 0/${job.total_sentences} sentences (0%)</div>`
      pollBgJob(job.id, job.total_sentences)
      pollQueue()
    } catch {
      transcriptContainer.innerHTML = '<div class="error">Could not reach server</div>'
      synthBtn.disabled = false
    }
  }

  synthBtn.addEventListener('click', async () => {
    const text = textInput.value.trim()
    const url  = urlInput.value.trim()

    if (!text && !url) {
      fetchStatus.textContent = 'Paste a URL first.'
      fetchStatus.className = 'fetch-status error'
      return
    }

    // If text area is empty, fetch the URL first
    const resolvedText = text || (await fetchDocument(url) ? textInput.value.trim() : '')
    if (!resolvedText) return

    if (bgSynthCb.checked) {
      await startBgJob(resolvedText, fetchedDocument?.current.id)
    } else {
      startSynth(resolvedText)
    }
  })

  urlInput.addEventListener('keydown', async (e: KeyboardEvent) => {
    if (e.key !== 'Enter') return
    const url = urlInput.value.trim()
    if (url) await fetchDocument(url)
  })
}

// ── Boot ──────────────────────────────────────────────────────────────────────

showReader()
