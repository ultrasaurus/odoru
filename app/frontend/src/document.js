/**
 * Document — owns document state, subscribes to WS document_status events,
 * notifies listeners on state changes.
 */
import * as Ws from './ws';
// ── Document class ────────────────────────────────────────────────────────────
export class Document {
    state;
    listeners = new Set();
    constructor(state) {
        this.state = state;
        Ws.watch(state.id, msg => this.applyStatus(msg));
    }
    get current() { return this.state; }
    /** Subscribe to state changes. Fires immediately with current state.
     *  Returns an unsubscribe function. */
    subscribe(cb) {
        this.listeners.add(cb);
        cb(this.state);
        return () => this.listeners.delete(cb);
    }
    /** Unwatch WS events and clear listeners. Call when done with this document. */
    destroy() {
        Ws.unwatch(this.state.id);
        this.listeners.clear();
    }
    applyStatus(msg) {
        const { type: _type, ...rest } = msg;
        this.state = { ...this.state, ...rest };
        for (const cb of this.listeners)
            cb(this.state);
    }
    // ── Static constructors ───────────────────────────────────────────────────
    /** Load a document by id (GET /documents/:id). */
    static async load(id) {
        const res = await fetch(`/documents/${id}`);
        if (!res.ok)
            throw new Error(`Failed to load document ${id}`);
        return new Document(await res.json());
    }
    /** Fetch-or-create a document by URL (POST /documents), then wait via WS
     *  watch for status: ready. Returns once content is available. */
    static async fetch(url) {
        // Create or retrieve
        const createRes = await fetch('/documents', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ url }),
        });
        if (!createRes.ok) {
            const err = await createRes.json().catch(() => ({}));
            throw new Error(err.error ?? 'Fetch failed');
        }
        const { id } = await createRes.json();
        // Get current state (may already be ready if dedup hit)
        const stateRes = await fetch(`/documents/${id}`);
        if (!stateRes.ok)
            throw new Error('Failed to get document state');
        const state = await stateRes.json();
        const doc = new Document(state);
        if (state.status === 'ready')
            return doc;
        if (state.status === 'error')
            throw new Error(state.error ?? 'Fetch failed');
        // Wait for status: ready via WS watch. Poll every 500ms as a fallback in
        // case the broadcast fired before the watch was registered. When ready,
        // fetch the full document state — WS events don't carry plain_text/content.
        return new Promise((resolve, reject) => {
            let settled = false;
            const settle = (s) => {
                if (settled)
                    return;
                if (s.status === 'ready') {
                    settled = true;
                    unsub();
                    clearInterval(pollTimer);
                    fetch(`/documents/${state.id}`)
                        .then(r => r.ok ? r.json() : null)
                        .then(full => { if (full)
                        doc.applyStatus({ type: 'document_status', ...full }); })
                        .catch(() => { })
                        .finally(() => resolve(doc));
                }
                else if (s.status === 'error') {
                    settled = true;
                    unsub();
                    clearInterval(pollTimer);
                    reject(new Error(s.error ?? 'Fetch failed'));
                }
            };
            const unsub = doc.subscribe(settle);
            const pollTimer = setInterval(async () => {
                try {
                    const r = await fetch(`/documents/${state.id}`);
                    const s = r.ok ? await r.json() : null;
                    if (s)
                        doc.applyStatus({ type: 'document_status', ...s });
                }
                catch { /* ignore */ }
            }, 500);
        });
    }
}
