// Approximate generation seconds per word for each backend.
export const SECS_PER_WORD = {
    kokoro: 0.2,
    f5: 3.0,
};
// Pick the best voice from a document's voices map.
// Priority: published → first ready → first stale → first any.
export function pickVoice(voices) {
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
// ── DOM helpers ───────────────────────────────────────────────────────────────
export function makeEl(tag, className, text) {
    const el = document.createElement(tag);
    el.className = className;
    el.textContent = text;
    return el;
}
export function setError(container, msg) {
    container.innerHTML = '';
    container.appendChild(makeEl('div', 'error', msg));
}
export function setStatus(container, className, msg) {
    container.innerHTML = '';
    container.appendChild(makeEl('span', className, msg));
}
export function fmt(s) {
    const m = Math.floor(s / 60);
    const sec = Math.floor(s % 60);
    return `${m}:${sec.toString().padStart(2, '0')}`;
}
export function wireControls(player, playBtn, downloadBtn, progressFill, timeCurrent, timeTotal, getFilename) {
    const playIcon = playBtn.querySelector('.play-icon');
    player.onReady(() => {
        playBtn.disabled = false;
    });
    player.onSynthDone(() => {
        downloadBtn.disabled = false;
    });
    player.onTimeUpdate(t => {
        timeCurrent.textContent = fmt(t);
        const dur = player.duration;
        const pct = dur > 0 ? (t / dur) * 100 : 0;
        progressFill.style.width = `${Math.min(pct, 100)}%`;
        timeTotal.textContent = fmt(dur);
        playIcon.textContent = player.paused ? '▶' : '⏸';
    });
    player.onEnded(() => {
        playIcon.textContent = '▶';
        progressFill.style.width = '100%';
    });
    playBtn.addEventListener('click', async () => {
        await player.toggle();
        playIcon.textContent = player.paused ? '▶' : '⏸';
    });
    downloadBtn.addEventListener('click', () => {
        player.downloadWav(getFilename());
    });
}
export function controlsHtml() {
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
export function grabControlEls() {
    return {
        playBtn: document.getElementById('play-btn'),
        downloadBtn: document.getElementById('download-btn'),
        progressFill: document.getElementById('progress-fill'),
        timeCurrent: document.getElementById('time-current'),
        timeTotal: document.getElementById('time-total'),
    };
}
