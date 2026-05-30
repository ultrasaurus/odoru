function isSegment(m) { return 'audio' in m; }
function isDone(m) { return 'done' in m; }
function isError(m) { return 'error' in m; }
// ---------------------------------------------------------------------------
// AudioQueue — chains AudioBufferSourceNodes for gapless playback
// ---------------------------------------------------------------------------
class AudioQueue {
    ctx;
    nextStartTime = 0;
    started = false;
    // AudioContext clock value when the first buffer of the current play/seek
    // was scheduled. Used to compute elapsed time since last seek.
    firstStartTime = 0;
    constructor() {
        this.ctx = new AudioContext({ sampleRate: 24000 });
    }
    get currentTime() { return this.ctx.currentTime; }
    enqueue(samples) {
        const buf = this.ctx.createBuffer(1, samples.length, 24000);
        buf.copyToChannel(samples, 0);
        const src = this.ctx.createBufferSource();
        src.buffer = buf;
        src.connect(this.ctx.destination);
        if (!this.started) {
            this.nextStartTime = this.ctx.currentTime + 0.05;
            this.firstStartTime = this.nextStartTime;
            this.started = true;
        }
        else {
            this.nextStartTime = Math.max(this.nextStartTime, this.ctx.currentTime + 0.01);
        }
        src.start(this.nextStartTime);
        this.nextStartTime += buf.duration;
    }
    resume() { return this.ctx.resume(); }
    suspend() { return this.ctx.suspend(); }
    get state() { return this.ctx.state; }
    reset() {
        this.ctx.close();
        this.ctx = new AudioContext({ sampleRate: 24000 });
        this.ctx.resume(); // unlock while still in user gesture
        this.nextStartTime = 0;
        this.firstStartTime = 0;
        this.started = false;
    }
}
// ---------------------------------------------------------------------------
// Player
// ---------------------------------------------------------------------------
export class Player {
    queue;
    segments = [];
    segmentEls = [];
    activeIndex = -1;
    rafId = 0;
    container;
    ws = null;
    timeUpdateCbs = [];
    endedCbs = [];
    onReadyCb = null;
    onErrorCb = null;
    // Seconds into the full audio where the current play session started.
    // = segments[seekIndex].startTime, updated on each seek.
    seekOffset = 0;
    constructor(container) {
        this.container = container;
        this.queue = new AudioQueue();
    }
    // ---------------------------------------------------------------------------
    // Public API
    // ---------------------------------------------------------------------------
    synthesize(text) {
        this.reset();
        this.container.innerHTML = '<div class="loading">Synthesizing…</div>';
        const proto = location.protocol === 'https:' ? 'wss' : 'ws';
        this.ws = new WebSocket(`${proto}://${location.host}/ws`);
        this.ws.onopen = () => {
            this.ws.send(JSON.stringify({ text }));
        };
        this.ws.onmessage = (ev) => {
            const msg = JSON.parse(ev.data);
            if (isError(msg)) {
                this.onErrorCb?.(msg.error);
                return;
            }
            if (isDone(msg)) {
                this.ws?.close();
                return;
            }
            if (isSegment(msg)) {
                const samples = decodeF32PCM(msg.audio);
                const duration = samples.length / 24000;
                // Audio-relative start = end of previous segment (or 0)
                const prev = this.segments[this.segments.length - 1];
                const startTime = prev ? prev.endTime : 0;
                const endTime = startTime + duration;
                this.queue.enqueue(samples);
                this.segments.push({ transcript: msg.transcript, startTime, endTime, samples, paragraphEnd: msg.paragraph_end });
                this.renderSegment(msg.transcript, this.segments.length - 1);
                if (this.segments.length === 1)
                    this.onReadyCb?.();
            }
        };
        this.ws.onerror = () => { this.onErrorCb?.('WebSocket error'); };
    }
    onReady(cb) { this.onReadyCb = cb; }
    onError(cb) { this.onErrorCb = cb; }
    onEnded(cb) { this.endedCbs.push(cb); }
    onTimeUpdate(cb) { this.timeUpdateCbs.push(cb); }
    async play() {
        await this.queue.resume();
        this.startTracking();
    }
    async pause() {
        await this.queue.suspend();
        this.stopTracking();
    }
    async toggle() {
        if (this.queue.state === 'suspended') {
            await this.play();
        }
        else {
            await this.pause();
        }
    }
    get paused() { return this.queue.state !== 'running'; }
    // Total duration of all segments — stable, never changes with seeks.
    get duration() {
        const last = this.segments[this.segments.length - 1];
        return last ? last.endTime : 0;
    }
    // Current playback position in seconds relative to the full audio.
    // seekOffset anchors us to the right place after a seek.
    get position() {
        const elapsed = Math.max(0, this.queue.currentTime - this.queue.firstStartTime);
        return Math.min(this.seekOffset + elapsed, this.duration);
    }
    // ---------------------------------------------------------------------------
    // Private
    // ---------------------------------------------------------------------------
    reset() {
        this.ws?.close();
        this.stopTracking();
        this.queue.reset();
        this.segments = [];
        this.segmentEls = [];
        this.activeIndex = -1;
        this.seekOffset = 0;
    }
    renderSegment(transcript, index) {
        if (index === 0)
            this.container.innerHTML = '';
        const span = document.createElement('span');
        span.className = 'segment';
        span.textContent = transcript.text;
        span.dataset.index = String(index);
        span.addEventListener('click', () => {
            this.stopTracking();
            if (this.activeIndex >= 0) {
                this.segmentEls[this.activeIndex]?.classList.remove('active');
            }
            this.activeIndex = -1;
            // seekOffset is just the audio-relative startTime of the clicked segment
            this.seekOffset = this.segments[index].startTime;
            this.queue.reset();
            for (let i = index; i < this.segments.length; i++) {
                this.queue.enqueue(this.segments[i].samples);
            }
            this.highlightSegment(index);
            this.startTracking();
        });
        this.container.appendChild(span);
        const seg = this.segments[index];
        if (seg?.paragraphEnd) {
            // Paragraph break — add a block-level spacer
            const br = document.createElement('div');
            br.className = 'paragraph-break';
            this.container.appendChild(br);
        }
        else {
            this.container.appendChild(document.createTextNode(' '));
        }
        this.segmentEls.push(span);
    }
    startTracking() {
        const lastEndedIndex = { value: -1 };
        const tick = () => {
            const pos = this.position;
            this.timeUpdateCbs.forEach(cb => cb(pos));
            this.highlightCurrent();
            // Detect end of playback
            const last = this.segments[this.segments.length - 1];
            if (last && this.queue.currentTime >= this.queue.firstStartTime +
                (last.endTime - this.seekOffset) &&
                lastEndedIndex.value < this.segments.length - 1) {
                lastEndedIndex.value = this.segments.length - 1;
                this.stopTracking();
                this.endedCbs.forEach(cb => cb());
                return;
            }
            this.rafId = requestAnimationFrame(tick);
        };
        this.rafId = requestAnimationFrame(tick);
    }
    stopTracking() {
        cancelAnimationFrame(this.rafId);
    }
    highlightCurrent() {
        // Convert AudioContext clock to audio-relative position for segment lookup
        const pos = this.position;
        let found = -1;
        for (let i = 0; i < this.segments.length; i++) {
            const s = this.segments[i];
            if (pos >= s.startTime && pos < s.endTime) {
                found = i;
                break;
            }
        }
        this.highlightSegment(found);
    }
    highlightSegment(index) {
        if (index === this.activeIndex)
            return;
        if (this.activeIndex >= 0) {
            this.segmentEls[this.activeIndex]?.classList.remove('active');
        }
        this.activeIndex = index;
        if (index >= 0) {
            const el = this.segmentEls[index];
            el?.classList.add('active');
            el?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        }
    }
}
// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
function decodeF32PCM(b64) {
    const binary = atob(b64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++)
        bytes[i] = binary.charCodeAt(i);
    return new Float32Array(bytes.buffer);
}
