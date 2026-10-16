import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'

import { ParameterField, SUPPORTED_CONTROLS } from './parameter-field'

const referenceCatalog = {
  inputs: [{ id: 'inputs.question', label: 'question', path: 'inputs.question', selector: { namespace: 'inputs' as const, run: { kind: 'current' as const }, item: { kind: 'current' as const }, path: ['question'] }, type: 'string', children: [] }],
  outputs: [],
  contexts: [],
  execution: [{ id: 'execution.root', label: 'Execution information', path: 'execution', children: [] }],
}

vi.mock('./code-editor', () => ({
  CodeEditor: ({ value, onChange }: { value: string; onChange: (value: string) => void }) => <textarea aria-label="code-editor" onChange={(event) => onChange(event.target.value)} value={value} />,
}))

describe('ParameterField', () => {
  it('exposes manifest labels as accessible names for static text, number and option controls', () => {
    const onChange = vi.fn()
    render(<>
      <ParameterField name="label" onChange={onChange} parameters={{}} schema={{type:'string'}} ui={{control:'text',label:'Plugin label'}} value="before" />
      <ParameterField name="limit" onChange={onChange} parameters={{}} schema={{type:'integer'}} ui={{control:'number',label:'Result limit'}} value={5} />
      <ParameterField name="mode" onChange={onChange} parameters={{}} schema={{type:'string',enum:['safe']}} ui={{control:'select',label:'Design mode'}} value="safe" />
    </>)
    fireEvent.change(screen.getByRole('textbox',{name:'Plugin label'}),{target:{value:'after'}})
    expect(onChange).toHaveBeenCalledWith('after')
    expect(screen.getByRole('spinbutton',{name:'Result limit'})).toHaveValue(5)
    expect(screen.getByRole('combobox',{name:'Design mode'})).toHaveTextContent('safe')
  })

  it('renders Platform API provider options declared by the manifest', () => {
    render(
      <ParameterField
        name="workflowVersionId"
        onChange={vi.fn()}
        parameters={{ workflowVersionId: 'version-2' }}
        providerOptions={[{ value: 'version-1', label: 'Orders · v1' }, { value: 'version-2', label: 'Orders · v2' }]}
        required
        schema={{ type: 'string', format: 'uuid' }}
        ui={{ control: 'provider_options', provider: 'workflow_versions' }}
        value="version-2"
      />,
    )

    expect(screen.getByText('Orders · v2')).toBeInTheDocument()
    expect(screen.getByText('*')).toBeInTheDocument()
  })

  it('renders a dedicated key/value mapper declared by the manifest', () => {
    const onChange = vi.fn()
    render(<ParameterField name="values" onChange={onChange} parameters={{}} schema={{ type: 'object' }} ui={{ control: 'mapper' }} value={{ kind: 'object', fields: { total: { kind: 'literal', value: 'gross' } } }} />)

    expect(screen.getByTestId('mapper-control')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /Add field|添加字段/ }))
    expect(onChange).toHaveBeenCalledWith({ kind: 'object', fields: { total: { kind: 'literal', value: 'gross' }, field2: { kind: 'literal', value: '' } } })
  })

  it('propagates mapper binding namespaces to dynamic value rows', () => {
    render(<ParameterField
      name="inputs"
      onChange={vi.fn()}
      parameters={{}}
      referenceCatalog={referenceCatalog}
      schema={{ type: 'object', additionalProperties: {}, "x-agentx-binding": { acceptedKinds: ['literal', 'reference', 'template', 'array', 'object'], allowedNamespaces: ['execution'], acceptedCardinality: ['single'], missingPolicies: ['error'], recursive: true } }}
      ui={{ control: 'mapper' }}
      value={{ kind: 'object', fields: { workflow_name: { kind: 'literal', value: '' } } }}
    />)

    fireEvent.click(screen.getByRole('textbox', { name: 'Value' }))
    expect(screen.getByRole('button', { name: 'Execution information' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Inputs|输入/ })).not.toBeInTheDocument()
  })

  it('edits object and array parameters through visible field controls', () => {
    const onChange = vi.fn()
    render(
      <ParameterField
        name="payload"
        onChange={onChange}
        parameters={{}}
        schema={{
          type: 'object',
          properties: {
            text: { type: 'string', title: 'Text' },
            tags: { type: 'array', title: 'Tags', items: { type: 'string' } },
          },
        }}
        ui={{ control: 'json' }}
        value={{ text: '', tags: [] }}
      />,
    )

    const field = screen.getByTestId('parameter-payload')
    fireEvent.change(within(field).getByRole('textbox'), {
      target: { value: 'Agentx E2E' },
    })
    expect(onChange).toHaveBeenCalledWith({ text: 'Agentx E2E', tags: [] })

    fireEvent.click(within(field).getByRole('button', { name: 'Add item' }))
    expect(onChange).toHaveBeenCalledWith({ text: '', tags: [''] })
    expect(within(field).queryByLabelText('code-editor')).not.toBeInTheDocument()
  })

  it('supports recursively typed values when the manifest leaves JSON children open', () => {
    const onChange = vi.fn()
    function Harness() {
      const [value, setValue] = useState<unknown[]>([])
      return <ParameterField name="items" onChange={(next) => { onChange(next); setValue(next as unknown[]) }} parameters={{}} schema={{ type: 'array' }} ui={{ control: 'json' }} value={value} />
    }
    render(<Harness />)

    const field = screen.getByTestId('parameter-items')
    fireEvent.click(within(field).getByRole('button', { name: 'Add item' }))
    const item = within(field).getByTestId('json-array-item')
    fireEvent.click(within(item).getByRole('combobox', { name: 'Value type' }))
    fireEvent.click(screen.getByRole('option', { name: 'Object' }))
    fireEvent.click(within(item).getByRole('button', { name: 'Add field' }))
    fireEvent.change(within(item).getByRole('textbox', { name: 'Key' }), { target: { value: 'nested' } })
    fireEvent.blur(within(item).getByRole('textbox', { name: 'Key' }))
    fireEvent.change(within(item).getByRole('textbox', { name: 'Nested' }), { target: { value: 'ok' } })

    expect(onChange).toHaveBeenLastCalledWith([{ nested: 'ok' }])
  })

  it('keeps recursively dynamic JSON literals on their selected scalar type', () => {
    render(
      <ParameterField
        name="items"
        onChange={vi.fn()}
        parameters={{}}
        referenceCatalog={referenceCatalog}
        schema={{
          type: 'array',
          "x-agentx-binding": { acceptedKinds: ['literal', 'reference', 'template', 'array', 'object'], allowedNamespaces: ['inputs'], acceptedCardinality: ['single'], missingPolicies: ['error'], recursive: true },
        }}
        ui={{ control: 'structured' }}
        value={{ kind: 'array', items: [{ kind: 'literal', value: 7 }] }}
      />,
    )

    const field = screen.getByTestId('parameter-items')
    expect(within(field).queryByTestId('literal-type-badge')).not.toBeInTheDocument()
    expect(within(field).getByRole('textbox', { name: 'Value' })).toHaveTextContent('7')
  })

  it('uses manifest localization paths for nested array fields and enum options', () => {
    render(
      <ParameterField
        name="messages"
        nestedLocalization={{
          label: (path) => path === 'messages[].role' ? '角色' : path,
          description: (path) => path === 'messages[].role' ? '消息发送者' : undefined,
          placeholder: () => undefined,
          enumLabel: (_path, value) => value === 'user' ? '用户' : value,
        }}
        onChange={vi.fn()}
        parameters={{}}
        schema={{ type: 'array', items: { type: 'object', properties: { role: { type: 'string', enum: ['user', 'assistant'] } }, additionalProperties: false } }}
        ui={{ control: 'json' }}
        value={[{ role: 'user' }]}
      />,
    )

    expect(screen.getByText('角色')).toBeInTheDocument()
    expect(screen.getByText('消息发送者')).toBeInTheDocument()
    expect(screen.getByRole('combobox', { name: '角色' })).toHaveTextContent('用户')
  })

  it('exposes stable manifest paths for fixed object children', () => {
    const onChange = vi.fn()
    render(
      <ParameterField
        name="operations"
        onChange={onChange}
        parameters={{}}
        schema={{
          type: 'array',
          items: {
            type: 'object',
            properties: {
              operation: { type: 'string', enum: ['count', 'sum'] },
              outputField: { type: 'string' },
            },
            additionalProperties: false,
          },
        }}
        ui={{ control: 'json' }}
        value={[{ operation: 'count', outputField: '' }]}
      />,
    )

    const operation = document.querySelector<HTMLElement>('[data-field-path="operations[].operation"]')
    const outputField = document.querySelector<HTMLElement>('[data-field-path="operations[].outputField"]')
    expect(operation).not.toBeNull()
    expect(outputField).not.toBeNull()
    fireEvent.change(within(outputField!).getByRole('textbox'), { target: { value: 'count' } })
    expect(onChange).toHaveBeenCalledWith([{ operation: 'count', outputField: 'count' }])
  })

  it('enables references only when the exact parameter schema declares them', () => {
    const rendered = render(
      <ParameterField
        name="prompt"
        onChange={vi.fn()}
        parameters={{}}
        referenceCatalog={referenceCatalog}
        schema={{ type: 'string' }}
        ui={{ control: 'text' }}
        value=""
      />,
    )

    fireEvent.focus(screen.getByRole('textbox'))
    expect(screen.queryByTestId('reference-picker')).not.toBeInTheDocument()

    rendered.rerender(
      <ParameterField
        name="userQuestion"
        onChange={vi.fn()}
        parameters={{}}
        referenceCatalog={referenceCatalog}
        schema={{ type: 'string', "x-agentx-binding": { acceptedKinds: ['literal', 'reference', 'template'], allowedNamespaces: ['inputs'], acceptedCardinality: ['single'], missingPolicies: ['error'], recursive: false } }}
        ui={{ control: 'template' }}
        value=""
      />,
    )
    fireEvent.click(screen.getByRole('textbox'))
    expect(screen.getByTestId('reference-picker')).toBeInTheDocument()
  })

  it('preserves a selected variable chip after the parent stores the dynamic value', async () => {
    function Harness() {
      const [value, setValue] = useState<unknown>('')
      return (
        <ParameterField
          name="userQuestion"
          onChange={setValue}
          parameters={{}}
          referenceCatalog={referenceCatalog}
          schema={{ type: 'string', "x-agentx-binding": { acceptedKinds: ['literal', 'reference', 'template'], allowedNamespaces: ['inputs'], acceptedCardinality: ['single'], missingPolicies: ['error'], recursive: false } }}
          ui={{ control: 'template' }}
          value={value}
        />
      )
    }
    render(<Harness />)

    fireEvent.click(screen.getByRole('textbox', { name: 'Value' }))
    fireEvent.click(screen.getByRole('button', { name: /输入|Inputs/ }))
    fireEvent.click(screen.getByRole('button', { name: /question/i }))

    await waitFor(() => {
      expect(screen.getByTestId('parameter-userQuestion').querySelector('[data-agentx-variable]')).toBeInTheDocument()
      expect(screen.getByRole('textbox', { name: 'Value' })).not.toHaveTextContent('[object Object]')
      expect(screen.queryByTestId('reference-picker')).not.toBeInTheDocument()
    })
  })

  it('shows units next to numeric node parameters', () => {
    render(<ParameterField name="maxOutputTokens" onChange={vi.fn()} parameters={{}} schema={{ type: 'integer' }} ui={{ control: 'number', unit: 'tokens' }} value={4096} />)

    expect(screen.getByText('tokens')).toBeInTheDocument()
  })
})

describe('SUPPORTED_CONTROLS reconciliation', () => {
  it('matches exactly the control set offered by the current backend catalog', () => {
    // Hardcoded mirror of the control names emitted by src/crates/agentx-runtime/src/registry.rs.
    const backendControls = [
      'text', 'textarea', 'number', 'boolean', 'select', 'provider_options',
      'collection', 'fixed_collection', 'mapper', 'reference', 'value', 'template', 'structured', 'prompt',
      'json', 'code', 'condition_builder', 'buttons_editor', 'schema_editor',
      'json5_example', 'network_policy', 'kv_builder', 'sort_builder', 'api_key_placement',
    ]
    expect([...SUPPORTED_CONTROLS].sort()).toEqual([...backendControls].sort())
  })
})

describe('condition builder control', () => {
  it('edits IF/ELIF branches with names, logical operators and condition rows', () => {
    const onChange = vi.fn()
    const value = { cases: [{ id: 'case_1', name: 'Long', conditions: [{ condition: { left: { kind: 'literal', value: true }, operator: 'eq', right: { kind: 'literal', value: true } }, label: '' }], logicalOp: 'and' }] }
    render(<ParameterField name="cases" onChange={onChange} parameters={{ cases: value.cases }} schema={{ type: 'array' }} ui={{ control: 'condition_builder' }} value={value.cases} />)

    const builder = screen.getByTestId('condition-builder')
    expect(within(builder).getByTestId('condition-branch-0')).toBeInTheDocument()
    fireEvent.click(within(builder).getByRole('button', { name: /Rename branch|重命名分支/ }))
    fireEvent.change(within(builder).getByLabelText(/Name|名称/), { target: { value: 'Approved' } })
    expect(onChange).toHaveBeenLastCalledWith([{ id: 'case_1', name: 'Approved', conditions: value.cases[0].conditions, logicalOp: 'and' }])

    fireEvent.click(within(builder).getByRole('button', { name: /Add branch|添加分支/ }))
    const added = onChange.mock.calls.at(-1)?.[0] as Array<{ id: string; name: string; conditions: unknown[]; logicalOp: string }>
    expect(added[0]).toEqual(value.cases[0])
    expect(added[1]).toMatchObject({ name: '', conditions: [{ condition: { left: { kind: 'literal', value: '' }, operator: 'eq', right: { kind: 'literal', value: '' } } }], logicalOp: 'and' })
    expect(added[1].id).toMatch(/^case_[0-9a-f-]{36}$/)

  })

  it('adds and removes condition rows inside a branch', () => {
    const onChange = vi.fn()
    const cases = [{ id: 'case_1', name: '', conditions: [], logicalOp: 'and' }]
    render(<ParameterField name="cases" onChange={onChange} parameters={{ cases }} schema={{ type: 'array' }} ui={{ control: 'condition_builder' }} value={cases} />)

    const builder = screen.getByTestId('condition-builder')
    fireEvent.click(within(builder).getByRole('button', { name: /Add condition|添加条件/ }))
    expect(onChange).toHaveBeenLastCalledWith([{ id: 'case_1', name: '', conditions: [{ condition: { left: { kind: 'literal', value: '' }, operator: 'eq', right: { kind: 'literal', value: '' } } }], logicalOp: 'and' }])
  })

  it('edits a single condition group for object-shaped filter parameters', () => {
    const onChange = vi.fn()
    const filter = { conditions: [{ condition: { left: { kind: 'literal', value: true }, operator: 'eq', right: { kind: 'literal', value: true } }, label: '' }], logicalOp: 'and' }
    render(<ParameterField name="filter" onChange={onChange} parameters={{ filter }} schema={{ type: 'object' }} ui={{ control: 'condition_builder' }} value={filter} />)

    const builder = screen.getByTestId('condition-builder')
    expect(within(builder).queryByRole('combobox', { name: /Logical operator|逻辑运算/i })).not.toBeInTheDocument()

    fireEvent.click(within(builder).getAllByRole('button', { name: /Remove condition|删除条件/ })[0])
    expect(onChange).toHaveBeenLastCalledWith({ conditions: [], logicalOp: 'and' })
  })
})

describe('buttons editor control', () => {
  it('keeps stable hidden ids while editing labels and adding buttons', () => {
    const onChange = vi.fn()
    const value = { buttons: [{ id: 'approve', label: 'Approve' }] }
    render(<ParameterField name="buttons" onChange={onChange} parameters={value} schema={{ type: 'array' }} ui={{ control: 'buttons_editor' }} value={value} />)

    const editor = screen.getByTestId('buttons-editor')
    fireEvent.change(within(editor).getByLabelText(/Button label|按钮文本/), { target: { value: '通过' } })
    expect(onChange).toHaveBeenLastCalledWith({ buttons: [{ id: 'approve', label: '通过' }] })

    fireEvent.click(within(editor).getByRole('button', { name: /Add button|添加按钮/ }))
    const added = onChange.mock.calls.at(-1)?.[0] as { buttons: Array<{ id: string; label: string }> }
    expect(added.buttons[0]).toEqual({ id: 'approve', label: 'Approve' })
    expect(added.buttons[1]).toMatchObject({ label: '' })
    expect(added.buttons[1].id).toMatch(/^decision_[0-9a-f-]{36}$/)

    fireEvent.click(within(editor).getByRole('button', { name: /Remove button|删除按钮/ }))
    expect(onChange).toHaveBeenLastCalledWith({ buttons: [] })
  })
})
