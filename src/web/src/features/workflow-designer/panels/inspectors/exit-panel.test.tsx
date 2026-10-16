import { fireEvent, render, screen, within } from '@testing-library/react'
import { useState } from 'react'
import { afterEach, describe, expect, it } from 'vitest'

import { i18n } from '../../../../app/i18n'
import type { ReferenceCatalog, WorkflowEnd } from '../../model/types'
import { ExitPanel } from './exit-panel'

const end: WorkflowEnd = {
  completion: "first_return",
  outputs: {},
  error: {
    outputs: {
      failure_message: {
        schema: { type: 'string' },
        required: true,
        sensitive: false,
      },
    },
  },
}

const catalog: ReferenceCatalog = {
  inputs: [],
  outputs: [],
  contexts: [],
  execution: [{ id: 'execution.root', label: '运行信息', path: 'execution', children: [] }],
}

afterEach(async () => { await i18n.changeLanguage('zh-CN') })

const exitEntry = (id: string, key: string) => ({
  id,
  data: { editorKind: 'exit' as const, key, label: key, protected: id === 'exit-1', parameters: { outputs: {}, errorOutputs: {} } },
})

function ExitHarness({ end: initialEnd = end, exits: initialExits }: { end?: WorkflowEnd; exits?: ReturnType<typeof exitEntry>[] }) {
  const [endValue, setEnd] = useState(initialEnd)
  const [exits, setExits] = useState(initialExits ?? [exitEntry('exit-1', 'exit')])
  return <><ExitPanel end={endValue} errorReferenceCatalog={catalog} exitId="exit-1" exits={exits} onClose={() => undefined} onEndChange={setEnd} onExitUpdate={(id, patch) => setExits((current) => current.map((exit) => exit.id === id ? { ...exit, data: { ...exit.data, ...patch } } : exit))} referenceCatalog={catalog} /><output data-testid="definition-state">{JSON.stringify({ end: endValue, exits })}</output></>
}

describe('Exit panel', () => {
  it('opens AI output mappings whose selectors omit the wire defaults', () => {
    const outputs = { reply: { schema: { type: 'string' }, required: true, sensitive: false } }
    const source: ReferenceCatalog = { inputs: [], contexts: [], outputs: [{ id: 'text', label: 'Model text', path: 'outputs.model.text', children: [], selector: { namespace: 'outputs', sourceNodeId: 'model', port: 'main', run: { kind: 'current' }, item: { kind: 'current' }, path: ['text'] } }] }
    render(<ExitPanel end={{ ...end, outputs }} exitId="exit-1" exits={[{ ...exitEntry('exit-1', 'exit'), data: { ...exitEntry('exit-1', 'exit').data, parameters: { outputs: { reply: { kind: 'reference', selector: { namespace: 'outputs', sourceNodeId: 'model', path: ['text'] } } }, errorOutputs: {} } } }]} referenceCatalog={source} onClose={() => undefined} onEndChange={() => undefined} onExitUpdate={() => undefined} />)
    expect(screen.getByTestId('exit-mapping-reply')).toHaveTextContent('Model text')
  })

  it('renders the shared contract and per-node binding sections in Chinese', async () => {
    await i18n.changeLanguage('zh-CN')
    render(<ExitHarness end={{ ...end, outputs: { answer: { schema: { type: 'string' }, required: false, sensitive: false } } }} />)
    expect(screen.getByTestId('exit-shared-banner')).toHaveTextContent('全局共享')
    expect(screen.getByText(/全局输出字段契约，所有结束节点共享.*本节点选择各自的取值来源/)).toBeInTheDocument()
    expect(screen.getByText(/全局错误输出字段契约.*本节点选择各自的取值来源/)).toBeInTheDocument()
    expect(screen.getByTestId('exit-mapping-answer')).toBeInTheDocument()
    expect(screen.getByTestId('exit-mapping-failure_message')).toBeInTheDocument()
  })

  it('offers the current error object only for error output mappings', () => {
    render(<ExitHarness />)

    fireEvent.click(within(screen.getByTestId('exit-mapping-failure_message')).getByRole('textbox', { name: 'Value' }))
    fireEvent.click(screen.getByRole('button', { name: '当前数据' }))
    expect(screen.getByRole('button', { name: /当前错误/ })).toBeInTheDocument()
  })

  it('switches the shared completion mode from any end node', () => {
    render(<ExitHarness />)
    expect(screen.getByTestId('completion-first_return')).toHaveClass('border-primary')
    fireEvent.click(screen.getByTestId('completion-all_complete'))

    const state = JSON.parse(screen.getByTestId('definition-state').textContent ?? '{}')
    expect(state.end.completion).toBe('all_complete')
    expect(screen.getByTestId('completion-all_complete')).toHaveClass('border-primary')
  })

  it('defaults to first return completion', () => {
    render(<ExitHarness end={{ ...end, completion: undefined as never }} />)
    expect(screen.getByTestId('completion-first_return')).toBeInTheDocument()
    expect(screen.getByTestId('completion-all_complete')).toBeInTheDocument()
    expect(screen.getByText('第一次返回即结束')).toBeInTheDocument()
    expect(screen.getByText('等待全部完成')).toBeInTheDocument()
  })

  it('renames a contract field and syncs bindings across all end nodes', () => {
    const exits = [
      { id: 'exit-1', data: { editorKind: 'exit' as const, key: 'exit', label: 'End', protected: true, parameters: { outputs: { answer: { kind: 'literal', value: 'one' } }, errorOutputs: {} } } },
      { id: 'exit-2', data: { editorKind: 'exit' as const, key: 'exit_2', label: 'End 2', protected: false, parameters: { outputs: { answer: { kind: 'literal', value: 'two' } }, errorOutputs: {} } } },
    ]
    render(<ExitHarness end={{ ...end, outputs: { answer: { schema: { type: 'string' }, required: false, sensitive: false } } }} exits={exits} />)

    fireEvent.click(screen.getAllByRole('button', { name: /Edit field|编辑字段/ })[0])
    fireEvent.change(screen.getByLabelText(/Output name|输出名称/), { target: { value: 'result' } })
    fireEvent.click(screen.getByRole('button', { name: /Save|保存/ }))

    const state = JSON.parse(screen.getByTestId('definition-state').textContent ?? '{}')
    expect(Object.keys(state.end.outputs)).toEqual(['result'])
    expect(state.exits[0].data.parameters.outputs.result).toMatchObject({ kind: 'literal', value: 'one' })
    expect(state.exits[0].data.parameters.outputs.answer).toBeUndefined()
    expect(state.exits[1].data.parameters.outputs.result).toMatchObject({ kind: 'literal', value: 'two' })
  })

  it('configures a shared output field through the contract dialog', () => {
    render(<ExitHarness />)

    fireEvent.click(screen.getAllByRole('button', { name: /Add field|添加字段/ })[0])
    fireEvent.click(screen.getByRole('combobox', { name: /Type|类型/ }))
    fireEvent.click(screen.getByRole('option', { name: /Number|数字/ }))
    fireEvent.change(screen.getByLabelText(/Output name|输出名称/), { target: { value: 'answer' } })
    fireEvent.change(screen.getByLabelText(/Title|标题/), { target: { value: 'Answer' } })
    fireEvent.change(screen.getByLabelText(/Description|说明/), { target: { value: 'Final answer' } })
    fireEvent.click(screen.getByLabelText(/Sensitive|敏感/))
    fireEvent.click(screen.getByRole('button', { name: /Save|保存/ }))

    const state = JSON.parse(screen.getByTestId('definition-state').textContent ?? '{}')
    expect(state.end.outputs.answer).toMatchObject({
      schema: { type: 'number', title: 'Answer', description: 'Final answer' },
      sensitive: true,
    })
    expect(screen.getByTestId('exit-mapping-answer')).toBeInTheDocument()
  })

  it('rejects invalid and duplicate contract field names before changing the definition', () => {
    render(<ExitHarness />)

    fireEvent.click(screen.getAllByRole('button', { name: /Add field|添加字段/ })[0])
    fireEvent.change(screen.getByLabelText(/Output name|输出名称/), { target: { value: '中文输出' } })

    expect(screen.getByRole('alert')).toHaveTextContent(/lowercase letters|小写字母/i)
    expect(screen.getByRole('button', { name: /Save|保存/ })).toBeDisabled()
    const state = JSON.parse(screen.getByTestId('definition-state').textContent ?? '{}')
    expect(state.end.outputs).toEqual({})
  })

  it('places required and sensitive flags before the optional description', () => {
    render(<ExitHarness />)

    fireEvent.click(screen.getAllByRole('button', { name: /Add field|添加字段/ })[0])
    const required = screen.getByLabelText(/Required|必填/)
    const sensitive = screen.getByLabelText(/Sensitive|敏感/)
    const description = screen.getByLabelText(/Description|说明/)

    expect(required.compareDocumentPosition(description) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(sensitive.compareDocumentPosition(description) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('preserves an in-progress field name when upstream contracts refresh', () => {
    const view = render(<ExitHarness />)
    fireEvent.click(screen.getAllByRole('button', { name: /Add field|添加字段/ })[0])
    fireEvent.change(screen.getByLabelText(/Output name|输出名称/), { target: { value: 'mapped' } })
    view.rerender(<ExitHarness />)
    expect(screen.getByLabelText(/Output name|输出名称/)).toHaveValue('mapped')
    fireEvent.click(screen.getByRole('combobox', { name: /Type|类型/ }))
    fireEvent.click(screen.getByRole('option', { name: /Number|数字/ }))
    fireEvent.click(screen.getByRole('button', { name: /Save|保存/ }))
    const state = JSON.parse(screen.getByTestId('definition-state').textContent ?? '{}')
    expect(state.end.outputs.mapped.schema.type).toBe('number')
    expect(state.end.outputs.output_1).toBeUndefined()
  })
})
