import { describe, expect, it } from 'vitest'
import { setInputAreaVisibility, setEditPreviewVisibility, setDocStage, setOutline } from './view-state'
import { type HeadingEntry } from './markdown'

function div(): HTMLDivElement {
  return document.createElement('div')
}

describe('setInputAreaVisibility', () => {
  it('shows urlArea, hides docFields on url tab before fetch', () => {
    const els = { urlArea: div(), docFields: div() }
    setInputAreaVisibility(els, 'url', false)
    expect(els.urlArea.style.display).toBe('')
    expect(els.docFields.style.display).toBe('none')
  })

  it('hides urlArea, shows docFields on url tab after fetch', () => {
    const els = { urlArea: div(), docFields: div() }
    setInputAreaVisibility(els, 'url', true)
    expect(els.urlArea.style.display).toBe('none')
    expect(els.docFields.style.display).toBe('')
  })

  it('hides urlArea, shows docFields on text tab regardless of fetch state', () => {
    const els = { urlArea: div(), docFields: div() }
    setInputAreaVisibility(els, 'text', false)
    expect(els.urlArea.style.display).toBe('none')
    expect(els.docFields.style.display).toBe('')
  })
})

describe('setEditPreviewVisibility', () => {
  it('edit=true shows editArea, hides articleArea, labels toggle Read', () => {
    const els = { editArea: div(), articleArea: div(), editToggleBtn: div() }
    setEditPreviewVisibility(els, true)
    expect(els.editArea.style.display).toBe('')
    expect(els.articleArea.style.display).toBe('none')
    expect(els.editToggleBtn.textContent).toBe('Read')
  })

  it('edit=false shows articleArea, hides editArea, labels toggle Edit', () => {
    const els = { editArea: div(), articleArea: div(), editToggleBtn: div() }
    setEditPreviewVisibility(els, false)
    expect(els.editArea.style.display).toBe('none')
    expect(els.articleArea.style.display).toBe('')
    expect(els.editToggleBtn.textContent).toBe('Edit')
  })
})

describe('setDocStage', () => {
  function elsFor() {
    return { editToggleBtn: div(), copyAnnotationsBtn: div() }
  }

  it('closed: both hidden', () => {
    const els = elsFor()
    setDocStage(els, 'closed')
    expect(els.editToggleBtn.style.display).toBe('none')
    expect(els.copyAnnotationsBtn.style.display).toBe('none')
  })

  it('loading: both hidden', () => {
    const els = elsFor()
    setDocStage(els, 'loading')
    expect(els.editToggleBtn.style.display).toBe('none')
    expect(els.copyAnnotationsBtn.style.display).toBe('none')
  })

  it('open: both shown', () => {
    const els = elsFor()
    setDocStage(els, 'open')
    expect(els.editToggleBtn.style.display).toBe('')
    expect(els.copyAnnotationsBtn.style.display).toBe('')
  })
})

describe('setOutline', () => {
  const heading: HeadingEntry = { depth: 1, text: 'A', element: div(), sentenceIndex: 0 }

  it('shows the outline section when headings exist', () => {
    const els = { editOutlineSection: div() }
    setOutline(els, [heading])
    expect(els.editOutlineSection.style.display).toBe('')
  })

  it('hides the outline section when there are no headings', () => {
    const els = { editOutlineSection: div() }
    setOutline(els, [])
    expect(els.editOutlineSection.style.display).toBe('none')
  })
})
