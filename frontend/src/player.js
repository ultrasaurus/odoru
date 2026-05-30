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
            // Small initial buffer to avoid glitches
            this.nextStartTime = this.ctx.currentTime + 0.05;
            this.started = true;
        }
        else {
            // On cache hits segments arrive faster than real-time — ensure we never
            // schedule a buffer in the past relative to the AudioContext clock
            this.nextStartTime = Math.max(this.nextStartTime, this.ctx.currentTime + 0.01);
        }
        const startTime = this.nextStartTime;
        src.start(startTime);
        this.nextStartTime += buf.duration;
        return { startTime, endTime: this.nextStartTime };
    }
    resume() { return this.ctx.resume(); }
    suspend() { return this.ctx.suspend(); }
    get state() { return this.ctx.state; }
    reset() {
        this.ctx.close();
        this.ctx = new AudioContext({ sampleRate: 24000 });
        // Resume immediately — we're always called from a click handler (user gesture),
        // so this is the right moment to unlock the AudioContext.
        this.ctx.resume();
        this.nextStartTime = 0;
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
                const { startTime, endTime } = this.queue.enqueue(samples);
                this.segments.push({ transcript: msg.transcript, startTime, endTime, samples });
                this.renderSegment(msg.transcript, this.segments.length - 1);
                // Signal ready on first segment
                if (this.segments.length === 1) {
                    this.onReadyCb?.();
                }
            }
        };
        this.ws.onerror = () => {
            this.onErrorCb?.('WebSocket error');
        };
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
    // Total duration based on queued segments so far
    get duration() {
        const last = this.segments[this.segments.length - 1];
        return last ? last.endTime - (this.segments[0]?.startTime ?? 0) : 0;
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
    }
    renderSegment(transcript, index) {
        // Remove loading placeholder on first segment
        if (index === 0)
            this.container.innerHTML = '';
        const span = document.createElement('span');
        span.className = 'segment';
        span.textContent = transcript.text;
        span.dataset.index = String(index);
        span.addEventListener('click', async () => {
            this.stopTracking();
            this.queue.reset();
            // Clear old highlight before resetting activeIndex
            if (this.activeIndex >= 0) {
                this.segmentEls[this.activeIndex]?.classList.remove('active');
            }
            this.activeIndex = -1;
            // Re-enqueue all segments from the clicked index onward
            for (let i = index; i < this.segments.length; i++) {
                const seg = this.segments[i];
                const { startTime, endTime } = this.queue.enqueue(seg.samples);
                seg.startTime = startTime;
                seg.endTime = endTime;
            }
            this.highlightSegment(index);
            this.startTracking();
        });
        this.container.appendChild(span);
        if (index < this.segments.length - 1 || transcript.text.endsWith('.')) {
            this.container.appendChild(document.createTextNode(' '));
        }
        this.segmentEls.push(span);
    }
    startTracking() {
        const lastEndedIndex = { value: -1 };
        const tick = () => {
            const t = this.queue.currentTime;
            this.timeUpdateCbs.forEach(cb => cb(t));
            this.highlightCurrent(t);
            // Check if all segments have finished playing
            const last = this.segments[this.segments.length - 1];
            if (last && t >= last.endTime && lastEndedIndex.value < this.segments.length - 1) {
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
    highlightCurrent(t) {
        let found = -1;
        for (let i = 0; i < this.segments.length; i++) {
            const s = this.segments[i];
            if (t >= s.startTime && t < s.endTime) {
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
