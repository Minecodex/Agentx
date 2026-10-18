import { fireEvent, render, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it } from 'vitest'
import { i18n } from '../../../app/i18n'
import { EvaluationReport } from './report'
import type { Batch, CaseResult, Run } from './types'

function result(overrides: Partial<CaseResult> = {}): CaseResult {
  return {
    id: 'case', name: 'Example', lineageId: 'origin', input: { question: 'Hello' },
    expectedOutput: { answer: 'Hi' }, actualOutput: { answer: 'Hi' }, status: 'passed',
    score: 1, reason: 'Equivalent meaning', executionId: 'business', judgeExecutionId: 'judge',
    businessCostMicros: 10, judgeCostMicros: 20,
    businessCostByCurrency: { CNY: 10 }, judgeCostByCurrency: { USD: 20 }, ...overrides,
  }
}
function run(overrides: Partial<Run> = {}): Run {
  return {
    id: 'run-1', datasetId: 'dataset', datasetName: 'Examples', contractFingerprint: 'contract',
    datasetVersionId: 'snapshot', sourceRevision: 1, workflowVersionId: 'version-1', workflowVersionNumber: 1,
    status: 'completed', totalCases: 1, passedCount: 1, failedCount: 0, errorCount: 0,
    scoredCount: 1, averageScore: 1, passRate: 1, businessCostMicros: 10, judgeCostMicros: 20,
    businessCostByCurrency: { CNY: 10 }, judgeCostByCurrency: { USD: 20 }, cases: [result()], ...overrides,
  }
}
function show(runs: Run[]) {
  const report: Batch = {
    id: 'batch', applicationId: 'app', name: 'Comparison', status: 'completed', profileName: 'Correctness',
    profileVersionId: 'profile-1', criteria: 'Compare meaning', passThreshold: 0.8,
    modelId: 'model', modelVersion: 'deployment-1', modelName: 'Judge', modelParameters: {},
    createdAt: '2026-10-18T00:00:00Z', durationMs: 1200, runs,
  }
  return render(<MemoryRouter><EvaluationReport report={report} /></MemoryRouter>)
}

describe('application evaluation reports', () => {
  beforeEach(async () => { await i18n.changeLanguage('zh-CN') })

  it('uses the lowest Workflow version as baseline and permits switching it', () => {
    show([
      run({ id: 'run-2', workflowVersionNumber: 2, failedCount: 1, passedCount: 0, cases: [result({ status: 'failed', score: 0.5 })] }),
      run(),
    ])
    const baseline = screen.getByRole('combobox', { name: 'Examples 的基线版本' })
    expect(baseline).toHaveTextContent('基线 v1')
    expect(screen.getByText('改善 0 条 · 退步 1 条')).toBeInTheDocument()
    fireEvent.click(baseline)
    fireEvent.click(screen.getByRole('option', { name: '基线 v2' }))
    expect(screen.getByText('改善 1 条 · 退步 0 条')).toBeInTheDocument()
  })

  it('keeps model errors unscored and exposes separate execution traces', () => {
    show([run({ status: 'failed', errorCount: 1, scoredCount: 0, passedCount: 0, averageScore: null, cases: [result({ status: 'error', score: null, reason: null, errorMessage: 'Judge unavailable' })] })])
    const row = screen.getByRole('row', { name: /Example/ })
    expect(within(row).getAllByRole('cell')[2]).toHaveTextContent('—')
    expect(row).toHaveTextContent('Judge unavailable')
    fireEvent.click(within(row).getByRole('button', { name: '查看详情' }))
    const detail = screen.getByRole('dialog')
    expect(within(detail).getByRole('link', { name: '业务执行 Trace' })).toHaveAttribute('href', '/executions/business')
    expect(within(detail).getByRole('link', { name: 'AI 评分 Trace' })).toHaveAttribute('href', '/executions/judge')
  })

  it('aligns only copied lineage and labels changed contracts or data', () => {
    show([
      run(),
      run({ id: 'copy', datasetId: 'copy', datasetName: 'Copied', contractFingerprint: 'changed', cases: [result({ id: 'copy-case' })] }),
      run({ id: 'unrelated', datasetId: 'other', datasetName: 'Other', cases: [result({ id: 'other-case', lineageId: 'unrelated' })] }),
    ])
    fireEvent.click(screen.getByRole('button', { name: '按来源并排对比' }))
    expect(screen.getAllByRole('row').filter(row => within(row).queryByRole('cell', { name: 'Example' }))).toHaveLength(2)
    expect(screen.getByText('测试数据已变化')).toBeInTheDocument()
    expect(screen.getAllByText('无对应用例')).toHaveLength(3)
  })
})
