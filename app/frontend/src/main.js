import './style.css';
import { Player } from './player';
// Approximate generation seconds per word for each backend.
// Kokoro: ~0.2 sec/word (measured: 143 words in 26s)
// F5:     ~3.0 sec/word (measured: 143 words in 410s)
const SECS_PER_WORD = {
    kokoro: 0.2,
    f5: 3.0,
};
const ARTICLE_URL = 'https://www.dougengelbart.org/content/view/148';
const ARTICLE_VOICE = 'f5:sarah';
const ARTICLES = [
    { title: 'Authorship Provisions in Augment', url: ARTICLE_URL, live: true },
    { title: 'As We May Think' },
    { title: 'A File Structure for the Complex, the Changing, and the Indeterminate' },
    { title: 'Augmenting Human Intellect' },
    { title: 'Intermedia: The Architecture and Construction of an Object-Oriented Hypermedia System and Applications Framework' },
    { title: "Hypertext '87 Keynote Address" },
    { title: 'Hypertext: An Introduction and Survey' },
];
const app = document.getElementById('app');
function splitSentences(text) {
    const result = [];
    const paragraphs = text.split(/\n\n+/).map(p => p.trim()).filter(Boolean);
    for (const para of paragraphs) {
        const sentences = [];
        for (const line of para.split('\n')) {
            const trimmed = line.trim();
            if (!trimmed)
                continue;
            if (typeof Intl !== 'undefined' && 'Segmenter' in Intl) {
                const seg = new Intl.Segmenter('en', { granularity: 'sentence' });
                for (const { segment } of seg.segment(trimmed)) {
                    const s = segment.trim();
                    if (s)
                        sentences.push(s);
                }
            }
            else {
                // Fallback for older browsers
                trimmed.split(/(?<=[.!?])\s+/).forEach(s => { if (s.trim())
                    sentences.push(s.trim()); });
            }
        }
        for (let i = 0; i < sentences.length; i++) {
            result.push({ text: sentences[i], paragraphEnd: i === sentences.length - 1 });
        }
    }
    return result;
}
// ── Shared helpers ────────────────────────────────────────────────────────────
function fmt(s) {
    const m = Math.floor(s / 60);
    const sec = Math.floor(s % 60);
    return `${m}:${sec.toString().padStart(2, '0')}`;
}
function wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal, filename) {
    const playIcon = playBtn.querySelector('.play-icon');
    player.onReady(() => {
        playBtn.disabled = false;
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
        downloadBtn.disabled = false;
    });
    playBtn.addEventListener('click', () => {
        player.toggle();
        playIcon.textContent = player.paused ? '▶' : '⏸';
    });
    downloadBtn.addEventListener('click', () => {
        player.downloadWav(filename);
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
    const listHtml = ARTICLES.map((a, i) => `
    <div class="article-item${i === 0 ? ' selected' : ''}${a.live ? '' : ' disabled'}" data-index="${i}">
      ${a.title}
    </div>
  `).join('');
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
          <div id="job-area" class="job-area"></div>
        </div>
        <div id="transcript-container" class="transcript-container">
          <div class="loading">Loading…</div>
        </div>
        ${controlsHtml()}
      </div>
    </div>
  `;
    document.getElementById('new-btn').addEventListener('click', showNew);
    const transcriptContainer = document.getElementById('transcript-container');
    const jobArea = document.getElementById('job-area');
    const { playBtn, downloadBtn, progressFill, timeCurrent, timeTotal } = grabControlEls();
    const player = new Player(transcriptContainer);
    player.onError(msg => {
        transcriptContainer.innerHTML = `<div class="error">Error: ${msg}</div>`;
        playBtn.disabled = true;
    });
    wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal, 'authorship-provisions-in-augment.wav');
    // ── Job polling ────────────────────────────────────────────────────────────
    let pollTimer = null;
    function stopPolling() {
        if (pollTimer !== null) {
            clearTimeout(pollTimer);
            pollTimer = null;
        }
    }
    function pollJob(jobId, total) {
        stopPolling();
        pollTimer = setTimeout(async () => {
            try {
                const res = await fetch(`/jobs/${jobId}`);
                if (!res.ok) {
                    jobArea.innerHTML = `<span class="job-status error">Job not found — server may have restarted</span>`;
                    return;
                }
                const job = await res.json();
                if (job.status === 'done') {
                    jobArea.innerHTML = '<span class="job-status done">✓ Audio ready</span>';
                    return;
                }
                if (job.status === 'error') {
                    jobArea.innerHTML = `<span class="job-status error">Synthesis error: ${job.error ?? ''}</span>`;
                    return;
                }
                const pct = total > 0 ? Math.round((job.completed_sentences / total) * 100) : 0;
                jobArea.innerHTML = `<span class="job-status running">Synthesizing… ${job.completed_sentences}/${total} (${pct}%)</span>`;
                pollJob(jobId, total);
            }
            catch {
                pollJob(jobId, total); // retry silently on network blip
            }
        }, 4000);
    }
    async function startJob(text) {
        jobArea.innerHTML = '<span class="job-status running">Queuing…</span>';
        try {
            const res = await fetch('/jobs', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ text, voice: ARTICLE_VOICE }),
            });
            const job = await res.json();
            if (!res.ok) {
                jobArea.innerHTML = `<span class="job-status error">${job.error ?? 'Failed to queue'}</span>`;
                return;
            }
            if (job.status === 'done') {
                jobArea.innerHTML = '<span class="job-status done">✓ Audio ready</span>';
                return;
            }
            pollJob(job.id, job.total_sentences);
        }
        catch {
            jobArea.innerHTML = '<span class="job-status error">Could not reach server</span>';
        }
    }
    // ── Doc fetch + pre-render ─────────────────────────────────────────────────
    fetch(`/doc?url=${encodeURIComponent(ARTICLE_URL)}&voice=${encodeURIComponent(ARTICLE_VOICE)}`)
        .then(res => res.json())
        .then(data => {
        const audioReady = !!data.cached?.audio;
        const isF5 = ARTICLE_VOICE.startsWith('f5:');
        if (audioReady) {
            jobArea.innerHTML = '<span class="job-status done">✓ Audio ready</span>';
        }
        else if (isF5) {
            const btn = document.createElement('button');
            btn.className = 'job-btn';
            btn.textContent = 'Synthesize in background';
            btn.addEventListener('click', () => {
                btn.remove();
                startJob(data.plain_text);
            });
            jobArea.appendChild(btn);
        }
        // Pre-render all sentences as gray pending spans so the article is
        // visible immediately; player activates each span as audio arrives.
        const sentences = splitSentences(data.plain_text);
        transcriptContainer.innerHTML = '';
        const pendingSpans = [];
        for (const { text, paragraphEnd } of sentences) {
            const span = document.createElement('span');
            span.className = 'segment pending';
            span.textContent = text;
            pendingSpans.push(span);
            transcriptContainer.appendChild(span);
            if (paragraphEnd) {
                const br = document.createElement('div');
                br.className = 'paragraph-break';
                transcriptContainer.appendChild(br);
            }
            else {
                transcriptContainer.appendChild(document.createTextNode(' '));
            }
        }
        player.synthesize(data.plain_text, ARTICLE_VOICE, pendingSpans);
    })
        .catch(() => {
        transcriptContainer.innerHTML = '<div class="error">Failed to load article.</div>';
        stopPolling();
    });
}
// ── New view ──────────────────────────────────────────────────────────────────
function showNew() {
    let voices = [];
    let selectedVoice = null; // stores prefixed id, e.g. "f5:sarah"
    let synthStart = 0;
    app.innerHTML = `
    <div class="layout">
      <div id="error-bar" class="error-bar" style="display:none">
        <span id="error-bar-msg" class="error-bar-msg"></span>
        <button id="error-bar-retry" class="error-bar-retry">Retry</button>
      </div>
      <header class="header">
        <a class="back-link" id="back-link">← Articles</a>
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
                placeholder="…or paste text here directly, then press Synthesize"
                rows="4"
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
              <div class="placeholder">Fetch a URL or enter text above, then press Synthesize.</div>
            </div>

            ${controlsHtml()}
          </div>

          <div id="queue-section" class="queue-section" style="display:none">
            <div class="queue-header">Background Queue</div>
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
    const queueSection = document.getElementById('queue-section');
    const queueList = document.getElementById('queue-list');
    // ── Background Queue ───────────────────────────────────────────────────────
    let queuePollTimer = null;
    function stopQueuePoll() {
        if (queuePollTimer !== null) {
            clearTimeout(queuePollTimer);
            queuePollTimer = null;
        }
    }
    function statusLabel(status) {
        return {
            pending: '⏳ Pending',
            in_progress: '⚙ Running',
            done: '✓ Done',
            error: '✕ Error',
            cancelled: '— Cancelled',
        }[status] ?? status;
    }
    function renderQueue(jobs) {
        if (jobs.length === 0) {
            queueSection.style.display = 'none';
            return;
        }
        queueSection.style.display = '';
        // Sort: running/pending first, then by created_at descending.
        jobs.sort((a, b) => {
            const activeA = a.status === 'in_progress' || a.status === 'pending';
            const activeB = b.status === 'in_progress' || b.status === 'pending';
            if (activeA !== activeB)
                return activeA ? -1 : 1;
            return b.created_at.localeCompare(a.created_at);
        });
        queueList.innerHTML = '';
        for (const job of jobs) {
            const active = job.status === 'pending' || job.status === 'in_progress';
            const pct = job.total_sentences > 0
                ? Math.round((job.completed_sentences / job.total_sentences) * 100) : 0;
            const progress = active
                ? `${job.completed_sentences}/${job.total_sentences} (${pct}%)`
                : job.status === 'done' ? `${job.total_sentences} sentences` : '';
            const row = document.createElement('div');
            row.className = 'queue-row';
            row.innerHTML = `
        <div class="queue-row-main">
          <span class="queue-voice">${job.voice}</span>
          <span class="queue-preview">${job.text_preview}</span>
        </div>
        <div class="queue-row-meta">
          <span class="queue-status ${job.status}">${statusLabel(job.status)}</span>
          ${progress ? `<span class="queue-progress">${progress}</span>` : ''}
          <span class="queue-date">${job.created_at}</span>
          ${active ? `<button class="queue-cancel-btn" data-id="${job.id}">✕</button>` : ''}
        </div>
      `;
            queueList.appendChild(row);
        }
        // Wire cancel buttons.
        queueList.querySelectorAll('.queue-cancel-btn').forEach(btn => {
            btn.addEventListener('click', async () => {
                const id = btn.dataset.id;
                await fetch(`/jobs/${id}`, { method: 'DELETE' });
                pollQueue();
            });
        });
    }
    async function pollQueue() {
        stopQueuePoll();
        try {
            const res = await fetch('/jobs');
            if (res.ok) {
                const jobs = await res.json();
                renderQueue(jobs);
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
        transcriptContainer.innerHTML = `<div class="error">Error: ${msg}</div>`;
        synthBtn.disabled = false;
        playBtn.disabled = true;
    });
    wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal, downloadFilename());
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
    // Fetch a URL into the textarea. Returns true on success.
    async function fetchUrl(url) {
        fetchStatus.textContent = 'Fetching…';
        fetchStatus.className = 'fetch-status loading';
        urlInput.disabled = true;
        synthBtn.disabled = true;
        try {
            const res = await fetch(`/doc?url=${encodeURIComponent(url)}`);
            const data = await res.json();
            if (!res.ok) {
                fetchStatus.textContent = data.error ?? 'Fetch failed';
                fetchStatus.className = 'fetch-status error';
                return false;
            }
            textInput.value = data.plain_text;
            updateEstimate(data.plain_text);
            const cached = data.cached?.content ? ' (cached)' : '';
            const title = data.title ?? url;
            fetchStatus.textContent = `✔ ${title}${cached}`;
            fetchStatus.className = 'fetch-status success';
            return true;
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
        player.synthesize(text, selectedVoice ?? undefined);
    }
    // ── Background job (polls until done, shows progress in transcript area) ──
    let bgPollTimer = null;
    function stopBgPoll() {
        if (bgPollTimer !== null) {
            clearTimeout(bgPollTimer);
            bgPollTimer = null;
        }
    }
    function pollBgJob(jobId, total) {
        stopBgPoll();
        bgPollTimer = setTimeout(async () => {
            try {
                const res = await fetch(`/jobs/${jobId}`);
                if (!res.ok) {
                    transcriptContainer.innerHTML = `<div class="error">Job not found (${res.status}) — server may have restarted</div>`;
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
                    transcriptContainer.innerHTML = `<div class="error">Synthesis error: ${job.error ?? ''}</div>`;
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
    async function startBgJob(text) {
        stopBgPoll();
        synthBtn.disabled = true;
        transcriptContainer.innerHTML = '<div class="loading">Queuing background job…</div>';
        try {
            const res = await fetch('/jobs', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ text, voice: selectedVoice }),
            });
            const job = await res.json();
            if (!res.ok) {
                transcriptContainer.innerHTML = `<div class="error">${job.error ?? 'Failed to queue job'}</div>`;
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
            fetchStatus.textContent = 'Paste a URL or enter text first.';
            fetchStatus.className = 'fetch-status error';
            return;
        }
        const resolvedText = text || (await fetchUrl(url) ? textInput.value.trim() : '');
        if (!resolvedText)
            return;
        if (bgSynthCb.checked) {
            await startBgJob(resolvedText);
        }
        else {
            startSynth(resolvedText);
        }
    });
    textInput.addEventListener('input', () => updateEstimate(textInput.value));
    textInput.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' && (e.ctrlKey || e.metaKey))
            synthBtn.click();
    });
    urlInput.addEventListener('keydown', async (e) => {
        if (e.key !== 'Enter')
            return;
        const url = urlInput.value.trim();
        if (url)
            await fetchUrl(url);
    });
}
// ── Boot ──────────────────────────────────────────────────────────────────────
showReader();
