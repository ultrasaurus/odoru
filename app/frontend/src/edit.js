import { Player } from './player';
import { renderMarkdown } from './markdown';
import { Document } from './document';
import { ReaderCore } from './reader-core';
import { SECS_PER_WORD, pickVoice, wireControls, controlsHtml, grabControlEls, } from './ui';
export function mount(onReader) {
    const app = document.getElementById('app');
    let voices = [];
    let selectedVoice = null; // stores prefixed id, e.g. "f5:sarah"
    let synthStart = 0;
    let fetchedDocument = null;
    let currentPendingSpans = [];
    let currentHeadings = [];
    let loadSeq = 0;
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
            <div class="queue-header">
              Documents
              <button id="queue-collapse-btn" class="queue-collapse-btn">hide ready</button>
            </div>
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
              <div class="article-area">
                <div id="edit-outline-section" class="edit-outline-panel" style="display:none">
                  <div class="sidebar-label">Outline</div>
                  <div id="edit-outline-list" class="outline-list"></div>
                </div>
                <div id="article-content" class="article-content">
                  <div class="placeholder">Fetch a URL above to see the article.</div>
                </div>
              </div>
              <div class="synth-row">
                <div id="time-estimate" class="time-estimate"></div>
                <span id="synth-progress" class="synth-progress"></span>
                <div class="synth-buttons">
                  <button id="listen-btn" class="listen-btn" style="display:none">Listen</button>
                  <button id="reset-btn" class="reset-btn" style="display:none">New</button>
                  <button id="synth-btn" class="synth-btn">Synthesize</button>
                </div>
              </div>
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
  `;
    document.getElementById('back-link').addEventListener('click', onReader);
    const queueList = document.getElementById('queue-list');
    const collapseBtn = document.getElementById('queue-collapse-btn');
    let hideReady = false;
    collapseBtn.addEventListener('click', () => {
        hideReady = !hideReady;
        pollQueue();
    });
    // ── Documents panel ────────────────────────────────────────────────────────
    let queuePollTimer = null;
    let bgPollTimer = null;
    const openMetaForms = new Set(); // doc IDs with metadata form expanded
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
        const hasReadyVoice = (doc) => Object.values(doc.voices).some(v => !!v.duration);
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
                // Show voice name in meta line only while active — picker handles it when ready
                if (active)
                    displayVoiceName = voices.find(v => v.id === job.voice)?.name ?? job.voice;
            }
            else if (hasReadyVoice(doc)) {
                statusText = '✓ Ready';
                statusClass = 'done';
            }
            const row = document.createElement('div');
            row.className = 'queue-row';
            // Top line: title + status badge
            const top = document.createElement('div');
            top.className = 'queue-row-top';
            const titleEl = document.createElement('span');
            titleEl.textContent = doc.title ?? doc.source_url ?? doc.id;
            titleEl.className = 'queue-title queue-title-link';
            titleEl.addEventListener('click', () => loadAndListen(doc));
            top.appendChild(titleEl);
            if (statusText) {
                const statusEl = document.createElement('span');
                statusEl.className = `queue-status ${statusClass}`;
                statusEl.textContent = statusText;
                top.appendChild(statusEl);
            }
            // Delete button — far right of top row
            const deleteBtn = document.createElement('button');
            deleteBtn.className = 'queue-delete-btn';
            deleteBtn.textContent = '🗑';
            deleteBtn.title = 'Delete document';
            deleteBtn.addEventListener('click', () => {
                // Replace trash btn with inline confirm
                deleteBtn.replaceWith(confirmEl);
            });
            const confirmEl = document.createElement('span');
            confirmEl.className = 'queue-delete-confirm';
            const confirmLabel = document.createElement('span');
            confirmLabel.className = 'queue-delete-label';
            confirmLabel.textContent = 'Delete?';
            const confirmYes = document.createElement('button');
            confirmYes.className = 'queue-confirm-yes';
            confirmYes.textContent = '✓';
            confirmYes.addEventListener('click', async () => {
                row.remove();
                const res = await fetch(`/documents/${doc.id}`, { method: 'DELETE' });
                if (!res.ok) {
                    queueList.appendChild(row);
                    confirmEl.replaceWith(deleteBtn);
                }
                pollQueue();
            });
            const confirmNo = document.createElement('button');
            confirmNo.className = 'queue-confirm-no';
            confirmNo.textContent = '✗';
            confirmNo.addEventListener('click', () => {
                confirmEl.replaceWith(deleteBtn);
            });
            confirmEl.append(confirmLabel, confirmYes, confirmNo);
            top.appendChild(deleteBtn);
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
                .filter(([, v]) => !!v.duration);
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
                // Pencil button to toggle metadata edit form
                const editBtn = document.createElement('button');
                editBtn.className = 'queue-edit-btn';
                editBtn.title = 'Edit metadata';
                editBtn.textContent = '✎';
                pub.append(cb, label);
                if (readyVoices.length > 0)
                    pub.appendChild(select);
                pub.appendChild(editBtn);
                row.appendChild(pub);
                // Metadata edit form — hidden until pencil clicked
                const metaForm = document.createElement('div');
                metaForm.className = 'queue-meta-form';
                metaForm.style.display = 'none';
                const titleInput = document.createElement('input');
                titleInput.type = 'text';
                titleInput.className = 'queue-meta-input';
                titleInput.value = doc.title ?? '';
                const authorsInput = document.createElement('input');
                authorsInput.type = 'text';
                authorsInput.className = 'queue-meta-input';
                authorsInput.value = (doc.authors ?? []).join(', ');
                const dateInput = document.createElement('input');
                dateInput.type = 'date';
                dateInput.className = 'queue-meta-input';
                dateInput.value = doc.date ?? '';
                function makeMetaRow(labelText, input) {
                    const row = document.createElement('div');
                    row.className = 'queue-meta-row';
                    const lbl = document.createElement('label');
                    lbl.className = 'queue-meta-label';
                    lbl.textContent = labelText;
                    row.append(lbl, input);
                    return row;
                }
                const formActions = document.createElement('div');
                formActions.className = 'queue-meta-actions';
                const saveBtn = document.createElement('button');
                saveBtn.className = 'queue-meta-save';
                saveBtn.textContent = 'Save';
                const cancelBtn = document.createElement('button');
                cancelBtn.className = 'queue-meta-cancel';
                cancelBtn.textContent = 'Cancel';
                formActions.append(saveBtn, cancelBtn);
                metaForm.append(makeMetaRow('title:', titleInput), makeMetaRow('author(s):', authorsInput), makeMetaRow('date:', dateInput), formActions);
                row.appendChild(metaForm);
                // Restore open state if this form was open before a re-render
                if (openMetaForms.has(doc.id)) {
                    metaForm.style.display = '';
                    editBtn.classList.add('active');
                }
                editBtn.addEventListener('click', () => {
                    const open = metaForm.style.display !== 'none';
                    metaForm.style.display = open ? 'none' : '';
                    editBtn.classList.toggle('active', !open);
                    if (open)
                        openMetaForms.delete(doc.id);
                    else
                        openMetaForms.add(doc.id);
                });
                cancelBtn.addEventListener('click', () => {
                    metaForm.style.display = 'none';
                    editBtn.classList.remove('active');
                    openMetaForms.delete(doc.id);
                    if (openMetaForms.size === 0)
                        pollQueue();
                });
                saveBtn.addEventListener('click', async () => {
                    const authors = authorsInput.value.split(',').map(s => s.trim()).filter(Boolean);
                    await fetch(`/documents/${doc.id}`, {
                        method: 'PATCH',
                        headers: { 'Content-Type': 'application/json' },
                        body: JSON.stringify({
                            title: titleInput.value.trim() || undefined,
                            authors,
                            date: dateInput.value || undefined,
                        }),
                    });
                    metaForm.style.display = 'none';
                    editBtn.classList.remove('active');
                    openMetaForms.delete(doc.id);
                    // Update displayed title in this row immediately
                    titleEl.textContent = titleInput.value.trim() || (doc.source_url ?? doc.id);
                    pollQueue();
                });
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
            if (docsRes.ok && jobsRes.ok && openMetaForms.size === 0) {
                const allDocs = await docsRes.json();
                const jobs = await jobsRes.json();
                let docs = allDocs.filter(d => d.status !== 'error');
                if (hideReady) {
                    const activeDocIds = new Set(jobs
                        .filter(j => j.document_id && (j.status === 'in_progress' || j.status === 'pending'))
                        .map(j => j.document_id));
                    const hiddenCount = docs.filter(d => !activeDocIds.has(d.id)).length;
                    docs = docs.filter(d => activeDocIds.has(d.id));
                    collapseBtn.textContent = hiddenCount > 0 ? `show all (${hiddenCount} ready)` : 'show all';
                }
                else {
                    collapseBtn.textContent = 'hide ready';
                }
                renderQueue(docs, jobs);
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
    const listenBtn = document.getElementById('listen-btn');
    const newBtn = document.getElementById('reset-btn');
    const articleContent = document.getElementById('article-content');
    const synthProgress = document.getElementById('synth-progress');
    const timeEstimate = document.getElementById('time-estimate');
    const urlInput = document.getElementById('url-input');
    const fetchStatus = document.getElementById('fetch-status');
    const voiceList = document.getElementById('voice-list');
    const voiceDescription = document.getElementById('voice-description');
    const editOutlineSection = document.getElementById('edit-outline-section');
    const editOutlineList = document.getElementById('edit-outline-list');
    const { playBtn, downloadBtn, progressFill, timeCurrent, timeTotal } = grabControlEls();
    const player = new Player(articleContent);
    const editCore = new ReaderCore(articleContent, editOutlineList);
    player.onError(msg => {
        synthProgress.textContent = `Error: ${msg}`;
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
            if (voices.length > 0 && !selectedVoice) {
                const preferred = voices.find(v => v.id === 'kokoro:af_heart')
                    ?? voices.find(v => v.id.startsWith('kokoro:'))
                    ?? voices[0];
                selectVoice(preferred.id);
            }
            else
                renderVoices();
            updateEstimate(fetchedDocument?.current.plain_text ?? '');
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
    // Fetch a URL via Document.fetch (POST /documents + WS watch).
    async function fetchDocument(url) {
        fetchStatus.textContent = 'Fetching…';
        fetchStatus.className = 'fetch-status loading';
        urlInput.disabled = true;
        synthBtn.disabled = true;
        fetchedDocument?.destroy();
        fetchedDocument = null;
        currentPendingSpans = [];
        currentHeadings = [];
        articleContent.innerHTML = '<div class="placeholder">Fetch a URL above to see the article.</div>';
        try {
            const doc = await Document.fetch(url);
            const state = doc.current;
            const wasDedup = !state.cached_at || Date.now() - new Date(state.cached_at).getTime() > 5000;
            fetchedDocument = doc;
            articleContent.innerHTML = '';
            const { pendingSpans, headings } = renderMarkdown(state.content ?? '', state.plain_text ?? '', articleContent);
            currentPendingSpans = pendingSpans;
            currentHeadings = headings;
            editCore.renderOutline(headings, _i => { });
            editOutlineSection.style.display = headings.length ? '' : 'none';
            updateEstimate(state.plain_text ?? '');
            const suffix = wasDedup ? ' (previously fetched)' : '';
            fetchStatus.textContent = `✔ ${state.title ?? url}${suffix}`;
            fetchStatus.className = 'fetch-status success';
            return true;
        }
        catch (e) {
            fetchStatus.textContent = e?.message ?? 'Fetch failed';
            fetchStatus.className = 'fetch-status error';
            return false;
        }
        finally {
            urlInput.disabled = false;
            synthBtn.disabled = false;
        }
    }
    // ── Background job ─────────────────────────────────────────────────────────
    function showListenNew() {
        synthBtn.style.display = 'none';
        listenBtn.style.display = '';
        newBtn.style.display = '';
        synthBtn.disabled = false;
    }
    function pollBgJob(jobId, total) {
        stopBgPoll();
        bgPollTimer = setTimeout(async () => {
            try {
                const res = await fetch(`/jobs/${jobId}`);
                if (!res.ok) {
                    synthProgress.textContent = `Job not found (${res.status}) — server may have restarted`;
                    return;
                }
                const job = await res.json();
                if (job.status === 'done') {
                    synthProgress.textContent = '✓ Synthesis complete';
                    return;
                }
                if (job.status === 'error') {
                    synthProgress.textContent = `Synthesis error: ${job.error ?? ''}`;
                    return;
                }
                if (job.status === 'cancelled') {
                    synthProgress.textContent = 'Job cancelled.';
                    return;
                }
                const pct = total > 0 ? Math.round((job.completed_sentences / total) * 100) : 0;
                synthProgress.textContent =
                    `${job.completed_sentences}/${total} sentences (${pct}%)`;
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
        synthProgress.textContent = 'Queuing…';
        try {
            const body = { text };
            if (selectedVoice)
                body.voice = selectedVoice;
            if (documentId)
                body.document_id = documentId;
            const res = await fetch('/jobs', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
            });
            const job = await res.json();
            if (!res.ok) {
                synthProgress.textContent = job.error ?? 'Failed to queue job';
                synthBtn.disabled = false;
                return;
            }
            if (job.status === 'done') {
                synthProgress.textContent = '✓ Synthesis complete';
            }
            else {
                synthProgress.textContent = `0/${job.total_sentences} sentences (0%)`;
                pollBgJob(job.id, job.total_sentences);
            }
            showListenNew();
            pollQueue();
        }
        catch {
            synthProgress.textContent = 'Could not reach server';
            synthBtn.disabled = false;
        }
    }
    function startListen() {
        if (!fetchedDocument?.current)
            return;
        const doc = fetchedDocument.current;
        listenBtn.disabled = true;
        player.setPendingSpans(currentPendingSpans);
        editCore.renderOutline(currentHeadings, i => player.seekTo(i));
        synthStart = Date.now();
        player.synthesize(doc.plain_text, selectedVoice ?? undefined, currentPendingSpans, doc.id);
    }
    function resetEdit() {
        stopQueuePoll();
        stopBgPoll();
        player.stop();
        articleContent.innerHTML = '<div class="placeholder">Fetch a URL above to see the article.</div>';
        urlInput.value = '';
        fetchStatus.textContent = '';
        fetchStatus.className = 'fetch-status';
        synthProgress.textContent = '';
        timeEstimate.textContent = '';
        synthBtn.style.display = '';
        listenBtn.style.display = 'none';
        listenBtn.disabled = false;
        newBtn.style.display = 'none';
        editOutlineSection.style.display = 'none';
        playBtn.disabled = true;
        downloadBtn.disabled = true;
        progressFill.style.width = '0%';
        timeCurrent.textContent = '0:00';
        timeTotal.textContent = '0:00';
        fetchedDocument?.destroy();
        fetchedDocument = null;
        currentPendingSpans = [];
        currentHeadings = [];
        pollQueue();
    }
    async function loadAndListen(summary) {
        const seq = ++loadSeq;
        player.stop();
        playBtn.disabled = true;
        playBtn.querySelector('.play-icon').textContent = '▶';
        downloadBtn.disabled = true;
        progressFill.style.width = '0%';
        timeCurrent.textContent = '0:00';
        timeTotal.textContent = '0:00';
        synthProgress.textContent = '';
        synthBtn.style.display = 'none';
        listenBtn.style.display = 'none';
        newBtn.style.display = 'none';
        editOutlineSection.style.display = 'none';
        articleContent.innerHTML = '<div class="loading">Loading…</div>';
        fetchedDocument?.destroy();
        fetchedDocument = null;
        currentPendingSpans = [];
        currentHeadings = [];
        try {
            const loaded = await Document.load(summary.id);
            if (loadSeq !== seq) {
                loaded.destroy();
                return;
            }
            fetchedDocument = loaded;
            const data = fetchedDocument.current;
            if (!data.content || !data.plain_text) {
                articleContent.innerHTML = '<div class="error">Content not available.</div>';
                return;
            }
            articleContent.innerHTML = '';
            const { pendingSpans, headings } = renderMarkdown(data.content, data.plain_text, articleContent);
            currentPendingSpans = pendingSpans;
            currentHeadings = headings;
            editCore.renderOutline(headings, _i => { });
            editOutlineSection.style.display = headings.length ? '' : 'none';
            const voice = pickVoice(data.voices);
            const voiceEntry = voice ? data.voices[voice] : undefined;
            const audioReady = !!voiceEntry && voiceEntry.status !== 'error';
            if (!audioReady)
                updateEstimate(data.plain_text);
            else
                timeEstimate.textContent = '';
            showListenNew();
            startListen();
        }
        catch {
            articleContent.innerHTML = '<div class="error">Could not load document.</div>';
            fetchedDocument?.destroy();
            fetchedDocument = null;
        }
    }
    listenBtn.addEventListener('click', startListen);
    newBtn.addEventListener('click', resetEdit);
    synthBtn.addEventListener('click', async () => {
        const url = urlInput.value.trim();
        if (!fetchedDocument && !url) {
            fetchStatus.textContent = 'Paste a URL first.';
            fetchStatus.className = 'fetch-status error';
            return;
        }
        // Fetch if we don't have a document yet
        if (!fetchedDocument) {
            const ok = await fetchDocument(url);
            if (!ok)
                return;
        }
        const text = fetchedDocument?.current.plain_text;
        if (!text)
            return;
        await startBgJob(text, fetchedDocument?.current.id);
    });
    urlInput.addEventListener('keydown', async (e) => {
        if (e.key !== 'Enter')
            return;
        const url = urlInput.value.trim();
        if (url)
            await fetchDocument(url);
    });
    return () => { stopQueuePoll(); stopBgPoll(); };
}
