import './style.css'
import { marked } from 'marked'

// Configure marked to use the same CSS classes as the main reader so styles
// defined in style.css under .transcript-container apply correctly.
marked.use({
  renderer: {
    heading({ tokens, depth }) {
      const text = this.parser!.parseInline(tokens)
      return `<h${depth} class="md-heading">${text}</h${depth}>\n`
    },
    paragraph({ tokens }) {
      const text = this.parser!.parseInline(tokens)
      return `<p class="md-paragraph">${text}</p>\n`
    },
    blockquote({ tokens }) {
      const body = this.parser!.parse(tokens)
      return `<blockquote class="md-blockquote">${body}</blockquote>\n`
    },
    list({ items, ordered }) {
      const tag = ordered ? 'ol' : 'ul'
      const body = items.map(item =>
        `<li class="md-list-item">${this.parser!.parseInline(item.tokens)}</li>`
      ).join('\n')
      return `<${tag} class="md-list">${body}</${tag}>\n`
    },
    code({ text }) {
      return `<pre class="md-code"><code>${text}</code></pre>\n`
    },
  }
})

// ---------------------------------------------------------------------------
// Types — injected by CLI at export time via window.__ODORU__
// ---------------------------------------------------------------------------

interface ManifestEntry {
  title: string
  slug: string
  authors?: string[]
  description?: string
  date?: string
}

interface TranscriptEntry {
  index: number
  text: string
  start: number
  end: number
  paragraph_end: boolean
}

interface DocumentContent {
  content: string  // markdown
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
// Data — populated by CLI injection; empty stubs for Stage 1
// ---------------------------------------------------------------------------

const data: OdoruExport = window.__ODORU__ ?? { manifest: [], transcripts: {}, documents: {} }

// ---------------------------------------------------------------------------
// Render
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
        </div>
        <div id="transcript-container" class="transcript-container">
          <div class="loading" id="placeholder">
            ${data.manifest.length === 0 ? 'No published documents.' : 'Select a document.'}
          </div>
        </div>
        <div class="controls">
          <button class="play-btn" disabled><span class="play-icon">▶</span></button>
          <div class="progress-wrap">
            <div class="progress-bar"><div class="progress-fill"></div></div>
            <div class="time-row">
              <span class="time">0:00</span>
              <span class="time">0:00</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  `

  populateSidebar()
  wireTabSwitcher()
}

function populateSidebar() {
  const list = document.getElementById('article-list')!
  if (data.manifest.length === 0) {
    list.innerHTML = '<div class="outline-loading">No documents.</div>'
    return
  }
  list.innerHTML = data.manifest.map((entry, i) => `
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

  // Auto-load first document
  if (data.manifest.length > 0) {
    loadDocument(data.manifest[0].slug)
  }
}

function loadDocument(slug: string) {
  const entry = data.manifest.find(e => e.slug === slug)
  if (!entry) return

  // Header
  const header = document.getElementById('reader-header')!
  header.style.display = ''
  document.getElementById('article-title')!.textContent = entry.title
  const byline = document.getElementById('article-byline')!
  const bylineParts = [
    entry.authors?.join(', '),
    entry.date,
  ].filter(Boolean)
  byline.textContent = bylineParts.join(' · ')

  // Render markdown content if available; fall back to sentence spans
  const container = document.getElementById('transcript-container')!
  const doc = data.documents[slug]
  if (doc?.content) {
    container.innerHTML = marked.parse(doc.content) as string
    return
  }

  // Fallback: sentence spans (used in Stage 3 for audio sync)
  const transcript = data.transcripts[slug]
  if (!transcript || transcript.length === 0) {
    container.innerHTML = '<div class="loading">No content available.</div>'
    return
  }
  container.innerHTML = transcript.map(seg =>
    `<span class="segment pending" data-index="${seg.index}">${seg.text} </span>`
  ).join('')
}

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
