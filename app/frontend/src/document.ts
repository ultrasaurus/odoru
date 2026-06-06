/**
 * Document — owns document state, subscribes to WS document_status events,
 * notifies listeners on state changes.
 */

import * as Ws from './ws'

// ── Types ─────────────────────────────────────────────────────────────────────

export interface VoiceEntry {
  status: 'in_progress' | 'ready' | 'stale' | 'error'
  duration?: number
  job_id?: string
  published?: boolean
}

export interface DocumentState {
  id: string
  status: string          // 'fetching' | 'ready' | 'error'
  source_url?: string
  title?: string
  authors: string[]
  date?: string
  description?: string
  cached_at?: string
  publish: boolean
  voices: Record<string, VoiceEntry>
  content?: string        // absent in list responses
  plain_text?: string     // absent in list responses
  error?: string
}

type StateListener = (state: DocumentState) => void

// ── Document class ────────────────────────────────────────────────────────────

export class Document {
  private state: DocumentState
  private listeners: Set<StateListener> = new Set()

  constructor(state: DocumentState) {
    this.state = state
    Ws.watch(state.id, msg => this.applyStatus(msg))
  }

  get current(): DocumentState { return this.state }

  /** Subscribe to state changes. Fires immediately with current state.
   *  Returns an unsubscribe function. */
  subscribe(cb: StateListener): () => void {
    this.listeners.add(cb)
    cb(this.state)
    return () => this.listeners.delete(cb)
  }

  /** Unwatch WS events and clear listeners. Call when done with this document. */
  destroy(): void {
    Ws.unwatch(this.state.id)
    this.listeners.clear()
  }

  private applyStatus(msg: Ws.DocumentStatusMsg): void {
    const { type: _type, ...rest } = msg
    this.state = { ...this.state, ...(rest as Partial<DocumentState>) }
    for (const cb of this.listeners) cb(this.state)
  }

  // ── Static constructors ───────────────────────────────────────────────────

  /** Load a document by id (GET /documents/:id). */
  static async load(id: string): Promise<Document> {
    const res = await fetch(`/documents/${id}`)
    if (!res.ok) throw new Error(`Failed to load document ${id}`)
    return new Document(await res.json() as DocumentState)
  }

  /** Fetch-or-create a document by URL (POST /documents), then wait via WS
   *  watch for status: ready. Returns once content is available. */
  static async fetch(url: string): Promise<Document> {
    // Create or retrieve
    const createRes = await fetch('/documents', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ url }),
    })
    if (!createRes.ok) {
      const err = await createRes.json().catch(() => ({}))
      throw new Error((err as any).error ?? 'Fetch failed')
    }
    const { id } = await createRes.json() as { id: string }

    // Get current state (may already be ready if dedup hit)
    const stateRes = await fetch(`/documents/${id}`)
    if (!stateRes.ok) throw new Error('Failed to get document state')
    const state: DocumentState = await stateRes.json()

    const doc = new Document(state)
    if (state.status === 'ready') return doc
    if (state.status === 'error') throw new Error(state.error ?? 'Fetch failed')

    // Wait for status: ready via WS watch. Poll every 500ms as a fallback in
    // case the broadcast fired before the watch was registered. When ready,
    // fetch the full document state — WS events don't carry plain_text/content.
    return new Promise((resolve, reject) => {
      let settled = false
      const settle = (s: DocumentState) => {
        if (settled) return
        if (s.status === 'ready') {
          settled = true; unsub(); clearInterval(pollTimer)
          fetch(`/documents/${state.id}`)
            .then(r => r.ok ? r.json() as Promise<DocumentState> : null)
            .then(full => { if (full) doc.applyStatus({ type: 'document_status', ...full }) })
            .catch(() => {})
            .finally(() => resolve(doc))
        } else if (s.status === 'error') {
          settled = true; unsub(); clearInterval(pollTimer)
          reject(new Error(s.error ?? 'Fetch failed'))
        }
      }
      const unsub = doc.subscribe(settle)
      const pollTimer = setInterval(async () => {
        try {
          const r = await fetch(`/documents/${state.id}`)
          const s: DocumentState | null = r.ok ? await r.json() as DocumentState : null
          if (s) doc.applyStatus({ type: 'document_status', ...s })
        } catch { /* ignore */ }
      }, 500)
    })
  }
}
