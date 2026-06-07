import './style.css';
// ---------------------------------------------------------------------------
// Data — populated by CLI injection; empty stubs for Stage 1
// ---------------------------------------------------------------------------
const data = window.__ODORU__ ?? { manifest: [], transcripts: {} };
// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------
function render() {
    const app = document.getElementById('app');
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
  `;
    populateSidebar();
    wireTabSwitcher();
}
function populateSidebar() {
    const list = document.getElementById('article-list');
    if (data.manifest.length === 0) {
        list.innerHTML = '<div class="outline-loading">No documents.</div>';
        return;
    }
    list.innerHTML = data.manifest.map((entry, i) => `
    <div class="article-item live${i === 0 ? ' selected' : ''}"
         data-slug="${entry.slug}"
         title="${entry.description ?? ''}">
      ${entry.title}
    </div>
  `).join('');
    list.querySelectorAll('.article-item').forEach(el => {
        el.addEventListener('click', () => {
            list.querySelectorAll('.article-item').forEach(x => x.classList.remove('selected'));
            el.classList.add('selected');
            loadDocument(el.dataset.slug);
        });
    });
    // Auto-load first document
    if (data.manifest.length > 0) {
        loadDocument(data.manifest[0].slug);
    }
}
function loadDocument(slug) {
    const entry = data.manifest.find(e => e.slug === slug);
    if (!entry)
        return;
    // Header
    const header = document.getElementById('reader-header');
    header.style.display = '';
    document.getElementById('article-title').textContent = entry.title;
    const byline = document.getElementById('article-byline');
    byline.textContent = [entry.date].filter(Boolean).join(' · ');
    // Transcript — Stage 2 will render sentence spans; for now render plain text
    const container = document.getElementById('transcript-container');
    const transcript = data.transcripts[slug];
    if (!transcript || transcript.length === 0) {
        container.innerHTML = '<div class="loading">No transcript available.</div>';
        return;
    }
    container.innerHTML = transcript.map(seg => `<span class="segment pending" data-index="${seg.index}">${seg.text} </span>`).join('');
}
function wireTabSwitcher() {
    const tabDocs = document.getElementById('tab-documents');
    const tabOutline = document.getElementById('tab-outline');
    const articleList = document.getElementById('article-list');
    const outlineList = document.getElementById('outline-list');
    tabDocs.addEventListener('click', () => {
        tabDocs.classList.add('active');
        tabOutline.classList.remove('active');
        articleList.style.display = '';
        outlineList.style.display = 'none';
    });
    tabOutline.addEventListener('click', () => {
        tabOutline.classList.add('active');
        tabDocs.classList.remove('active');
        outlineList.style.display = '';
        articleList.style.display = 'none';
    });
}
render();
