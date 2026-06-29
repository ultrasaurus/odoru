import { type HeadingEntry } from './markdown'

// Single source of truth for the handful of edit.ts elements whose
// visibility is driven by edit/preview mode, doc lifecycle stage, or
// heading count. Every transition point should call these instead of
// hand-rolling its own subset of .style.display assignments — see
// dev/frontend.md for the bug class this guards against.

export function setInputAreaVisibility(
  els: { urlArea: HTMLElement, docFields: HTMLElement },
  activeTab: 'url' | 'text',
  urlFetched: boolean,
) {
  els.urlArea.style.display = (activeTab === 'url' && !urlFetched) ? '' : 'none'
  els.docFields.style.display = (activeTab === 'text' || urlFetched) ? '' : 'none'
}

export function setEditPreviewVisibility(
  els: { editArea: HTMLElement, articleArea: HTMLElement, editToggleBtn: HTMLElement },
  edit: boolean,
) {
  els.editArea.style.display = edit ? '' : 'none'
  els.articleArea.style.display = edit ? 'none' : ''
  els.editToggleBtn.textContent = edit ? 'Read' : 'Edit'
}

// 'closed'  — no doc loaded (initial state, or after New)
// 'loading' — loadAndListen mid-fetch; nothing actionable yet
// 'open'    — a doc/draft exists, regardless of whether it has audio yet —
//             Edit/Copy-Annotations apply equally either way
//
// Synthesize isn't part of this matrix at all anymore — it lives in the
// voice panel now, tied to whatever voice is picked there, not to doc
// stage. New also isn't part of this matrix — it's a global action, not
// scoped to whatever's currently open, so it lives in the header.
export type DocStage = 'closed' | 'loading' | 'open'

export function setDocStage(
  els: { editToggleBtn: HTMLElement, copyAnnotationsBtn: HTMLElement },
  stage: DocStage,
) {
  els.editToggleBtn.style.display = stage === 'open' ? '' : 'none'
  els.copyAnnotationsBtn.style.display = stage === 'open' ? '' : 'none'
}

export function setOutline(els: { editOutlineSection: HTMLElement }, headings: HeadingEntry[]) {
  els.editOutlineSection.style.display = headings.length ? '' : 'none'
}
