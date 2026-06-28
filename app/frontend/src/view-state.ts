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

// 'blank'      — no doc loaded (initial state, or after New)
// 'loadingDoc' — loadAndListen mid-fetch; nothing actionable yet
// 'draft'      — doc exists (just fetched, or reopened) but has no audio
//                for the active voice yet; Synthesize is still live
//                alongside New/Edit/Copy so the user can edit first
// 'listening'  — audio exists or is being synthesized
export type DocStage = 'blank' | 'loadingDoc' | 'draft' | 'listening'

export function setDocStage(
  els: { synthBtn: HTMLElement, newBtn: HTMLElement, editToggleBtn: HTMLElement, copyAnnotationsBtn: HTMLElement },
  stage: DocStage,
) {
  els.synthBtn.style.display = (stage === 'blank' || stage === 'draft') ? '' : 'none'
  els.newBtn.style.display = (stage === 'listening' || stage === 'draft') ? '' : 'none'
  els.editToggleBtn.style.display = (stage === 'listening' || stage === 'draft') ? '' : 'none'
  els.copyAnnotationsBtn.style.display = (stage === 'listening' || stage === 'draft') ? '' : 'none'
}

export function setOutline(els: { editOutlineSection: HTMLElement }, headings: HeadingEntry[]) {
  els.editOutlineSection.style.display = headings.length ? '' : 'none'
}
