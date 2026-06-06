/**
 * Singleton WebSocket connection. One connection per browser session,
 * shared across all views. Handles synthesis segments and document status
 * events on the same channel.
 */

import type { Segment } from './types'

// ── Server → client message types ────────────────────────────────────────────

export interface SegmentMsg {
  type: 'segment'
  index: number
  transcript: Segment
  cached: boolean
  paragraph_end: boolean
  audioData: ArrayBuffer  // MP3 bytes from binary WS frame
}

export interface DoneMsg {
  type: 'done'
}

export interface DocumentStatusMsg {
  type: 'document_status'
  id: string
  status: string
  [key: string]: unknown
}

// ── Synthesis handler (one active at a time) ──────────────────────────────────

export interface SynthHandlers {
  onSegment: (msg: SegmentMsg) => void | Promise<void>
  onDone: () => void
  onError: (msg: string) => void
}

// ── Internal state ────────────────────────────────────────────────────────────

type StatusHandler = (msg: DocumentStatusMsg) => void

let ws: WebSocket | null = null
let connecting = false
let currentSynth: SynthHandlers | null = null
const statusHandlers = new Map<string, StatusHandler>()
// Messages to send once connected (e.g. synth queued before open)
const sendQueue: string[] = []
// Buffered JSON header waiting for its paired binary audio frame.
let pendingSegmentHeader: Omit<SegmentMsg, 'samples'> | null = null

// ── Connection management ─────────────────────────────────────────────────────

function connect(): void {
  if (ws?.readyState === WebSocket.OPEN || connecting) return
  connecting = true

  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  ws = new WebSocket(`${proto}://${location.host}/ws`)
  ws.binaryType = 'arraybuffer'

  ws.onopen = () => {
    connecting = false
    // Re-subscribe to all watched documents
    for (const id of statusHandlers.keys()) {
      ws!.send(JSON.stringify({ type: 'watch', document_id: id }))
    }
    // Flush any queued messages
    for (const msg of sendQueue.splice(0)) {
      ws!.send(msg)
    }
  }

  ws.onmessage = (ev: MessageEvent) => {
    // Binary frame: audio payload for the preceding JSON header.
    if (ev.data instanceof ArrayBuffer) {
      if (pendingSegmentHeader) {
        currentSynth?.onSegment({ ...pendingSegmentHeader, audioData: ev.data })
        pendingSegmentHeader = null
      }
      return
    }

    let msg: any
    try { msg = JSON.parse(ev.data as string) } catch { return }

    const type: string | undefined = msg.type
    if (!type) {
      console.log('[ws] message without type field:', msg)
      return
    }

    switch (type) {
      case 'segment':
        // Save header; audio arrives in the next binary frame.
        pendingSegmentHeader = msg as Omit<SegmentMsg, 'samples'>
        break
      case 'done':
        currentSynth?.onDone()
        currentSynth = null
        break
      case 'error':
        currentSynth?.onError(msg.error ?? 'Unknown error')
        currentSynth = null
        break
      case 'document_status':
        statusHandlers.get(msg.id)?.(msg as DocumentStatusMsg)
        break
      default:
        console.log('[ws] unexpected message type:', type, msg)
    }
  }

  ws.onclose = () => {
    connecting = false
    ws = null
    pendingSegmentHeader = null
    if (currentSynth) {
      currentSynth.onError('Connection lost — server may have restarted')
      currentSynth = null
    }
    // Reconnect after a short delay
    setTimeout(connect, 2000)
  }

  ws.onerror = () => { /* onclose handles cleanup */ }
}

function send(msg: Record<string, unknown>): void {
  const json = JSON.stringify(msg)
  if (ws?.readyState === WebSocket.OPEN) {
    ws.send(json)
  } else {
    sendQueue.push(json)
    connect()
  }
}

// ── Public API ────────────────────────────────────────────────────────────────

/** Start a synthesis stream. Replaces any in-progress synthesis. */
export function sendSynth(
  text: string,
  voice: string,
  documentId: string | undefined,
  handlers: SynthHandlers,
): void {
  currentSynth = handlers
  const msg: Record<string, string> = { type: 'synth', text, voice }
  if (documentId) msg.document_id = documentId
  send(msg)
}

/** Cancel the current synthesis handler without closing the connection. */
export function cancelSynth(): void {
  currentSynth = null
}

/** Subscribe to document_status events for a given document id. */
export function watch(documentId: string, handler: StatusHandler): void {
  statusHandlers.set(documentId, handler)
  if (ws?.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify({ type: 'watch', document_id: documentId }))
  }
  // else: onopen re-subscribes all statusHandlers on connect/reconnect
}

/** Unsubscribe from document_status events. */
export function unwatch(documentId: string): void {
  statusHandlers.delete(documentId)
}

// Open connection at module load
connect()
