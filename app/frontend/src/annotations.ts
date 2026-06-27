export type AnnotationColor = 'yellow' | 'coral' | 'mint' | 'blue' | 'lavender'

export interface Annotation {
  id: string
  text: string
  context: string   // "<= 40 chars before>|<= 40 chars after>"
  color: AnnotationColor
  created_at: string
}

export const ANNOTATION_COLORS: Record<AnnotationColor, string> = {
  yellow:   '#fde68a',
  coral:    '#fca5a5',
  mint:     '#6ee7b7',
  blue:     '#93c5fd',
  lavender: '#c4b5fd',
}

const COLOR_ORDER: AnnotationColor[] = ['yellow', 'coral', 'mint', 'blue', 'lavender']
const COLOR_LABELS: Record<AnnotationColor, string> = {
  yellow: 'Yellow', coral: 'Coral', mint: 'Mint', blue: 'Blue', lavender: 'Lavender',
}

function generateId(): string {
  const hex = crypto.randomUUID().replace(/-/g, '')
  const bytes = new Uint8Array(16)
  for (let i = 0; i < 16; i++) bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16)
  return btoa(String.fromCharCode(...bytes))
    .replace(/\+/g, '-').replace(/\//g, '_').replace(/=/g, '')
}

// ── API helpers ─────────────────────────────────────────────────────────────

async function fetchAnnotations(docId: string): Promise<Annotation[]> {
  try {
    const res = await fetch(`/documents/${docId}/annotations`)
    if (!res.ok) return []
    return await res.json()
  } catch { return [] }
}

async function persistAnnotations(docId: string, annotations: Annotation[], voice?: string): Promise<void> {
  try {
    await fetch(`/documents/${docId}/annotations`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ annotations, voice }),
    })
  } catch { /* best-effort */ }
}

// ── DOM helpers ──────────────────────────────────────────────────────────────

function makeContext(segText: string, matchStart: number, matchLen: number): string {
  const before = segText.slice(Math.max(0, matchStart - 40), matchStart)
  const after  = segText.slice(matchStart + matchLen, matchStart + matchLen + 40)
  return `${before}|${after}`
}

function createMarkEl(ann: Annotation): HTMLElement {
  const mark = document.createElement('mark')
  mark.className = `annotation annotation-${ann.color}`
  mark.dataset.id = ann.id
  return mark
}

interface DocPosition {
  // Full text of `container`, built by walking every text node in document
  // order — including the literal space text node the renderer inserts
  // between sentences (`markdown.ts`'s `container.appendChild(createTextNode(' '))`).
  // Matches exactly what `Range.toString()` would capture for any selection,
  // including one crossing sentence (or paragraph) boundaries.
  text: string
  // Each `.segment`'s [start, end) range within `text`, in document order.
  segmentRanges: { seg: HTMLElement; start: number; end: number }[]
}

function buildDocPosition(container: HTMLElement): DocPosition {
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT)
  let text = ''
  const segmentRanges: DocPosition['segmentRanges'] = []
  let node: Text | null
  while ((node = walker.nextNode() as Text | null)) {
    const seg = node.parentElement?.closest<HTMLElement>('.segment') ?? null
    const start = text.length
    text += node.textContent ?? ''
    const end = text.length
    if (seg) {
      const last = segmentRanges[segmentRanges.length - 1]
      if (last && last.seg === seg) last.end = end  // extend (segment has multiple text nodes, e.g. inline formatting)
      else segmentRanges.push({ seg, start, end })
    }
  }
  return { text, segmentRanges }
}

// All occurrences of `needle` in `text`, as match-start offsets.
function findAllOccurrences(text: string, needle: string): number[] {
  const matches: number[] = []
  let searchFrom = 0
  let idx: number
  while ((idx = text.indexOf(needle, searchFrom)) !== -1) {
    matches.push(idx)
    searchFrom = idx + 1
  }
  return matches
}

// Wrap a document-wide [start, end) range by splitting at text-node
// boundaries — the same traversal `buildDocPosition` used to compute
// offsets, so they stay consistent. This handles segment-internal text,
// the literal space text node the renderer inserts *between* `.segment`s,
// and a segment with multiple text nodes (inline formatting) all
// uniformly: each gets wrapped in its own `<mark>` sharing `ann.id`/color,
// so CSS renders the whole thing as one continuous highlight regardless of
// how many text nodes or `.segment` boundaries it crosses. Splitting by
// `.segment` first (an earlier version of this) left the inter-segment gap
// text unwrapped, producing a visible gap in a cross-sentence highlight —
// walking all text nodes directly avoids that by construction.
function wrapRange(container: HTMLElement, start: number, end: number, ann: Annotation): boolean {
  // Snapshot every text node + its offset range *before* mutating anything.
  // surroundContents() on an earlier fragment splits/reparents that text
  // node, which would leave a live TreeWalker's currentNode pointing into a
  // now-detached part of the tree — nextNode() then stops early, silently
  // truncating the wrap to just the first fragment. Collecting the full
  // list up front and mutating in a separate pass avoids that.
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT)
  const nodes: { node: Text; start: number; end: number }[] = []
  let offset = 0
  let n: Text | null
  while ((n = walker.nextNode() as Text | null)) {
    const nodeStart = offset
    const nodeEnd = offset + (n.textContent?.length ?? 0)
    offset = nodeEnd
    nodes.push({ node: n, start: nodeStart, end: nodeEnd })
  }

  const marks: HTMLElement[] = []
  for (const { node, start: nodeStart, end: nodeEnd } of nodes) {
    const interStart = Math.max(start, nodeStart)
    const interEnd = Math.min(end, nodeEnd)
    if (interStart >= interEnd) continue
    const range = document.createRange()
    range.setStart(node, interStart - nodeStart)
    range.setEnd(node, interEnd - nodeStart)
    const mark = createMarkEl(ann)
    try { range.surroundContents(mark); marks.push(mark) }
    catch { /* skip this fragment, keep wrapping the rest */ }
  }

  // Round only the outer corners of a multi-fragment annotation, so
  // interior fragments (including a lone-space gap between sentences)
  // butt flush against their neighbors instead of looking like separate
  // bubbles. A single-fragment annotation gets both classes — rounded on
  // all sides, same as before.
  marks[0]?.classList.add('annotation-frag-start')
  marks[marks.length - 1]?.classList.add('annotation-frag-end')

  return marks.length > 0
}

function applyAnnotationToDOM(container: HTMLElement, ann: Annotation): boolean {
  const docPos = buildDocPosition(container)
  const occurrences = findAllOccurrences(docPos.text, ann.text)

  // Context only disambiguates when the text isn't already unique — the
  // 40-chars-after window can cross into a neighboring sentence, so an edit
  // there can change the recorded context even though the annotated text
  // itself is untouched. Don't let that break an otherwise-unambiguous match.
  if (occurrences.length === 1) {
    const idx = occurrences[0]
    return wrapRange(container, idx, idx + ann.text.length, ann)
  }

  for (const idx of occurrences) {
    const ctx = makeContext(docPos.text, idx, ann.text.length)
    if (!ann.context || ctx === ann.context) {
      if (wrapRange(container, idx, idx + ann.text.length, ann)) return true
    }
  }
  return false
}

// Unwrap a <mark> element, leaving its children in place.
function unwrapMark(mark: HTMLElement): void {
  const parent = mark.parentNode
  if (!parent) return
  while (mark.firstChild) parent.insertBefore(mark.firstChild, mark)
  parent.removeChild(mark)
  // Merge adjacent text nodes so future lookups work cleanly
  parent.normalize()
}

// ── Public API ───────────────────────────────────────────────────────────────

// Load from server (with migration), apply to DOM.
export async function applyAnnotations(container: HTMLElement, docId: string): Promise<void> {
  const annotations = await fetchAnnotations(docId)
  for (const ann of annotations) {
    applyAnnotationToDOM(container, ann)
  }
}

// Group this document's annotations by color (yellow/coral/mint/blue/
// lavender, in that fixed order — insertion order within each group, not
// reading order) and create a new document listing them as a heading per
// color with the annotation text as bullets underneath. Returns the new
// document's id, or null if there's nothing to copy or the create request
// fails.
export async function copyAnnotationsToNewDoc(docId: string, sourceTitle?: string): Promise<string | null> {
  const annotations = await fetchAnnotations(docId)
  if (annotations.length === 0) return null

  const byColor = new Map<AnnotationColor, Annotation[]>()
  for (const ann of annotations) {
    const list = byColor.get(ann.color) ?? []
    list.push(ann)
    byColor.set(ann.color, list)
  }

  const sections: string[] = []
  for (const color of COLOR_ORDER) {
    const list = byColor.get(color)
    if (!list || list.length === 0) continue
    const bullets = list.map(a => `- ${a.text}`).join('\n')
    sections.push(`## ${COLOR_LABELS[color]}\n\n${bullets}`)
  }
  if (sections.length === 0) return null

  const content = sections.join('\n\n')
  const plainText = content.replace(/^##\s+/gm, '').replace(/^-\s+/gm, '')
  const title = sourceTitle ? `Annotations: ${sourceTitle}` : 'Annotations'

  try {
    const res = await fetch('/documents', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ content, plain_text: plainText, title }),
    })
    if (!res.ok) return null
    const data = await res.json()
    return data.id ?? null
  } catch {
    return null
  }
}

// Wrap the current selection, save to server. Returns the new annotation or
// null. Selections may cross `.segment` boundaries — `wrapRange` (shared
// with `applyAnnotationToDOM`) handles that uniformly.
export async function wrapSelection(container: HTMLElement, docId: string, color: AnnotationColor, voice?: string): Promise<Annotation | null> {
  const sel = window.getSelection()
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null
  const range = sel.getRangeAt(0)

  const anchor = range.startContainer.nodeType === Node.TEXT_NODE
    ? range.startContainer.parentElement
    : range.startContainer as HTMLElement
  const anchorSeg = anchor?.closest<HTMLElement>('.segment')
  if (!anchorSeg) return null

  const rawText = range.toString().trim()
  if (!rawText) return null

  const docPos = buildDocPosition(container)
  const anchorRange = docPos.segmentRanges.find(r => r.seg === anchorSeg)
  if (!anchorRange) return null

  // Search from the anchor segment's own start — the selection can't start
  // before where the user clicked, so this is a safe floor that also avoids
  // accidentally matching an earlier coincidental occurrence of the same
  // text elsewhere in the document. Fall back to a plain search if that
  // somehow doesn't find it.
  let idx = docPos.text.indexOf(rawText, anchorRange.start)
  if (idx === -1) idx = docPos.text.indexOf(rawText)
  if (idx === -1) return null
  let len = rawText.length

  // Expand to word boundaries
  const wordChar = /\w/
  while (idx > 0 && wordChar.test(docPos.text[idx - 1])) idx--
  while (idx + len < docPos.text.length && wordChar.test(docPos.text[idx + len])) len++

  const text = docPos.text.slice(idx, idx + len)
  const context = makeContext(docPos.text, idx, len)

  const ann: Annotation = {
    id: generateId(),
    text,
    context,
    color,
    created_at: new Date().toISOString(),
  }

  sel.removeAllRanges()
  if (!wrapRange(container, idx, idx + len, ann)) return null

  // Persist: fetch current list, append, PUT (optimistic — DOM already updated)
  const current = await fetchAnnotations(docId)
  await persistAnnotations(docId, [...current, ann], voice)
  return ann
}

// Delete an annotation by id: remove from server and unwrap from DOM.
export async function deleteAnnotation(
  container: HTMLElement,
  docId: string,
  annId: string,
): Promise<void> {
  // Remove from DOM immediately — a cross-sentence annotation wraps as
  // multiple fragments sharing one id (see `wrapRange`), so unwrap all of them.
  const marks = container.querySelectorAll<HTMLElement>(`.annotation[data-id="${annId}"]`)
  for (const mark of marks) unwrapMark(mark)

  // Remove from server (no alignment needed on delete)
  const current = await fetchAnnotations(docId)
  await persistAnnotations(docId, current.filter(a => a.id !== annId))
}

// ── Listen to annotation ─────────────────────────────────────────────────────

let listenGen = 0

interface WordEntry { word: string; start?: number; end?: number }

function findAnnotationWordRange(annText: string, words: WordEntry[]): { start: number; end: number } | null {
  const needle = annText.toLowerCase().trim()
  const joined = words.map(w => w.word).join(' ')
  const idx = joined.toLowerCase().indexOf(needle)
  if (idx === -1) return null

  let charOffset = 0
  let firstStart: number | undefined
  let firstIndex = -1
  let lastEnd: number | undefined
  let lastIndex = -1
  for (let i = 0; i < words.length; i++) {
    const w = words[i]
    const wordEnd = charOffset + w.word.length
    if (wordEnd > idx && charOffset < idx + needle.length) {
      if (firstStart === undefined && w.start !== undefined) { firstStart = w.start; firstIndex = i }
      if (w.end !== undefined) { lastEnd = w.end; lastIndex = i }
    }
    charOffset = wordEnd + 1  // +1 for the space
  }

  if (firstStart === undefined || lastEnd === undefined) return null

  // Stop just before the next word's onset (small safety margin) rather
  // than lastEnd + a flat buffer — forced alignment's *end* boundary for
  // short words tends to land early, so a flat buffer can either bleed
  // into the next word's audio (large buffer) or cut the word off (small/
  // halved buffer) depending on how tight the gap is. The next word's
  // *start* boundary is more reliable, and stopping just before it lets
  // the current word finish naturally without spilling into the next one.
  const SAFETY_MARGIN = 0.03
  const FLAT_BUFFER = 0.15  // no next/previous word (sentence edge) — just pad.
  const nextStart = words[lastIndex + 1]?.start
  const end = nextStart !== undefined
    ? Math.max(lastEnd, nextStart - SAFETY_MARGIN)
    : lastEnd + FLAT_BUFFER

  // Mirror of the end logic: start a small pre-roll before the first
  // word's onset, clamped so it never overlaps the previous word's audio.
  const prevEnd = words[firstIndex - 1]?.end
  const start = Math.max(0, prevEnd !== undefined
    ? Math.min(firstStart, prevEnd + SAFETY_MARGIN)
    : firstStart - SAFETY_MARGIN)

  return { start, end }
}

async function fetchWords(voice: string, sentence: string, sentenceIndex: number): Promise<WordEntry[]> {
  const res = await fetch(`/voices/${encodeURIComponent(voice)}/words`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    // sentence_index is only required for imported voices (e.g. "Andy:<doc-id>")
    // — live backends (f5/kokoro) key purely on voice + text and ignore it.
    body: JSON.stringify({ sentence, sentence_index: sentenceIndex }),
  })
  if (!res.ok) {
    throw new Error(`words fetch failed: ${res.status} ${await res.text()}`)
  }
  const data = await res.json()
  return data.words ?? []
}

export async function listenAnnotation(
  mark: HTMLElement,
  annText: string,
  player: import('./player').Player,
  getVoice: () => string | null,
): Promise<void> {
  if (!player.hasAudio) return

  // A cross-sentence annotation wraps as multiple <mark> fragments sharing
  // one id (see `wrapRange`) — gather them all to find the first and last
  // segment it touches. The common case (single segment) is just a
  // one-element list, same as before.
  const annId = mark.dataset.id
  const marks = annId
    ? Array.from(document.querySelectorAll<HTMLElement>(`.annotation[data-id="${annId}"]`))
    : [mark]
  const firstMark = marks[0] ?? mark
  const lastMark = marks[marks.length - 1] ?? mark

  const firstSeg = firstMark.closest<HTMLElement>('.segment')
  const lastSeg = lastMark.closest<HTMLElement>('.segment')
  if (!firstSeg || !lastSeg) return

  const firstSegIndex = player.segmentIndexForEl(firstSeg)
  const lastSegIndex = player.segmentIndexForEl(lastSeg)
  if (firstSegIndex === -1 || lastSegIndex === -1) return

  const voice = getVoice()
  if (!voice) return

  const gen = ++listenGen
  for (const m of marks) m.classList.remove('annotation-error')  // clear any stale error (e.g. from a different voice)
  for (const m of marks) m.classList.add('annotation-loading')
  try {
    let startOffset: number
    let endOffset: number

    if (firstSeg === lastSeg) {
      const words = await fetchWords(voice, firstSeg.textContent ?? '', firstSegIndex)
      if (gen !== listenGen) return  // superseded by a later click
      const range = findAnnotationWordRange(annText, words)
      if (!range) {
        console.error('listenAnnotation: could not match annotation text in words', { annText, words })
        for (const m of marks) m.classList.add('annotation-error')
        return
      }
      startOffset = range.start
      endOffset = range.end
    } else {
      // Cross-sentence: need the start offset from the first touched
      // segment's words and the end offset from the last touched
      // segment's words. Each mark fragment's own text is exactly the
      // portion of the annotation within its segment, so search each
      // segment's words for its own fragment text rather than the full
      // (multi-sentence) `annText`.
      const [firstWords, lastWords] = await Promise.all([
        fetchWords(voice, firstSeg.textContent ?? '', firstSegIndex),
        fetchWords(voice, lastSeg.textContent ?? '', lastSegIndex),
      ])
      if (gen !== listenGen) return  // superseded by a later click

      const startRange = findAnnotationWordRange(firstMark.textContent ?? '', firstWords)
      const endRange = findAnnotationWordRange(lastMark.textContent ?? '', lastWords)
      if (!startRange || !endRange) {
        console.error('listenAnnotation: could not match cross-sentence annotation text in words', {
          annText, firstText: firstMark.textContent, lastText: lastMark.textContent, firstWords, lastWords,
        })
        for (const m of marks) m.classList.add('annotation-error')
        return
      }
      startOffset = startRange.start
      endOffset = endRange.end

      console.log('listenAnnotation: cross-sentence', {
        annText,
        firstText: firstMark.textContent, lastText: lastMark.textContent,
        firstSegIndex, lastSegIndex,
        firstSegStart: player.segmentStartTime(firstSegIndex),
        lastSegStart: player.segmentStartTime(lastSegIndex),
        startOffset, endOffset,
        stopAt: (player.segmentStartTime(lastSegIndex) ?? 0) + endOffset,
      })
    }

    player.listenTo(firstSegIndex, endOffset, startOffset, lastSegIndex)
  } catch (err) {
    console.error('listenAnnotation error:', err)
    for (const m of marks) m.classList.add('annotation-error')
  } finally {
    for (const m of marks) m.classList.remove('annotation-loading')
  }
}

// ── Color picker popover ─────────────────────────────────────────────────────

let lastColor: AnnotationColor = 'yellow'
let pickerEl: HTMLElement | null = null

// ── Context menu ─────────────────────────────────────────────────────────────

let contextMenuEl: HTMLElement | null = null

function hideContextMenu(): void {
  contextMenuEl?.remove()
  contextMenuEl = null
}

function showContextMenu(
  x: number,
  y: number,
  annId: string,
  container: HTMLElement,
  getDocId: () => string | null,
): void {
  hideContextMenu()
  const menu = document.createElement('div')
  menu.className = 'annotation-context-menu'
  menu.style.top  = `${Math.min(y, window.innerHeight - 50)}px`
  menu.style.left = `${Math.min(x, window.innerWidth - 160)}px`

  const del = document.createElement('button')
  del.className = 'annotation-context-item'
  del.textContent = 'Delete highlight'
  del.addEventListener('mousedown', e => {
    e.preventDefault()
    const docId = getDocId()
    if (docId) deleteAnnotation(container, docId, annId)
    hideContextMenu()
  })

  menu.appendChild(del)
  document.body.appendChild(menu)
  contextMenuEl = menu
}

// ── Init ─────────────────────────────────────────────────────────────────────

export function initAnnotationPicker(
  articleArea: HTMLElement,
  getDocId: () => string | null,
  isReadMode: () => boolean,
  getVoice: () => string | null,
): void {
  // Color picker
  const picker = document.createElement('div')
  picker.className = 'annotation-picker'
  picker.style.display = 'none'
  picker.innerHTML = COLOR_ORDER.map(c =>
    `<button class="annotation-swatch annotation-swatch-${c}" data-color="${c}" title="${c}"></button>`
  ).join('')
  document.body.appendChild(picker)
  pickerEl = picker

  picker.addEventListener('mousedown', e => {
    e.preventDefault()  // keep selection alive
    const btn = (e.target as HTMLElement).closest<HTMLButtonElement>('[data-color]')
    if (!btn) return
    const color = btn.dataset.color as AnnotationColor
    const docId = getDocId()
    if (!docId) { hidePicker(); return }
    lastColor = color
    wrapSelection(articleArea, docId, color, getVoice() ?? undefined)
    hidePicker()
  })

  // Show picker on mouseup within article area
  articleArea.addEventListener('mouseup', () => {
    if (!isReadMode()) return
    const docId = getDocId()
    if (!docId) return

    setTimeout(() => {
      const sel = window.getSelection()
      if (!sel || sel.isCollapsed || !sel.toString().trim()) { hidePicker(); return }
      const range = sel.getRangeAt(0)
      const anchor = range.startContainer.nodeType === Node.TEXT_NODE
        ? range.startContainer.parentElement
        : range.startContainer as HTMLElement
      if (!anchor?.closest('.segment')) { hidePicker(); return }

      const rect = range.getBoundingClientRect()
      showPicker(rect)
    }, 0)
  })

  // Context menu on right-click of an annotation mark
  articleArea.addEventListener('contextmenu', e => {
    const mark = (e.target as HTMLElement).closest<HTMLElement>('.annotation')
    if (!mark || !mark.dataset.id) return
    e.preventDefault()
    hideContextMenu()
    showContextMenu(e.clientX, e.clientY, mark.dataset.id, articleArea, getDocId)
  })

  document.addEventListener('keydown', e => {
    if (e.key === 'Escape') { hidePicker(); hideContextMenu() }
  })

  document.addEventListener('mousedown', e => {
    if (pickerEl && pickerEl.style.display !== 'none' && !pickerEl.contains(e.target as Node)) {
      hidePicker()
    }
    if (contextMenuEl && !contextMenuEl.contains(e.target as Node)) {
      hideContextMenu()
    }
  })
}

function showPicker(rect: DOMRect): void {
  if (!pickerEl) return
  pickerEl.style.display = ''
  const top  = Math.min(rect.bottom + 8, window.innerHeight - 50)
  const left = Math.max(8, Math.min(rect.left + rect.width / 2 - 88, window.innerWidth - 185))
  pickerEl.style.top  = `${top}px`
  pickerEl.style.left = `${left}px`
  pickerEl.querySelectorAll<HTMLElement>('[data-color]').forEach(btn => {
    btn.classList.toggle('annotation-swatch-active', btn.dataset.color === lastColor)
  })
}

function hidePicker(): void {
  if (pickerEl) pickerEl.style.display = 'none'
}
