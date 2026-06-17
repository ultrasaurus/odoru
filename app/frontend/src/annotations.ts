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

function wrapInSegment(seg: HTMLElement, globalStart: number, len: number, ann: Annotation): boolean {
  const walker = document.createTreeWalker(seg, NodeFilter.SHOW_TEXT)
  let offset = 0
  let node: Text | null
  while ((node = walker.nextNode() as Text | null)) {
    const nodeLen = node.textContent!.length
    if (offset + nodeLen > globalStart) {
      const localStart = globalStart - offset
      const localEnd   = localStart + len
      if (localEnd <= nodeLen) {
        const range = document.createRange()
        range.setStart(node, localStart)
        range.setEnd(node, localEnd)
        try { range.surroundContents(createMarkEl(ann)); return true }
        catch { return false }
      }
      return false  // spans multiple text nodes — skip for MVP
    }
    offset += nodeLen
  }
  return false
}

function applyAnnotationToDOM(container: HTMLElement, ann: Annotation): boolean {
  for (const seg of container.querySelectorAll<HTMLElement>('.segment')) {
    const segText = seg.textContent ?? ''
    let searchFrom = 0
    let idx: number
    while ((idx = segText.indexOf(ann.text, searchFrom)) !== -1) {
      const ctx = makeContext(segText, idx, ann.text.length)
      if (!ann.context || ctx === ann.context) {
        if (wrapInSegment(seg, idx, ann.text.length, ann)) return true
      }
      searchFrom = idx + 1
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

// Wrap the current selection, save to server. Returns the new annotation or null.
export async function wrapSelection(docId: string, color: AnnotationColor, voice?: string): Promise<Annotation | null> {
  const sel = window.getSelection()
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null
  const range = sel.getRangeAt(0)

  const anchor = range.startContainer.nodeType === Node.TEXT_NODE
    ? range.startContainer.parentElement
    : range.startContainer as HTMLElement
  const seg = anchor?.closest<HTMLElement>('.segment')
  if (!seg) return null

  // Clamp to within the anchor sentence
  const segRange = document.createRange()
  segRange.selectNodeContents(seg)
  if (range.compareBoundaryPoints(Range.END_TO_END, segRange) > 0) {
    range.setEnd(segRange.endContainer, segRange.endOffset)
  }

  const text = range.toString().trim()
  if (!text) return null

  const segText = seg.textContent ?? ''
  const idx = segText.indexOf(text)
  const context = idx !== -1 ? makeContext(segText, idx, text.length) : ''

  const ann: Annotation = {
    id: generateId(),
    text,
    context,
    color,
    created_at: new Date().toISOString(),
  }

  try {
    range.surroundContents(createMarkEl(ann))
  } catch {
    sel.removeAllRanges()
    return null
  }
  sel.removeAllRanges()

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
  // Remove from DOM immediately
  const mark = container.querySelector<HTMLElement>(`.annotation[data-id="${annId}"]`)
  if (mark) unwrapMark(mark)

  // Remove from server (no alignment needed on delete)
  const current = await fetchAnnotations(docId)
  await persistAnnotations(docId, current.filter(a => a.id !== annId))
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
    wrapSelection(docId, color, getVoice() ?? undefined)
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
