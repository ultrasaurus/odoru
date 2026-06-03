import './style.css';
import { Player } from './player';
const app = document.getElementById('app');
app.innerHTML = `
  <div class="layout">
    <header class="header">
      <div class="logo">▶ odoru</div>
    </header>

    <main class="main">
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
          <button id="synth-btn" class="synth-btn">Synthesize</button>
        </div>

        <div id="transcript-container" class="transcript-container">
          <div class="placeholder">Fetch a URL or enter text above, then press Synthesize.</div>
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
          <button id="download-btn" class="download-btn" disabled title="Download WAV">↓</button>
        </div>
      </div>
    </main>
  </div>
`;
const synthBtn = document.getElementById('synth-btn');
const textInput = document.getElementById('text-input');
const urlInput = document.getElementById('url-input');
const fetchStatus = document.getElementById('fetch-status');
const playBtn = document.getElementById('play-btn');
const playIcon = playBtn.querySelector('.play-icon');
const downloadBtn = document.getElementById('download-btn');
const progressFill = document.getElementById('progress-fill');
const timeCurrent = document.getElementById('time-current');
const timeTotal = document.getElementById('time-total');
const transcriptContainer = document.getElementById('transcript-container');
// Derive a download filename from the current URL input or a default
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
function fmt(s) {
    const m = Math.floor(s / 60);
    const sec = Math.floor(s % 60);
    return `${m}:${sec.toString().padStart(2, '0')}`;
}
const player = new Player(transcriptContainer);
player.onReady(() => {
    playBtn.disabled = false;
    playIcon.textContent = '▶';
    player.play();
    playIcon.textContent = '⏸';
});
player.onError(msg => {
    transcriptContainer.innerHTML = `<div class="error">Error: ${msg}</div>`;
    synthBtn.disabled = false;
    playBtn.disabled = true;
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
    synthBtn.disabled = false;
    downloadBtn.disabled = false;
});
synthBtn.addEventListener('click', () => {
    const text = textInput.value.trim();
    if (!text)
        return;
    synthBtn.disabled = true;
    playBtn.disabled = true;
    downloadBtn.disabled = true;
    playIcon.textContent = '▶';
    progressFill.style.width = '0%';
    timeCurrent.textContent = '0:00';
    timeTotal.textContent = '0:00';
    player.synthesize(text);
});
playBtn.addEventListener('click', () => {
    player.toggle();
    playIcon.textContent = player.paused ? '▶' : '⏸';
});
downloadBtn.addEventListener('click', () => {
    player.downloadWav(downloadFilename());
});
textInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        synthBtn.click();
    }
});
// URL fetch on Enter
urlInput.addEventListener('keydown', async (e) => {
    if (e.key !== 'Enter')
        return;
    const url = urlInput.value.trim();
    if (!url)
        return;
    fetchStatus.textContent = 'Fetching…';
    fetchStatus.className = 'fetch-status loading';
    urlInput.disabled = true;
    try {
        const res = await fetch(`/doc?url=${encodeURIComponent(url)}`);
        const data = await res.json();
        if (!res.ok) {
            fetchStatus.textContent = data.error ?? 'Fetch failed';
            fetchStatus.className = 'fetch-status error';
            return;
        }
        textInput.value = data.plain_text;
        const cached = data.cached ? ' (cached)' : '';
        const title = data.title ?? url;
        fetchStatus.textContent = `✔ ${title}${cached}`;
        fetchStatus.className = 'fetch-status success';
    }
    catch (err) {
        fetchStatus.textContent = 'Network error';
        fetchStatus.className = 'fetch-status error';
    }
    finally {
        urlInput.disabled = false;
        urlInput.focus();
    }
});
