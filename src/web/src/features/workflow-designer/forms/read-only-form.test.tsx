import { act, render, screen, waitFor } from '@testing-library/react'
import { createRef } from 'react'
import type { LexicalEditor } from 'lexical'
import { expect, it, vi } from 'vitest'
import { ReadOnlyForm } from './read-only-form'
import { CodeEditor, type CodeEditorHandle } from './code-editor'
import { VariableTokenEditor, insertVariable } from './variable-token-editor'

it('locks native fields and rich editors, including programmatic variable insertion, then unlocks them together', async () => {
  const changed = vi.fn()
  const code = createRef<CodeEditorHandle>()
  let lexical: LexicalEditor | undefined
  const form = (readOnly: boolean) => <ReadOnlyForm readOnly={readOnly}>
    <input aria-label="Native configuration" defaultValue="Saved" />
    <button>Change configuration</button>
    <CodeEditor ref={code} onChange={changed} value="saved code" />
    <VariableTokenEditor value={{ kind:'template', segments:[] }} onChange={changed} onEditorReady={editor=>{ lexical=editor }} />
  </ReadOnlyForm>
  const view = render(form(true))
  expect(screen.getByLabelText('Native configuration')).toBeDisabled()
  expect(screen.getByRole('button',{name:'Change configuration'})).toBeDisabled()
  expect(screen.getByDisplayValue('saved code')).toHaveAttribute('readonly')
  await waitFor(()=>expect(lexical?.isEditable()).toBe(false))
  expect(screen.getByRole('textbox',{name:'Value'})).toHaveAttribute('contenteditable','false')
  act(()=>{ code.current?.insertText('Forbidden'); insertVariable(lexical!,{namespace:'inputs',path:['forbidden']}) })
  expect(changed).not.toHaveBeenCalled()
  view.rerender(form(false))
  await waitFor(()=>expect(lexical?.isEditable()).toBe(true))
  expect(screen.getByLabelText('Native configuration')).toBeEnabled()
  expect(screen.getByDisplayValue('saved code')).not.toHaveAttribute('readonly')
})
