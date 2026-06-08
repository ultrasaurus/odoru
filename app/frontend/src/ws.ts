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
let currentSynth: SynthHandlers | null = null
// stream_id assigned by the server for the current synthesis; null between
// sendSynth() and the synth_started acknowledgement. All incoming segment,
// done, and error frames are dropped unless their stream_id matches this value,
// preventing frames from a superseded server-side stream from reaching the
// player. Frames that already entered the player's decodeChain before cancel
// fired are handled by the generation counter in player.ts.
let currentStreamId: string | null = null
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
        if (header.stream_id === currentStreamId) {
          currentSynth?.onSegment({ ...header, audioData: ev.data })
        } else {
          console.log(`[ws] drop binary for stale stream ${header.stream_id?.slice(0, 8)} (current=${currentStreamId?.slice(0, 8)})`)
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
      case 'synth_started':
        currentStreamId = (msg as SynthStartedMsg).stream_id
        break
      case 'segment':
        if (msg.stream_id !== currentStreamId) {
          console.log(`[ws] drop segment idx=${msg.index} stream=${msg.stream_id?.slice(0, 8)} (current=${currentStreamId?.slice(0, 8)})`)
          // Still need to consume the pending binary frame that follows.
          // Store with a dummy handler so the binary arm can clear it cleanly.
          pendingSegmentHeader = msg as Omit<SegmentMsg, 'audioData'>
          break
        }
        // Save header; audio arrives in the next binary frame.
        pendingSegmentHeader = msg as Omit<SegmentMsg, 'audioData'>
        break
      case 'done':
        if (msg.stream_id !== currentStreamId) {
          console.log(`[ws] drop done for stale stream ${msg.stream_id?.slice(0, 8)}`)
          break
        }
        currentSynth?.onDone()
        currentSynth = null
        currentStreamId = null
        break
      case 'error':
        if (msg.stream_id !== undefined && msg.stream_id !== currentStreamId) {
          console.log(`[ws] drop error for stale stream ${msg.stream_id?.slice(0, 8)}`)
          break
        }
        currentSynth?.onError(msg.error ?? 'Unknown error')
        currentSynth = null
        currentStreamId = null
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
    currentStreamId = null
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

/** Start a synthesis stream. Cancels any in-progress stream on the server first. */
export function sendSynth(
  text: string,
  voice: string,
  documentId: string | undefined,
  handlers: SynthHandlers,
): void {
  // Cancel previous stream on the server so it stops sending.
  if (currentStreamId) {
    send({ type: 'cancel', stream_id: currentStreamId })
  }
  currentSynth = handlers
  currentStreamId = null  // will be set when synth_started arrives
  pendingSegmentHeader = null
  const msg: Record<string, string> = { type: 'synth', text, voice }
  if (documentId) msg.document_id = documentId
  send(msg)
}

/** Cancel the current synthesis stream. */
export function cancelSynth(): void {
  if (currentStreamId) {
    send({ type: 'cancel', stream_id: currentStreamId })
  }
  currentSynth = null
  currentStreamId = null
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
