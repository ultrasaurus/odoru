import { Player } from './player'
import { renderMarkdown } from './markdown'
import { Document, type DocumentState } from './document'
import { ReaderCore, formatByline } from './reader-core'
import {
  type JobInfo,
  pickVoice, setError, setStatus, fmt,
  wireControls, controlsHtml, grabControlEls,
} from './ui'

export function mount(onEdit: () => void): () => void {
  const app = document.getElementById('app')!

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

  document.getElementById('new-btn')!.addEventListener('click', onEdit)

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
  const core = new ReaderCore(transcriptContainer, outlineList)

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

  // ── Job polling ────────────────────────────────────────────────────────────
  let pollTimer: ReturnType<typeof setTimeout> | null = null

  function stopPolling() {
    if (pollTimer !== null) { clearTimeout(pollTimer); pollTimer = null }
  }

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
    player.stop()
    stopPolling()
    jobArea.innerHTML = ''
    playBtn.disabled = true
    playBtn.querySelector('.play-icon')!.textContent = '▶'
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
        core.renderOutline(hs, i => {
          player.seekTo(i, false)
          const icon = playBtn.querySelector('.play-icon')
          if (icon) icon.textContent = '▶'
        })
        player.onTimeUpdate(t => core.updateOutlineActive(t, i => player.segmentStartTime(i)))

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

  return () => { stopPolling(); player.stop() }
}
