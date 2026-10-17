import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useState } from 'react'

import { i18n } from '../../app/i18n'
import type { ApplicationSchedule } from '../../shared/api/types'
import { EntityFormDialog } from '../../shared/components/entity-form-dialog'
import type { JsonSchema } from '../../shared/components/schema-form'
import { scheduleFields } from './schedule-form'
import { initialScheduleInputs, scheduleInput, scheduleInputErrors } from './schedule-input-values'

const schema: JsonSchema = {
  type: 'object', required: ['message'], properties: {
    message: { type: 'string', title: '消息', minLength: 2 },
    count: { type: 'integer', title: '数量', minimum: 0, maximum: 5, default: 0 },
    enabled: { type: 'boolean', title: '启用选项', default: false },
    mode: { type: 'string', title: '模式', enum: ['fast', 'safe'], default: 'safe' },
    options: { type: 'object', required: ['region'], properties: { region: { type: 'string', title: '地区' } } },
    tags: { type: 'array', items: { type: 'string' }, minItems: 1 },
  },
}

beforeEach(async () => { await i18n.changeLanguage('zh-CN') })

function renderForm(value?: ApplicationSchedule, inputSchema: JsonSchema | undefined = schema) {
  const saved = vi.fn()
  function Harness() {
    const [open, setOpen] = useState(true)
    return <EntityFormDialog cancelLabel="取消" fields={scheduleFields(i18n.t, inputSchema, value)} onClose={() => setOpen(false)} onSubmit={async values => { saved(scheduleInput(values.input, inputSchema)) }} open={open} submitLabel="保存" title="定时触发" wide />
  }
  render(<Harness />)
  if (!value) fireEvent.change(screen.getByLabelText('名称'), { target: { value: 'Weekly report' } })
  return saved
}

function choose(label: string, option: string) {
  fireEvent.click(screen.getByRole('combobox', { name: label }))
  fireEvent.click(screen.getByRole('option', { name: option }))
}

describe('schedule input form', () => {
  it('closes only the dropdown on Escape and keeps unsaved values in the dialog', async () => {
    renderForm()
    fireEvent.change(screen.getByLabelText('消息'), { target: { value: 'Unsaved message' } })
    fireEvent.click(screen.getByRole('combobox', { name: '模式' }))
    expect(await screen.findByRole('listbox')).toBeVisible()
    fireEvent.keyDown(document, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('listbox')).not.toBeInTheDocument())
    expect(screen.getByRole('dialog', { name: '定时触发' })).toBeVisible()
    expect(screen.getByLabelText('消息')).toHaveValue('Unsaved message')
    fireEvent.keyDown(document, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
  })
  it('selects parameters and values, preserves number zero and boolean false, and never asks for JSON', async () => {
    const saved = renderForm()
    expect(screen.queryByText('输入 JSON')).not.toBeInTheDocument()
    expect(screen.queryByRole('textbox', { name: /JSON/ })).not.toBeInTheDocument()
    expect(screen.getByRole('spinbutton', { name: '数量' })).toHaveValue(0)
    expect(screen.getByRole('combobox', { name: '启用选项' })).toHaveTextContent('否')
    fireEvent.change(screen.getByLabelText('消息'), { target: { value: '定时报告' } })
    choose('模式', 'fast')
    choose('添加输入参数', 'options')
    fireEvent.change(screen.getByLabelText('地区'), { target: { value: '上海' } })
    choose('添加输入参数', 'tags')
    fireEvent.click(screen.getByRole('button', { name: '添加数组项' }))
    fireEvent.change(screen.getByLabelText('tags.0'), { target: { value: '日报' } })
    fireEvent.click(screen.getByRole('button', { name: '保存' }))
    await waitFor(() => expect(saved).toHaveBeenCalledWith({ message: '定时报告', count: 0, enabled: false, mode: 'fast', options: { region: '上海' }, tags: ['日报'] }))
  })

  it('blocks invalid values, focuses the field, and validates missing required parameters after removal', async () => {
    const saved = renderForm()
    fireEvent.click(screen.getByRole('button', { name: '保存' }))
    const message = screen.getByLabelText('消息')
    await waitFor(() => expect(message).toHaveAttribute('aria-invalid', 'true'))
    await waitFor(() => expect(message).toHaveFocus())
    expect(saved).not.toHaveBeenCalled()
    fireEvent.change(message, { target: { value: 'valid' } })
    fireEvent.change(screen.getByLabelText('数量'), { target: { value: '-1' } })
    fireEvent.click(screen.getByRole('button', { name: '保存' }))
    await waitFor(() => expect(screen.getByLabelText('数量')).toHaveAttribute('aria-invalid', 'true'))
    expect(saved).not.toHaveBeenCalled()
    fireEvent.change(screen.getByLabelText('数量'), { target: { value: '0' } })
    fireEvent.click(screen.getByRole('button', { name: '删除参数 message' }))
    fireEvent.click(screen.getByRole('button', { name: '保存' }))
    expect(await screen.findByText(/message:.*必填/)).toBeVisible()
    expect(saved).not.toHaveBeenCalled()
    choose('添加输入参数', '消息 (message) *')
    fireEvent.change(screen.getByLabelText('消息'), { target: { value: 'fixed' } })
    fireEvent.click(screen.getByRole('button', { name: '保存' }))
    await waitFor(() => expect(saved).toHaveBeenCalledWith(expect.objectContaining({ message: 'fixed', count: 0 })))
  })

  it('round-trips saved objects and arrays and prevents duplicate parameter selection', async () => {
    const input = { message: 'hello', count: 0, enabled: false, mode: 'safe', options: { region: '杭州' }, tags: ['保留'] }
    const saved = renderForm({ name: 'Existing', input, status: 'active', cronExpression: '0 9 * * 1', timezone: 'Asia/Shanghai', misfirePolicy: 'fire_once' } as ApplicationSchedule)
    expect(screen.getByLabelText('地区')).toHaveValue('杭州')
    expect(screen.getByLabelText('tags.0')).toHaveValue('保留')
    fireEvent.click(within(screen.getByRole('group', { name: '参数 options' })).getByRole('combobox', { name: '工作流输入' }))
    expect(screen.getByRole('option', { name: '消息 (message) *' })).toHaveAttribute('data-disabled')
    fireEvent.keyDown(screen.getByRole('listbox'), { key: 'Escape' })
    fireEvent.click(screen.getByRole('button', { name: '删除参数 tags' }))
    fireEvent.click(screen.getByRole('button', { name: '保存' }))
    const { tags: _tags, ...expected } = input
    await waitFor(() => expect(saved).toHaveBeenCalledWith(expected))
  })

  it('saves an empty object for workflows without inputs', async () => {
    const saved = renderForm(undefined, { type: 'object', properties: {} })
    expect(screen.getByText('此工作流无需配置输入参数。')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: '保存' }))
    await waitFor(() => expect(saved).toHaveBeenCalledWith({}))
  })

  it('clears the previous value when selecting a parameter of a different type', async () => {
    const saved = renderForm()
    fireEvent.change(screen.getByLabelText('消息'), { target: { value: 'hello' } })
    choose('添加输入参数', 'options')
    fireEvent.change(screen.getByLabelText('地区'), { target: { value: '上海' } })
    fireEvent.click(within(screen.getByRole('group', { name: '参数 options' })).getByRole('combobox', { name: '工作流输入' }))
    fireEvent.click(screen.getByRole('option', { name: 'tags' }))
    expect(screen.queryByLabelText('地区')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: '添加数组项' }))
    fireEvent.change(screen.getByLabelText('tags.0'), { target: { value: 'daily' } })
    fireEvent.click(screen.getByRole('button', { name: '保存' }))
    await waitFor(() => expect(saved).toHaveBeenCalledWith({ message: 'hello', count: 0, enabled: false, mode: 'safe', tags: ['daily'] }))
  })

  it('rejects invalid typed and nested values and preserves creation defaults', () => {
    expect(initialScheduleInputs(schema)).toEqual({ message: '', count: 0, enabled: false, mode: 'safe' })
    expect(scheduleInputErrors(schema, { message: 'ok', count: 1.5, enabled: 'false', mode: 'other', options: {}, tags: [3], removed: true })).toEqual({ count: expect.any(String), enabled: expect.any(String), mode: expect.any(String), 'options.region': expect.any(String), 'tags.0': expect.any(String), removed: expect.any(String) })
    expect(() => scheduleInput('{}', undefined)).toThrow(/发布工作流版本/)
    const files: JsonSchema = { required: ['document'], properties: { document: { type: 'object', 'x-agentx-artifact': true } } }
    expect(initialScheduleInputs(files)).toEqual({ document: null })
    expect(scheduleInputErrors(files, { document: {} })).toHaveProperty('document')
  })
})
