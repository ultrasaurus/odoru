/**
 * Singleton WebSocket connection. One connection per browser session,
 * shared across all views. Handles synthesis segments and document status
 * events on the same channel.
 */

import type { Segment } from './types'

// ── Server → client message types ────────────────────────────────────────────

export interface SynthStartedMsg {
  type: 'synth_started'
  stream_id: string
}

export interface SegmentMsg {
  type: 'segment'
  stream_id: string
  index: number
  transcript: Segment
  cached: boolean
  paragraph_end: boolean
  audioData: ArrayBuffer  // MP3 bytes from binary WS frame
}

export interface DoneMsg {
  type: 'done'
  stream_id: string
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

// The currently-acknowledged stream — the one whose segment/done/error
// frames should actually reach a handler. All incoming frames are dropped
// unless their stream_id matches this. Frames that already entered the
// player's decodeChain before cancel fired are handled separately by the
// generation counter in player.ts.
let active: { streamId: string; handlers: SynthHandlers } | null = null

// Requests sent but not yet synth_started-acknowledged, in the order they
// were sent. The server processes one WS message at a time and sends
// synth_started synchronously before spawning the actual generation task
// (see ws_handler in main.rs), so acks are guaranteed to arrive in the same
// order requests were sent — first-in-first-out matching is safe.
//
// `superseded` is set on every still-pending entry whenever a *newer*
// sendSynth()/cancelSynth() call happens. When a superseded entry's ack
// finally arrives, it's just dropped — never made `active`, and crucially
// no cancel is sent for it: the server already auto-cancels the previous
// task itself the instant it receives the newer `synth` message (see
// `active_cancel.take()` in main.rs's ws_handler). Sending our own redundant
// cancel here would be unsafe, since the server's cancel handler cancels
// "whatever is currently active" without checking stream_id — a stale
// cancel arriving after the server has already moved on to an even newer
// stream could wrongly cancel that one instead.
interface PendingSynthRequest {
  handlers: SynthHandlers
  superseded: boolean
}
let pendingRequests: PendingSynthRequest[] = []

const statusHandlers = new Map<string, StatusHandler>()
// Messages to send once connected (e.g. synth queued before open)
const sendQueue: string[] = []
// Buffered JSON header waiting for its paired binary audio frame.
let pendingSegmentHeader: Omit<SegmentMsg, 'audioData'> | null = null

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
    // Flush any queued messages (may include a synth request)
    for (const msg of sendQueue.splice(0)) {
      ws!.send(msg)
    }
  }

  ws.onmessage = (ev: MessageEvent) => {
    // Binary frame: audio payload for the preceding JSON segment header.
    if (ev.data instanceof ArrayBuffer) {
      if (pendingSegmentHeader) {
        const header = pendingSegmentHeader
        pendingSegmentHeader = null
        if (active && header.stream_id === active.streamId) {
          active.handlers.onSegment({ ...header, audioData: ev.data })
        } else {
          console.log(`[ws] drop binary for stale stream ${header.stream_id?.slice(0, 8)} (active=${active?.streamId.slice(0, 8)})`)
        }
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
      case 'synth_started': {
        const streamId = (msg as SynthStartedMsg).stream_id
        const req = pendingRequests.shift()
        if (!req || req.superseded) {
          console.log(`[ws] discarding ack for superseded stream ${streamId.slice(0, 8)}`)
          break
        }
        active = { streamId, handlers: req.handlers }
        break
      }
      case 'segment':
        if (!active || msg.stream_id !== active.streamId) {
          console.log(`[ws] drop segment idx=${msg.index} stream=${msg.stream_id?.slice(0, 8)} (active=${active?.streamId.slice(0, 8)})`)
          // Still need to consume the pending binary frame that follows.
          pendingSegmentHeader = msg as Omit<SegmentMsg, 'audioData'>
          break
        }
        // Save header; audio arrives in the next binary frame.
        pendingSegmentHeader = msg as Omit<SegmentMsg, 'audioData'>
        break
      case 'done':
        if (!active || msg.stream_id !== active.streamId) {
          console.log(`[ws] drop done for stale stream ${msg.stream_id?.slice(0, 8)}`)
          break
        }
        active.handlers.onDone()
        active = null
        break
      case 'error':
        if (msg.stream_id !== undefined && (!active || msg.stream_id !== active.streamId)) {
          console.log(`[ws] drop error for stale stream ${msg.stream_id?.slice(0, 8)}`)
          break
        }
        active?.handlers.onError(msg.error ?? 'Unknown error')
        active = null
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
    // Any not-yet-acked requests will never get their ack on this (now
    // closed) connection — drop them rather than leaving them to
    // mis-match against a future connection's stream_ids.
    pendingRequests = []
    if (active) {
      active.handlers.onError('Connection lost — server may have restarted')
      active = null
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

/** Start a synthesis stream. Cancels any in-progress stream on the server first. */
export function sendSynth(
  text: string,
  voice: string,
  documentId: string | undefined,
  handlers: SynthHandlers,
): void {
  // Cancel the currently-active stream, if we know its id. Safe to send now
  // (before the new `synth` message below) since it unambiguously targets
  // whatever the server currently has active.
  if (active) {
    send({ type: 'cancel', stream_id: active.streamId })
    active = null
  }
  // Anything still awaiting its ack is now superseded too — when that ack
  // arrives it'll just be dropped (see the synth_started case above), no
  // cancel needed since the server auto-cancels it for us.
  for (const p of pendingRequests) p.superseded = true
  pendingRequests.push({ handlers, superseded: false })
  pendingSegmentHeader = null
  const msg: Record<string, string> = { type: 'synth', text, voice }
  if (documentId) msg.document_id = documentId
  send(msg)
}

/** Cancel the current synthesis stream. */
export function cancelSynth(): void {
  if (active) {
    send({ type: 'cancel', stream_id: active.streamId })
    active = null
  }
  for (const p of pendingRequests) p.superseded = true
  pendingSegmentHeader = null
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
