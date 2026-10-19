import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { useState } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, expect, it, vi } from 'vitest'
import { i18n } from '../../../app/i18n'
import { formatDateTime } from '../../../shared/lib/locale-format'
import { DatasetCases } from './dataset-tables'
import { EvaluationCreate } from './evaluation-create'
import { EvaluationList } from './evaluation-list'
import type { Batch, ContractVersion, Dataset, Run, TestCase } from './types'

const contract = { waitInputNodes: [], fingerprint: 'original', inputSchema: {}, outputSchema: {} }
const source: Dataset = { id: 'source', applicationId: 'app', name: '原始数据', contract, revision: 3, version: 1, sourceWorkflowVersionId: 'v1', compatibleVersions: ['v1', 'v2'], caseCount: 2, incompleteCount: 0, status: 'active' }
const copy: Dataset = { ...source, id: 'copy', name: '新版数据', revision: 7, compatibleVersions: ['v3'] }
const versions: ContractVersion[] = [1, 2, 3].map(number => ({ id: `v${number}`, versionNumber: number, archived: false, contract }))
let modelStatus = 'ready'
let failures = 0
const requests: { body: Record<string, unknown>; key: string }[] = []

beforeEach(async () => {
  await i18n.changeLanguage('zh-CN'); modelStatus = 'ready'; failures = 0; requests.length = 0
  vi.stubGlobal('fetch', vi.fn(async (path: string, init?: RequestInit) => {
    if (path.endsWith('/test-contracts')) return Response.json(versions)
    if (path.endsWith('/datasets')) return Response.json([source, copy, { ...source, id: 'draft', name: '待补全数据', incompleteCount: 1 }])
    if (path.endsWith('/evaluation-profiles')) return Response.json([{ name: '语义标准', versionId: 'profile', versionNumber: 1 }])
    if (path.endsWith('/platform-ai/model-setting')) return Response.json({ modelAlias: 'Judge', status: modelStatus })
    if (path.endsWith('/evaluations')) {
      requests.push({ body: JSON.parse(String(init?.body)), key: new Headers(init?.headers).get('Idempotency-Key')! })
      if (failures-- > 0) return Response.json({ code: 'UNAVAILABLE', message: 'Try again' }, { status: 503 })
      return Response.json({ id: 'batch' })
    }
    throw new Error(`Unexpected request: ${path}`)
  }))
})

function showCreate() {
  const created = vi.fn(async () => {})
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}><MemoryRouter><EvaluationCreate applicationId="app" onClose={() => {}} onCreated={created} /></MemoryRouter></QueryClientProvider>)
  return created
}
function choose(label: string, option: string) {
  fireEvent.click(screen.getByRole('combobox', { name: label }))
  fireEvent.click(screen.getByRole('option', { name: option }))
}

it('requires a name, selects datasets before compatible versions, and keeps retries idempotent', async () => {
  failures = 2
  const created = showCreate()
  const submit = screen.getByRole('button', { name: '开始评测' })
  expect(screen.getByLabelText('评测名称')).toHaveValue('')
  await waitFor(() => expect(screen.getByRole('button', { name: '添加测试集' })).not.toBeDisabled())
  expect(screen.queryByRole('checkbox')).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole('combobox', { name: '测试集 1' }))
  expect(screen.queryByRole('option', { name: '待补全数据' })).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole('option', { name: '原始数据' }))
  expect(screen.getByRole('checkbox', { name: '原始数据 · v1' })).not.toBeChecked()
  expect(screen.queryByRole('checkbox', { name: '原始数据 · v3' })).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole('checkbox', { name: '原始数据 · v1' }))
  expect(submit).toBeDisabled()
  fireEvent.change(screen.getByLabelText('评测名称'), { target: { value: '  对比结果  ' } })
  fireEvent.click(screen.getByRole('button', { name: '添加测试集' }))
  fireEvent.click(screen.getByRole('combobox', { name: '测试集 2' }))
  expect(screen.queryByRole('option', { name: '原始数据' })).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole('option', { name: '新版数据' }))
  expect(submit).toBeDisabled()
  fireEvent.click(screen.getByRole('checkbox', { name: '新版数据 · v3' }))
  // Changing a dataset must clear the prior version choice.
  fireEvent.click(screen.getByRole('button', { name: '移除测试集 2' }))
  choose('测试集 1', '新版数据')
  expect(screen.getByRole('checkbox', { name: '新版数据 · v3' })).not.toBeChecked()
  expect(submit).toBeDisabled()
  fireEvent.click(screen.getByRole('checkbox', { name: '新版数据 · v3' }))
  fireEvent.click(screen.getByRole('button', { name: '添加测试集' }))
  choose('测试集 2', '原始数据')
  fireEvent.click(screen.getByRole('checkbox', { name: '原始数据 · v1' }))
  fireEvent.click(screen.getByRole('checkbox', { name: '原始数据 · v2' }))
  fireEvent.click(submit)
  await screen.findByRole('alert')
  await waitFor(() => expect(submit).not.toBeDisabled())
  fireEvent.click(submit)
  await waitFor(() => expect(requests).toHaveLength(2))
  await waitFor(() => expect(submit).not.toBeDisabled())
  expect(requests[0].key).toBeTruthy()
  expect(requests[0].key).toBe(requests[1].key)
  expect(requests[0].body).toEqual({ name: '对比结果', profileVersionId: 'profile', selections: [{ datasetId: 'copy', expectedRevision: 7, workflowVersionIds: ['v3'] }, { datasetId: 'source', expectedRevision: 3, workflowVersionIds: ['v1', 'v2'] }] })
  fireEvent.change(screen.getByLabelText('评测名称'), { target: { value: '新的对比' } })
  fireEvent.click(submit)
  await waitFor(() => expect(created).toHaveBeenCalledWith('batch'))
  expect(requests[2].key).not.toBe(requests[0].key)
})

it('blocks evaluation when the platform model is unavailable', async () => {
  modelStatus = 'unavailable'; showCreate()
  await screen.findByRole('link', { name: '前往模型配置' })
  fireEvent.change(screen.getByLabelText('评测名称'), { target: { value: '不能开始' } })
  choose('测试集 1', '原始数据')
  fireEvent.click(screen.getByRole('checkbox', { name: '原始数据 · v1' }))
  expect(screen.getByRole('button', { name: '开始评测' })).toBeDisabled()
  expect(requests).toHaveLength(0)
})

it('supports partial selection and select all for view-only users', () => {
  const cases: TestCase[] = ['a', 'b'].map(id => ({ id, name: id, waitInputs: {}, caseKey: id, lineageId: id, version: 1, sortOrder: 0, input: {}, expectedOutput: {}, tags: [], pendingFields: [], referenceConfirmed: true, validationErrors: [] }))
  function Harness() {
    const [selected, select] = useState<string[]>([])
    return <DatasetCases dataset={source} versions={versions} cases={cases} selectedIds={selected} onSelect={select} canManage={false} onEdit={() => {}} onDelete={() => {}} />
  }
  render(<Harness />)
  const all = screen.getByRole('checkbox', { name: '全选用例' })
  fireEvent.click(screen.getByRole('checkbox', { name: '选择用例 a' }))
  expect(all).toBePartiallyChecked()
  fireEvent.click(all)
  expect(screen.getAllByRole('checkbox').every(value => (value as HTMLInputElement).checked)).toBe(true)
  fireEvent.click(all)
  expect(screen.getAllByRole('checkbox').every(value => !(value as HTMLInputElement).checked)).toBe(true)
  expect(screen.queryByRole('button', { name: '编辑' })).not.toBeInTheDocument()
})

it('lists dataset/version pairs, weighted scores, status and localized time in the requested order', () => {
  const run = (id: string, dataset: Dataset, version: number, scored: number, score: number | null): Run => ({ id, datasetId: dataset.id, datasetName: dataset.name, datasetVersionId: 'snapshot', sourceRevision: 1, contractFingerprint: 'original', workflowVersionId: `v${version}`, workflowVersionNumber: version, scoredCount: scored, averageScore: score, status: score === null ? 'running' : 'completed', totalCases: 3, passedCount: 0, failedCount: 0, errorCount: 0, businessCostMicros: 0, judgeCostMicros: 0, businessCostByCurrency: {}, judgeCostByCurrency: {}, cases: [] })
  const batch: Batch = { id: 'batch', applicationId: 'app', name: '版本对比', status: 'running', profileName: '语义标准', profileVersionId: 'profile', criteria: 'Correctness', passThreshold: 0.8, modelId: 'model', modelVersion: 'deployment', modelName: 'Judge', modelParameters: {}, createdAt: '2026-10-18T02:00:00Z', durationMs: 0, runs: [run('a', source, 2, 3, 1), run('b', source, 1, 1, 0), run('c', copy, 3, 0, null)] }
  const select = vi.fn()
  render(<EvaluationList batches={[batch]} onSelect={select} />)
  expect(screen.getAllByRole('columnheader').map(value => value.textContent)).toEqual(['名称', '评测方案', '测试集', '工作流版本', '完成任务', '平均分', '状态', '时间', ''])
  const cells = within(screen.getAllByRole('row')[1]).getAllByRole('cell')
  expect(cells[2]).toHaveTextContent('原始数据新版数据')
  expect(cells[3]).toHaveTextContent('v1, v2v3')
  expect(cells[4]).toHaveTextContent('2/3')
  expect(cells[5]).toHaveTextContent('75.0')
  expect(cells[7]).toHaveTextContent(formatDateTime(batch.createdAt, 'zh-CN'))
  fireEvent.click(screen.getByRole('button', { name: '查看报告' }))
  expect(select).toHaveBeenCalledWith('batch')
})
