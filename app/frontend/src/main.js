import './style.css';
import { Player } from './player';
import { renderMarkdown } from './markdown';
// Approximate generation seconds per word for each backend.
// Kokoro: ~0.2 sec/word (measured: 143 words in 26s)
// F5:     ~3.0 sec/word (measured: 143 words in 410s)
const SECS_PER_WORD = {
    kokoro: 0.2,
    f5: 3.0,
};
// Fallback voice used when a document has no synthesized voices (e.g. text-only
// published documents, or documents awaiting first synthesis).
const DEFAULT_VOICE = 'f5:sarah';
// Pick the best voice from a document's voices map.
// Priority: published → first ready → first stale → first any.
function pickVoice(voices) {
    for (const [id, v] of Object.entries(voices)) {
        if (v.published)
            return id;
    }
    for (const [id, v] of Object.entries(voices)) {
        if (v.status === 'ready')
            return id;
    }
    for (const [id, v] of Object.entries(voices)) {
        if (v.status === 'stale')
            return id;
    }
    const keys = Object.keys(voices);
    return keys.length > 0 ? keys[0] : null;
}
const app = document.getElementById('app');
// Module-level cleanup — stops any timers belonging to the current view
// before the next view replaces the DOM.
let viewCleanup = null;
// ── Shared helpers ────────────────────────────────────────────────────────────
// Safe alternative to innerHTML interpolation for single-element status messages.
function makeEl(tag, className, text) {
    const el = document.createElement(tag);
    el.className = className;
    el.textContent = text;
    return el;
}
function setError(container, msg) {
    container.innerHTML = '';
    container.appendChild(makeEl('div', 'error', msg));
}
function setStatus(container, className, msg) {
    container.innerHTML = '';
    container.appendChild(makeEl('span', className, msg));
}
function fmt(s) {
    const m = Math.floor(s / 60);
    const sec = Math.floor(s % 60);
    return `${m}:${sec.toString().padStart(2, '0')}`;
}
function wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal, getFilename) {
    const playIcon = playBtn.querySelector('.play-icon');
    player.onReady(() => {
        playBtn.disabled = false;
    });
    // Enable download as soon as all audio is received — no need to wait
    // until the end of playback.
    player.onSynthDone(() => {
        downloadBtn.disabled = false;
    });
    player.onTimeUpdate(t => {
        timeCurrent.textContent = fmt(t);
        const dur = player.duration;
        const pct = dur > 0 ? (t / dur) * 100 : 0;
        progressFill.style.width = `${Math.min(pct, 100)}%`;
        timeTotal.textContent = fmt(dur);
    });
    player.onEnded(() => {
        playIcon.textContent = '▶';
        progressFill.style.width = '100%';
    });
    playBtn.addEventListener('click', () => {
        player.toggle();
        playIcon.textContent = player.paused ? '▶' : '⏸';
    });
    downloadBtn.addEventListener('click', () => {
        player.downloadWav(getFilename());
    });
}
function controlsHtml() {
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
  `;
}
function grabControlEls() {
    return {
        playBtn: document.getElementById('play-btn'),
        downloadBtn: document.getElementById('download-btn'),
        progressFill: document.getElementById('progress-fill'),
        timeCurrent: document.getElementById('time-current'),
        timeTotal: document.getElementById('time-total'),
    };
}
// ── Reader view ───────────────────────────────────────────────────────────────
function showReader() {
    viewCleanup?.();
    app.innerHTML = `
    <div class="reader-layout">
      <nav class="article-sidebar">
        <div class="sidebar-top">
          <button class="new-btn" id="new-btn">New</button>
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
  `;
    document.getElementById('new-btn').addEventListener('click', showNew);
    // ── Sidebar tabs ───────────────────────────────────────────────────────────
    const tabArticles = document.getElementById('tab-articles');
    const tabOutline = document.getElementById('tab-outline');
    const articleList = document.getElementById('article-list');
    const outlineList = document.getElementById('outline-list');
    function showTab(tab) {
        const isArticles = tab === 'articles';
        tabArticles.classList.toggle('active', isArticles);
        tabOutline.classList.toggle('active', !isArticles);
        articleList.style.display = isArticles ? '' : 'none';
        outlineList.style.display = isArticles ? 'none' : '';
    }
    tabArticles.addEventListener('click', () => showTab('articles'));
    tabOutline.addEventListener('click', () => showTab('outline'));
    const articleTitleEl = document.getElementById('article-title');
    const transcriptContainer = document.getElementById('transcript-container');
    const jobArea = document.getElementById('job-area');
    const autoscrollCb = document.getElementById('autoscroll-cb');
    const { playBtn, downloadBtn, progressFill, timeCurrent, timeTotal } = grabControlEls();
    const seekStatus = document.getElementById('seek-status');
    const player = new Player(transcriptContainer);
    autoscrollCb.checked = true;
    player.autoScroll = true;
    autoscrollCb.addEventListener('change', () => { player.autoScroll = autoscrollCb.checked; });
    player.onError(msg => {
        setError(transcriptContainer, `Error: ${msg}`);
        playBtn.disabled = true;
    });
    player.onWaiting(() => {
        playBtn.disabled = true;
        seekStatus.style.display = '';
    });
    player.onSeekReady(() => {
        playBtn.disabled = false;
        seekStatus.style.display = 'none';
    });
    let currentDoc = null;
    wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal, () => (currentDoc?.title ?? currentDoc?.source_url ?? 'document').replace(/[^a-z0-9]+/gi, '-').toLowerCase() + '.wav');
    // ── Outline ────────────────────────────────────────────────────────────────
    let headings = [];
    let outlineEls = [];
    let activeOutlineIdx = -1;
    function renderOutline(hs) {
        headings = hs;
        outlineEls = [];
        activeOutlineIdx = -1;
        outlineList.innerHTML = '';
        if (hs.length === 0) {
            outlineList.innerHTML = '<div class="outline-loading">No headings</div>';
            return;
        }
        const minDepth = Math.min(...hs.map(h => h.depth));
        for (const h of hs) {
            const el = document.createElement('div');
            el.className = 'outline-item';
            el.dataset.depth = String(h.depth - minDepth);
            el.textContent = h.text;
            el.addEventListener('click', () => {
                h.element.scrollIntoView({ behavior: 'instant', block: 'start' });
                player.seekTo(h.sentenceIndex);
            });
            outlineList.appendChild(el);
            outlineEls.push(el);
        }
    }
    function updateOutlineActive(position) {
        let found = -1;
        for (let i = 0; i < headings.length; i++) {
            const t = player.segmentStartTime(headings[i].sentenceIndex);
            if (t !== null && t <= position)
                found = i;
            else if (t !== null)
                break;
        }
        if (found === activeOutlineIdx)
            return;
        if (activeOutlineIdx >= 0)
            outlineEls[activeOutlineIdx]?.classList.remove('active');
        activeOutlineIdx = found;
        if (found >= 0)
            outlineEls[found]?.classList.add('active');
    }
    // ── Job polling ────────────────────────────────────────────────────────────
    let pollTimer = null;
    function stopPolling() {
        if (pollTimer !== null) {
            clearTimeout(pollTimer);
            pollTimer = null;
        }
    }
    viewCleanup = stopPolling;
    function pollJob(jobId, total) {
        stopPolling();
        pollTimer = setTimeout(async () => {
            try {
                const res = await fetch(`/jobs/${jobId}`);
                if (!res.ok) {
                    setStatus(jobArea, 'job-status error', 'Job not found — server may have restarted');
                    return;
                }
                const job = await res.json();
                if (job.status === 'done') {
                    setStatus(jobArea, 'job-status done', '✓ Audio ready');
                    return;
                }
                if (job.status === 'error') {
                    setStatus(jobArea, 'job-status error', `Synthesis error: ${job.error ?? ''}`);
                    return;
                }
                const pct = total > 0 ? Math.round((job.completed_sentences / total) * 100) : 0;
                setStatus(jobArea, 'job-status running', `Synthesizing… ${job.completed_sentences}/${total} (${pct}%)`);
                pollJob(jobId, total);
            }
            catch {
                pollJob(jobId, total); // retry silently on network blip
            }
        }, 4000);
    }
    async function startJob(plainText, documentId, voice) {
        setStatus(jobArea, 'job-status running', 'Queuing…');
        try {
            const res = await fetch('/jobs', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ text: plainText, voice, document_id: documentId }),
            });
            const job = await res.json();
            if (!res.ok) {
                setStatus(jobArea, 'job-status error', job.error ?? 'Failed to queue');
                return;
            }
            if (job.status === 'done') {
                setStatus(jobArea, 'job-status done', '✓ Audio ready');
                return;
            }
            pollJob(job.id, job.total_sentences);
        }
        catch {
            setStatus(jobArea, 'job-status error', 'Could not reach server');
        }
    }
    // ── Load document ──────────────────────────────────────────────────────────
    function loadDocument(doc) {
        currentDoc = doc;
        stopPolling();
        jobArea.innerHTML = '';
        playBtn.disabled = true;
        downloadBtn.disabled = true;
        progressFill.style.width = '0%';
        timeCurrent.textContent = '0:00';
        timeTotal.textContent = '0:00';
        transcriptContainer.innerHTML = '<div class="loading">Loading…</div>';
        articleTitleEl.textContent = doc.title ?? doc.source_url ?? doc.id;
        fetch(`/documents/${doc.id}`)
            .then(res => res.json())
            .then((data) => {
            if (data.status === 'error') {
                setError(transcriptContainer, `Failed to load: ${data.error ?? 'unknown error'}`);
                return;
            }
            if (!data.content || !data.plain_text) {
                setError(transcriptContainer, 'Document content not yet available.');
                return;
            }
            const voice = pickVoice(data.voices) ?? DEFAULT_VOICE;
            const voiceEntry = data.voices[voice];
            const audioReady = voiceEntry?.status === 'ready' || voiceEntry?.status === 'stale';
            if (voiceEntry?.duration) {
                timeTotal.textContent = fmt(voiceEntry.duration);
            }
            if (audioReady) {
                setStatus(jobArea, 'job-status done', '✓ Audio ready');
            }
            else {
                // Check for an existing active job before showing the button.
                fetch('/jobs')
                    .then(res => res.ok ? res.json() : [])
                    .then((jobs) => {
                    const active = jobs.find(j => j.document_id === doc.id &&
                        (j.status === 'pending' || j.status === 'in_progress'));
                    if (active) {
                        const pct = active.total_sentences > 0
                            ? Math.round((active.completed_sentences / active.total_sentences) * 100) : 0;
                        setStatus(jobArea, 'job-status running', `Synthesizing… ${active.completed_sentences}/${active.total_sentences} (${pct}%)`);
                        pollJob(active.id, active.total_sentences);
                    }
                    else {
                        const btn = document.createElement('button');
                        btn.className = 'job-btn';
                        btn.textContent = 'Synthesize in background';
                        btn.addEventListener('click', () => {
                            btn.remove();
                            startJob(data.plain_text, doc.id, voice);
                        });
                        jobArea.appendChild(btn);
                    }
                })
                    .catch(() => {
                    const btn = document.createElement('button');
                    btn.className = 'job-btn';
                    btn.textContent = 'Synthesize in background';
                    btn.addEventListener('click', () => {
                        btn.remove();
                        startJob(data.plain_text, doc.id, voice);
                    });
                    jobArea.appendChild(btn);
                });
            }
            transcriptContainer.innerHTML = '';
            const { pendingSpans, headings: hs } = renderMarkdown(data.content, data.plain_text, transcriptContainer);
            renderOutline(hs);
            player.synthesize(data.plain_text, voice, pendingSpans, doc.id);
            // Drive active outline heading from playback position.
            player.onTimeUpdate(t => updateOutlineActive(t));
        })
            .catch(() => {
            setError(transcriptContainer, 'Failed to load document.');
            stopPolling();
        });
    }
    // ── Fetch document list + load first ──────────────────────────────────────
    fetch('/documents')
        .then(res => res.json())
        .then((all) => {
        const docs = all.filter(d => d.publish);
        articleList.innerHTML = '';
        if (docs.length === 0) {
            articleList.innerHTML = '<div class="outline-loading">No documents.</div>';
            transcriptContainer.innerHTML = '<div class="loading">No documents found.</div>';
            articleTitleEl.textContent = '';
            return;
        }
        const itemEls = [];
        docs.forEach((doc, i) => {
            const el = document.createElement('div');
            el.className = 'article-item' + (i === 0 ? ' selected' : '');
            el.textContent = doc.title ?? doc.source_url ?? doc.id;
            el.addEventListener('click', () => {
                itemEls.forEach(e => e.classList.remove('selected'));
                el.classList.add('selected');
                loadDocument(doc);
            });
            articleList.appendChild(el);
            itemEls.push(el);
        });
        loadDocument(docs[0]);
    })
        .catch(() => {
        articleList.innerHTML = '<div class="outline-loading">Failed to load documents.</div>';
        setError(transcriptContainer, 'Failed to load document list.');
        articleTitleEl.textContent = '';
    });
}
// ── New view ──────────────────────────────────────────────────────────────────
function showNew() {
    viewCleanup?.();
    let voices = [];
    let selectedVoice = null; // stores prefixed id, e.g. "f5:sarah"
    let synthStart = 0;
    let fetchedDocumentId = null;
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

          <div id="queue-section" class="queue-section">
            <div class="queue-header">Documents</div>
            <div id="queue-list" class="queue-list"></div>
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
  `;
    document.getElementById('back-link').addEventListener('click', showReader);
    const queueList = document.getElementById('queue-list');
    // ── Documents panel ────────────────────────────────────────────────────────
    let queuePollTimer = null;
    let bgPollTimer = null;
    function stopQueuePoll() {
        if (queuePollTimer !== null) {
            clearTimeout(queuePollTimer);
            queuePollTimer = null;
        }
    }
    function stopBgPoll() {
        if (bgPollTimer !== null) {
            clearTimeout(bgPollTimer);
            bgPollTimer = null;
        }
    }
    viewCleanup = () => { stopQueuePoll(); stopBgPoll(); };
    function jobStatusLabel(status) {
        return {
            pending: '⏳ Pending',
            in_progress: '⚙ Running',
            done: '✓ Ready',
            error: '✕ Error',
            cancelled: '— Cancelled',
        }[status] ?? status;
    }
    function renderQueue(docs, jobs) {
        queueList.innerHTML = '';
        if (docs.length === 0) {
            const empty = document.createElement('div');
            empty.className = 'queue-empty';
            empty.textContent = 'No documents yet.';
            queueList.appendChild(empty);
            return;
        }
        // Build document_id → best job map (prefer active > done > others, then newest)
        const jobMap = new Map();
        for (const job of jobs) {
            if (!job.document_id)
                continue;
            const existing = jobMap.get(job.document_id);
            if (!existing) {
                jobMap.set(job.document_id, job);
                continue;
            }
            const rank = (s) => s === 'in_progress' ? 0 : s === 'pending' ? 1 : s === 'done' ? 2 : 3;
            const better = rank(job.status) < rank(existing.status) ||
                (rank(job.status) === rank(existing.status) && job.created_at > existing.created_at);
            if (better)
                jobMap.set(job.document_id, job);
        }
        // Check if any voice is ready in a document's voices map
        const hasReadyVoice = (doc) => Object.values(doc.voices).some(v => v.status === 'ready' || v.status === 'stale');
        // Assign sort rank
        const sortRank = (doc) => {
            const job = jobMap.get(doc.id);
            if (job?.status === 'in_progress')
                return 0;
            if (job?.status === 'pending')
                return 1;
            if (job?.status === 'done')
                return 2;
            if (hasReadyVoice(doc))
                return 3;
            return 4;
        };
        const sorted = [...docs].sort((a, b) => {
            const dr = sortRank(a) - sortRank(b);
            if (dr !== 0)
                return dr;
            return (b.cached_at ?? '').localeCompare(a.cached_at ?? '');
        });
        for (const doc of sorted) {
            const job = jobMap.get(doc.id);
            const active = job?.status === 'pending' || job?.status === 'in_progress';
            const pct = job && job.total_sentences > 0
                ? Math.round((job.completed_sentences / job.total_sentences) * 100) : 0;
            // Determine status label + voice name for display
            let statusText = '';
            let statusClass = '';
            let displayVoiceName = '';
            if (job) {
                statusText = jobStatusLabel(job.status);
                statusClass = job.status;
                displayVoiceName = voices.find(v => v.id === job.voice)?.name ?? job.voice;
            }
            else if (hasReadyVoice(doc)) {
                const readyVoiceId = pickVoice(doc.voices);
                statusText = '✓ Ready';
                statusClass = 'done';
                displayVoiceName = readyVoiceId
                    ? (voices.find(v => v.id === readyVoiceId)?.name ?? readyVoiceId)
                    : '';
            }
            const row = document.createElement('div');
            row.className = 'queue-row';
            // Top line: title + status badge
            const top = document.createElement('div');
            top.className = 'queue-row-top';
            const titleEl = document.createElement('span');
            titleEl.className = 'queue-title';
            titleEl.textContent = doc.title ?? doc.source_url ?? doc.id;
            top.appendChild(titleEl);
            if (statusText) {
                const statusEl = document.createElement('span');
                statusEl.className = `queue-status ${statusClass}`;
                statusEl.textContent = statusText;
                top.appendChild(statusEl);
            }
            row.appendChild(top);
            // Bottom line: voice + progress (only if there's something to show)
            if (displayVoiceName || active) {
                const meta = document.createElement('div');
                meta.className = 'queue-row-meta';
                if (displayVoiceName) {
                    const voiceEl = document.createElement('span');
                    voiceEl.className = 'queue-voice';
                    voiceEl.textContent = displayVoiceName;
                    meta.appendChild(voiceEl);
                }
                if (active && job) {
                    const bar = document.createElement('div');
                    bar.className = 'queue-progress-bar';
                    const fill = document.createElement('div');
                    fill.className = 'queue-progress-fill';
                    fill.style.width = `${pct}%`;
                    bar.appendChild(fill);
                    meta.appendChild(bar);
                    const pctEl = document.createElement('span');
                    pctEl.className = 'queue-progress';
                    pctEl.textContent = `${pct}%`;
                    meta.appendChild(pctEl);
                    const cancelBtn = document.createElement('button');
                    cancelBtn.className = 'queue-cancel-btn';
                    cancelBtn.textContent = '✕';
                    cancelBtn.addEventListener('click', async () => {
                        await fetch(`/jobs/${job.id}`, { method: 'DELETE' });
                        pollQueue();
                    });
                    meta.appendChild(cancelBtn);
                }
                else if (job?.status === 'done') {
                    const countEl = document.createElement('span');
                    countEl.className = 'queue-progress';
                    countEl.textContent = `${job.total_sentences} sentences`;
                    meta.appendChild(countEl);
                }
                row.appendChild(meta);
            }
            // Publish controls — shown for any fetched document (status: ready)
            // Voice picker shown alongside only when ready/stale voices exist
            const readyVoices = Object.entries(doc.voices)
                .filter(([, v]) => v.status === 'ready' || v.status === 'stale');
            if (doc.status === 'ready') {
                const pub = document.createElement('div');
                pub.className = 'queue-row-publish';
                const cb = document.createElement('input');
                cb.type = 'checkbox';
                cb.className = 'queue-publish-cb';
                cb.checked = doc.publish;
                cb.id = `pub-${doc.id}`;
                const label = document.createElement('label');
                label.htmlFor = cb.id;
                label.className = 'queue-publish-label';
                label.textContent = 'Publish';
                const select = document.createElement('select');
                select.className = 'queue-voice-select';
                for (const [vid, ve] of readyVoices) {
                    const opt = document.createElement('option');
                    opt.value = vid;
                    opt.textContent = voices.find(v => v.id === vid)?.name ?? vid;
                    opt.selected = !!ve.published;
                    select.appendChild(opt);
                }
                const patch = async () => {
                    await fetch(`/documents/${doc.id}`, {
                        method: 'PATCH',
                        headers: { 'Content-Type': 'application/json' },
                        body: JSON.stringify({ publish: cb.checked, published_voice: select.value || undefined }),
                    });
                };
                cb.addEventListener('change', patch);
                if (readyVoices.length > 0)
                    select.addEventListener('change', patch);
                pub.append(cb, label);
                if (readyVoices.length > 0)
                    pub.appendChild(select);
                row.appendChild(pub);
            }
            queueList.appendChild(row);
        }
    }
    async function pollQueue() {
        stopQueuePoll();
        try {
            const [docsRes, jobsRes] = await Promise.all([
                fetch('/documents'),
                fetch('/jobs'),
            ]);
            if (docsRes.ok && jobsRes.ok) {
                renderQueue(await docsRes.json(), await jobsRes.json());
            }
        }
        catch { /* silent */ }
        queuePollTimer = setTimeout(pollQueue, 10_000);
    }
    pollQueue();
    // Error bar helpers
    const errorBar = document.getElementById('error-bar');
    const errorBarMsg = document.getElementById('error-bar-msg');
    const errorBarRetry = document.getElementById('error-bar-retry');
    function showErrorBar(msg) {
        errorBarMsg.textContent = msg;
        errorBar.style.display = 'flex';
    }
    function hideErrorBar() {
        errorBar.style.display = 'none';
    }
    errorBarRetry.addEventListener('click', () => loadVoices());
    const synthBtn = document.getElementById('synth-btn');
    const bgSynthCb = document.getElementById('bg-synth-cb');
    const textInput = document.getElementById('text-input');
    const timeEstimate = document.getElementById('time-estimate');
    const urlInput = document.getElementById('url-input');
    const fetchStatus = document.getElementById('fetch-status');
    const voiceList = document.getElementById('voice-list');
    const voiceDescription = document.getElementById('voice-description');
    const transcriptContainer = document.getElementById('transcript-container');
    const { playBtn, downloadBtn, progressFill, timeCurrent, timeTotal } = grabControlEls();
    const player = new Player(transcriptContainer);
    player.onError(msg => {
        setError(transcriptContainer, `Error: ${msg}`);
        synthBtn.disabled = false;
        playBtn.disabled = true;
    });
    // downloadFilename is passed as a function so it's evaluated at click time.
    wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal, downloadFilename);
    player.onEnded(() => {
        synthBtn.disabled = false;
        if (synthStart > 0) {
            const elapsed = ((Date.now() - synthStart) / 1000).toFixed(0);
            const words = player.synthesizedWordCount;
            timeEstimate.textContent = `Synthesized ${words} words in ${elapsed}s`;
            synthStart = 0;
        }
    });
    // Voice picker
    function renderVoices() {
        if (voices.length === 0) {
            voiceList.innerHTML = '<div class="voice-loading">No voices available.</div>';
            return;
        }
        voiceList.innerHTML = '';
        let lastBackend = '';
        for (const v of voices) {
            if (v.backend !== lastBackend) {
                const hdr = document.createElement('div');
                hdr.className = 'voice-group-header';
                hdr.textContent = v.backend.toUpperCase();
                voiceList.appendChild(hdr);
                lastBackend = v.backend;
            }
            const row = document.createElement('button');
            row.className = 'voice-row' + (v.id === selectedVoice ? ' selected' : '');
            row.textContent = v.name;
            row.addEventListener('click', () => selectVoice(v.id));
            voiceList.appendChild(row);
        }
    }
    function selectVoice(id) {
        selectedVoice = id;
        const v = voices.find(v => v.id === id);
        voiceDescription.textContent = v?.description ?? '';
        renderVoices();
    }
    async function loadVoices() {
        try {
            const res = await fetch('/voices');
            if (!res.ok)
                throw new Error(`HTTP ${res.status}`);
            const data = await res.json();
            voices = data.voices;
            hideErrorBar();
            if (voices.length > 0 && !selectedVoice)
                selectVoice(voices[0].id);
            else
                renderVoices();
            updateEstimate(textInput.value);
        }
        catch {
            voiceList.innerHTML = '<div class="voice-loading">—</div>';
            showErrorBar('Could not reach server. Is it running?');
        }
    }
    loadVoices();
    // Time estimate
    function fmtDuration(secs) {
        if (secs < 60)
            return `~${Math.round(secs)}s`;
        const m = Math.floor(secs / 60);
        const s = Math.round(secs % 60);
        return s > 0 ? `~${m}m ${s}s` : `~${m}m`;
    }
    function updateEstimate(text) {
        const words = text.trim().split(/\s+/).filter(Boolean).length;
        if (words === 0) {
            timeEstimate.textContent = '';
            return;
        }
        const backend = selectedVoice?.split(':')[0] ?? 'kokoro';
        const rate = SECS_PER_WORD[backend] ?? 0.2;
        const secs = words * rate;
        timeEstimate.textContent = `${fmtDuration(secs)} to synthesize (${words} words)`;
    }
    function downloadFilename() {
        const url = urlInput.value.trim();
        if (!url)
            return 'odoru.wav';
        try {
            const u = new URL(url);
            const slug = (u.hostname + u.pathname)
                .replace(/[^a-z0-9]+/gi, '-')
                .replace(/^-+|-+$/g, '')
                .toLowerCase();
            return `${slug}.wav`;
        }
        catch {
            return 'odoru.wav';
        }
    }
    // Fetch a URL via POST /documents, poll until ready, populate textarea.
    async function fetchDocument(url) {
        fetchStatus.textContent = 'Fetching…';
        fetchStatus.className = 'fetch-status loading';
        urlInput.disabled = true;
        synthBtn.disabled = true;
        fetchedDocumentId = null;
        textInput.value = '';
        try {
            // Create or retrieve document
            const createRes = await fetch('/documents', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ url }),
            });
            if (!createRes.ok) {
                const err = await createRes.json().catch(() => ({}));
                fetchStatus.textContent = err.error ?? 'Fetch failed';
                fetchStatus.className = 'fetch-status error';
                return false;
            }
            const { id } = await createRes.json();
            // Poll until ready
            while (true) {
                const pollRes = await fetch(`/documents/${id}`);
                if (!pollRes.ok) {
                    fetchStatus.textContent = 'Fetch failed';
                    fetchStatus.className = 'fetch-status error';
                    return false;
                }
                const doc = await pollRes.json();
                if (doc.status === 'error') {
                    fetchStatus.textContent = doc.error ?? 'Fetch failed';
                    fetchStatus.className = 'fetch-status error';
                    return false;
                }
                if (doc.status === 'ready' && doc.plain_text) {
                    textInput.value = doc.plain_text;
                    updateEstimate(doc.plain_text);
                    fetchedDocumentId = id;
                    const cached = doc.cached_at ? ' (cached)' : '';
                    fetchStatus.textContent = `✔ ${doc.title ?? url}${cached}`;
                    fetchStatus.className = 'fetch-status success';
                    return true;
                }
                // Still fetching — wait 2 seconds and poll again
                await new Promise(r => setTimeout(r, 2000));
            }
        }
        catch {
            fetchStatus.textContent = 'Network error';
            fetchStatus.className = 'fetch-status error';
            return false;
        }
        finally {
            urlInput.disabled = false;
            synthBtn.disabled = false;
        }
    }
    function startSynth(text) {
        synthBtn.disabled = true;
        playBtn.disabled = true;
        downloadBtn.disabled = true;
        progressFill.style.width = '0%';
        timeCurrent.textContent = '0:00';
        timeTotal.textContent = '0:00';
        synthStart = Date.now();
        player.synthesize(text, selectedVoice ?? undefined, undefined, fetchedDocumentId ?? undefined);
    }
    // ── Background job ─────────────────────────────────────────────────────────
    function pollBgJob(jobId, total) {
        stopBgPoll();
        bgPollTimer = setTimeout(async () => {
            try {
                const res = await fetch(`/jobs/${jobId}`);
                if (!res.ok) {
                    setError(transcriptContainer, `Job not found (${res.status}) — server may have restarted`);
                    synthBtn.disabled = false;
                    return;
                }
                const job = await res.json();
                if (job.status === 'done') {
                    transcriptContainer.innerHTML = '<div class="loading">✓ Background synthesis complete — press Synthesize to play</div>';
                    synthBtn.disabled = false;
                    return;
                }
                if (job.status === 'error') {
                    setError(transcriptContainer, `Synthesis error: ${job.error ?? ''}`);
                    synthBtn.disabled = false;
                    return;
                }
                if (job.status === 'cancelled') {
                    transcriptContainer.innerHTML = '<div class="loading">Job cancelled.</div>';
                    synthBtn.disabled = false;
                    return;
                }
                const pct = total > 0 ? Math.round((job.completed_sentences / total) * 100) : 0;
                transcriptContainer.innerHTML =
                    `<div class="loading">Background synthesis: ${job.completed_sentences}/${total} sentences (${pct}%)</div>`;
                pollBgJob(jobId, total);
            }
            catch {
                pollBgJob(jobId, total); // retry silently on network blip
            }
        }, 4000);
    }
    async function startBgJob(text, documentId) {
        stopBgPoll();
        synthBtn.disabled = true;
        transcriptContainer.innerHTML = '<div class="loading">Queuing background job…</div>';
        try {
            const body = { text, voice: selectedVoice ?? DEFAULT_VOICE };
            if (documentId)
                body.document_id = documentId;
            const res = await fetch('/jobs', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
            });
            const job = await res.json();
            if (!res.ok) {
                setError(transcriptContainer, job.error ?? 'Failed to queue job');
                synthBtn.disabled = false;
                return;
            }
            if (job.status === 'done') {
                transcriptContainer.innerHTML = '<div class="loading">✓ Already synthesized — press Synthesize to play</div>';
                synthBtn.disabled = false;
                return;
            }
            transcriptContainer.innerHTML =
                `<div class="loading">Background synthesis: 0/${job.total_sentences} sentences (0%)</div>`;
            pollBgJob(job.id, job.total_sentences);
            pollQueue();
        }
        catch {
            transcriptContainer.innerHTML = '<div class="error">Could not reach server</div>';
            synthBtn.disabled = false;
        }
    }
    synthBtn.addEventListener('click', async () => {
        const text = textInput.value.trim();
        const url = urlInput.value.trim();
        if (!text && !url) {
            fetchStatus.textContent = 'Paste a URL first.';
            fetchStatus.className = 'fetch-status error';
            return;
        }
        // If text area is empty, fetch the URL first
        const resolvedText = text || (await fetchDocument(url) ? textInput.value.trim() : '');
        if (!resolvedText)
            return;
        if (bgSynthCb.checked) {
            await startBgJob(resolvedText, fetchedDocumentId ?? undefined);
        }
        else {
            startSynth(resolvedText);
        }
    });
    urlInput.addEventListener('keydown', async (e) => {
        if (e.key !== 'Enter')
            return;
        const url = urlInput.value.trim();
        if (url)
            await fetchDocument(url);
    });
}
// ── Boot ──────────────────────────────────────────────────────────────────────
showReader();
