import { act, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { $createTextNode, $getSelection, $getRoot, $isElementNode, $isNodeSelection, $isRangeSelection, KEY_ARROW_LEFT_COMMAND, KEY_ARROW_RIGHT_COMMAND, type LexicalEditor } from 'lexical'

import type { ReferenceCatalog, ValueSelector } from '../model/types'
import { insertVariable, selectorDisplayColor, selectorDisplayLabel, VariableTokenEditor } from './variable-token-editor'

const selector: ValueSelector = {
  namespace: 'outputs',
  sourceNodeId: 'model-node-id',
  port: 'main',
  run: { kind: 'current' },
  item: { kind: 'current' },
  path: ['text'],
}

const catalog: ReferenceCatalog = {
  inputs: [],
  contexts: [],
  outputs: [{
    id: 'outputs.model', label: 'Summarizer', path: 'outputs.model', sourceNodeType: 'model', children: [{
      id: 'outputs.model.main', label: 'main', path: 'outputs.model.main', children: [{
        id: 'outputs.model.main.current', label: 'current', path: 'outputs.model.main.current', children: [{
          id: 'outputs.model.main.current.text', label: 'text', path: 'outputs.model.main.current.text', selector, type: 'string', children: [],
        }],
      }],
    }],
  }],
}

describe('VariableTokenEditor', () => {
  it('opens a reference with the defaults allowed by the wire contract', () => {
    const compact = { namespace: 'outputs', sourceNodeId: 'model-node-id', path: ['text'] } satisfies ValueSelector
    expect(selectorDisplayLabel(compact, catalog)).toBe('Summarizer · text')
    render(<VariableTokenEditor catalog={catalog} onChange={vi.fn()} value={{ kind: 'template', segments: [{ kind: 'reference', selector: compact }] }} />)
    expect(screen.getByText('Summarizer · text')).toBeInTheDocument()
  })

  it('derives a readable chip label without persisting display metadata', () => {
    expect(selectorDisplayLabel(selector, catalog)).toBe('Summarizer · text')
    render(<VariableTokenEditor catalog={catalog} onChange={vi.fn()} value={{ kind: 'template', segments: [{ kind: 'reference', selector, missingPolicy: { kind: 'error' } }] }} />)

    expect(screen.getByText('Summarizer · text')).toBeInTheDocument()
  })

  it('renders reference chips as capsules with the source node color dot', () => {
    expect(selectorDisplayColor(selector, catalog)).toBe('#6366f1')
    render(<VariableTokenEditor catalog={catalog} onChange={vi.fn()} value={{ kind: 'template', segments: [{ kind: 'reference', selector, missingPolicy: { kind: 'error' } }] }} />)

    const chip = screen.getByText('Summarizer · text').closest('[data-agentx-variable]')!
    const dot = chip.querySelector<HTMLElement>('span[style]')
    expect(dot).toHaveStyle({ backgroundColor: '#6366f1' })
  })

  it('renders literal text without placeholder syntax', () => {
    render(<VariableTokenEditor onChange={vi.fn()} value={{ kind: 'template', segments: [{ kind: 'text', text: 'plain text' }] }} />)

    expect(screen.getByRole('textbox', { name: 'Value' })).toHaveTextContent('plain text')
  })

  it('preserves an omit policy while synchronizing a reference chip', async () => {
    const onChange = vi.fn()
    const view = render(<VariableTokenEditor catalog={catalog} onChange={onChange} value={{ kind: 'template', segments: [{ kind: 'reference', selector, missingPolicy: { kind: 'error' } }] }} />)

    view.rerender(<VariableTokenEditor catalog={catalog} onChange={onChange} value={{ kind: 'template', segments: [{ kind: 'reference', selector, missingPolicy: { kind: 'omit' } }] }} />)

    await waitFor(() => expect(screen.getByText('Summarizer · text')).toBeInTheDocument())
    expect(onChange).not.toHaveBeenCalledWith(expect.objectContaining({ missingPolicy: { kind: 'error' } }))
  })

  it('moves the caret across a standalone reference chip with arrow keys', async () => {
    let editor: LexicalEditor | undefined
    render(<VariableTokenEditor onChange={vi.fn()} onEditorReady={(value) => { editor = value }} value={{ kind: 'template', segments: [{ kind: 'reference', selector, missingPolicy: { kind: 'error' } }] }} />)
    await waitFor(() => expect(editor).toBeDefined())

    act(() => editor!.update(() => {
      $getRoot().getFirstChildOrThrow().selectEnd()
      editor!.dispatchCommand(KEY_ARROW_LEFT_COMMAND, new KeyboardEvent('keydown', { key: 'ArrowLeft' }))
    }, { discrete: true }))
    expect(readCaret(editor!)).toEqual({ type: 'element', offset: 0 })

    act(() => editor!.update(() => {
      editor!.dispatchCommand(KEY_ARROW_RIGHT_COMMAND, new KeyboardEvent('keydown', { key: 'ArrowRight' }))
    }, { discrete: true }))
    expect(readCaret(editor!)).toEqual({ type: 'element', offset: 1 })
  })

  it.each(['{{', '/'] as const)('replaces the %s picker trigger with the selected variable', async (trigger) => {
    let editor: LexicalEditor | undefined
    const onChange = vi.fn()
    render(<VariableTokenEditor catalog={catalog} onChange={onChange} onEditorReady={(value) => { editor = value }} value={{ kind: 'template', segments: [] }} />)
    await waitFor(() => expect(editor).toBeDefined())

    act(() => editor!.update(() => {
      const paragraph = $getRoot().getFirstChildOrThrow()
      if (!$isElementNode(paragraph)) throw new Error('Expected the root child to be an element')
      const text = $createTextNode(`prefix${trigger}`)
      paragraph.append(text)
      text.selectEnd()
    }, { discrete: true }))
    act(() => insertVariable(editor!, selector, 'Summarizer · text', '#6366f1', trigger))

    await waitFor(() => expect(onChange).toHaveBeenLastCalledWith({
      kind: 'template',
      segments: [
        { kind: 'text', text: 'prefix' },
        { kind: 'reference', selector, missingPolicy: { kind: 'error' } },
      ],
    }))
  })
})

function readCaret(editor: LexicalEditor) {
  let caret: { type: string; offset: number } | undefined
  editor.getEditorState().read(() => {
    const selection = $getSelection()
    if ($isRangeSelection(selection)) caret = { type: selection.anchor.type, offset: selection.anchor.offset }
    else if ($isNodeSelection(selection)) caret = { type: 'node', offset: selection.getNodes().length }
  })
  return caret
}
