import { Player } from './player'
import { renderMarkdown } from './markdown'
import { Document, type DocumentState } from './document'
import { ReaderCore, formatByline } from './reader-core'
import {
  type JobInfo,
  pickVoice, setError, setStatus, fmt,
  wireControls, controlsHtml, grabControlEls,
} from './ui'
import { pollJob } from './jobs'

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
  // Unlike edit.ts (which reveals this only once synth starts), the reader
  // page has no separate "synthesize now" action — its own playBtn starts
  // disabled, so it's safe to always show the player shell.
  document.getElementById('player-controls')!.style.display = ''

  const player = new Player(transcriptContainer)
  const core = new ReaderCore(transcriptContainer, outlineList)

  autoscrollCb.checked = true
  player.autoScroll = true
  autoscrollCb.addEventListener('change', () => { player.autoScroll = autoscrollCb.checked })

  // ── Pronunciation popover ──────────────────────────────────────────────────
  const popover = document.createElement('div')
  popover.className = 'pronunciation-popover'
  popover.style.display = 'none'
  popover.innerHTML = `
    <label class="popover-word-label">Pronounce "<span class="popover-word"></span>" as:</label>
    <input type="text" class="popover-replacement" placeholder="phonetic spelling" />
    <div class="pronunciation-popover-buttons">
      <button class="cancel-btn">Cancel</button>
      <button class="save-btn">Save</button>
    </div>
  `
  document.body.appendChild(popover)

  const popoverWordEl      = popover.querySelector<HTMLElement>('.popover-word')!
  const popoverInput       = popover.querySelector<HTMLInputElement>('.popover-replacement')!
  const popoverSaveBtn     = popover.querySelector<HTMLButtonElement>('.save-btn')!
  const popoverCancelBtn   = popover.querySelector<HTMLButtonElement>('.cancel-btn')!
  let   popoverWord        = ''

  function showPopover(word: string, anchorRect: DOMRect) {
    popoverWord = word
    popoverWordEl.textContent = word
    popoverInput.value = ''
    popover.style.display = ''
    // Position below the selection, clamp to viewport
    const top  = Math.min(anchorRect.bottom + 8, window.innerHeight - 160)
    const left = Math.min(anchorRect.left, window.innerWidth - 280)
    popover.style.top  = `${top}px`
    popover.style.left = `${left}px`
    // No auto-focus here — focusing this input would immediately clear the
    // page's text selection, breaking plain copy of a single selected word.
    // The user can click into the input to type a pronunciation fix.
  }

  function hidePopover() {
    popover.style.display = 'none'
    popoverWord = ''
  }

  transcriptContainer.addEventListener('mouseup', () => {
    const sel = window.getSelection()
    const word = sel?.toString().trim() ?? ''
    if (!word || !sel || sel.rangeCount === 0 || word.includes(' ')) { hidePopover(); return }
    const rect = sel.getRangeAt(0).getBoundingClientRect()
    showPopover(word, rect)
  })

  async function saveOverride() {
    const replacement = popoverInput.value.trim()
    if (!replacement) { popoverInput.focus(); return }
    popoverSaveBtn.disabled = true
    popoverSaveBtn.textContent = 'Saving…'
    try {
      const res = await fetch('/overrides', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ word: popoverWord, replacement }),
      })
      if (!res.ok) throw new Error(`${res.status}`)
      hidePopover()
      window.getSelection()?.removeAllRanges()
      if (currentDoc) {
        pendingScrollRestore = transcriptContainer.scrollTop
        loadDocument(currentDoc)
      }
    } catch {
      popoverSaveBtn.textContent = 'Error — retry?'
      popoverSaveBtn.disabled = false
    }
  }

  popoverSaveBtn.addEventListener('click', saveOverride)
  popoverCancelBtn.addEventListener('click', () => { hidePopover(); window.getSelection()?.removeAllRanges() })

  popoverInput.addEventListener('keydown', e => {
    if (e.key === 'Enter') saveOverride()
    if (e.key === 'Escape') { hidePopover(); window.getSelection()?.removeAllRanges() }
  })

  document.addEventListener('mousedown', e => {
    if (popover.style.display !== 'none' && !popover.contains(e.target as Node)) {
      hidePopover()
    }
  })

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
  let pendingScrollRestore: number | null = null
  // Bumped on every loadDocument call; captured as `seq` at the start of
  // each call and checked after every async gap. If a newer call has since
  // started, this one's continuation is stale and bails rather than writing
  // into jobArea/transcriptContainer out from under the newer load — mirrors
  // edit.ts's loadSeq guard on its own load path.
  let loadSeq = 0
  wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal,
    () => (currentDoc?.title ?? currentDoc?.source_url ?? 'document').replace(/[^a-z0-9]+/gi, '-').toLowerCase() + '.wav')

  // ── Job polling ────────────────────────────────────────────────────────────
  let stopPolling = () => {}

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
      stopPolling = pollJob(job.id, job.total_sentences, {
        onProgress: (completed, total, pct) =>
          setStatus(jobArea, 'job-status running', `Synthesizing… ${completed}/${total} (${pct}%)`),
        onDone: () => setStatus(jobArea, 'job-status done', '✓ Audio ready'),
        onError: msg => setStatus(jobArea, 'job-status error', msg),
      })
    } catch {
      setStatus(jobArea, 'job-status error', 'Could not reach server')
    }
  }

  // ── Load document ──────────────────────────────────────────────────────────
  function loadDocument(doc: DocumentState) {
    const seq = ++loadSeq
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
    articleTitleEl.textContent = doc.title ?? doc.source_url ?? 'Untitled'
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
        if (loadSeq !== seq) return  // superseded by a newer loadDocument call
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
          // synthesize() just called reset(), which clears any known
          // duration — set it after, not before.
          player.setKnownDuration(voiceEntry!.duration ?? null)
        }

        // Always render transcript (used for reading even without audio).
        transcriptContainer.innerHTML = ''
        const { pendingSpans, headings: hs } = renderMarkdown(data.content, data.plain_text, transcriptContainer)
        if (pendingScrollRestore !== null) {
          transcriptContainer.scrollTop = pendingScrollRestore
          pendingScrollRestore = null
        }
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
          if (loadSeq !== seq) return  // superseded by a newer loadDocument call
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
              if (loadSeq !== seq) return  // superseded by a newer loadDocument call
              const active = jobs.find(j =>
                j.document_id === doc.id &&
                (j.status === 'pending' || j.status === 'in_progress')
              )
              if (active) {
                const pct = active.total_sentences > 0
                  ? Math.round((active.completed_sentences / active.total_sentences) * 100) : 0
                setStatus(jobArea, 'job-status running',
                  `Synthesizing… ${active.completed_sentences}/${active.total_sentences} (${pct}%)`)
                stopPolling = pollJob(active.id, active.total_sentences, {
                  onProgress: (completed, total, pct) =>
                    setStatus(jobArea, 'job-status running', `Synthesizing… ${completed}/${total} (${pct}%)`),
                  onDone: () => { setStatus(jobArea, 'job-status done', '✓ Audio ready'); synthesizeNow() },
                  onError: msg => setStatus(jobArea, 'job-status error', msg),
                })
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
        .sort((a, b) => (a.title ?? a.source_url ?? 'Untitled')
          .localeCompare(b.title ?? b.source_url ?? 'Untitled', undefined, { sensitivity: 'base' }))
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
        el.textContent = doc.title ?? doc.source_url ?? 'Untitled'
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

  return () => { stopPolling(); player.stop() }  // stopPolling always points to the latest poll
}
