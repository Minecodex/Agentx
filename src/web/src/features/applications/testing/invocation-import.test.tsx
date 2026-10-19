import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { i18n } from '../../../app/i18n'
import { InvocationImport } from './invocation-import'
import { CaseEditor } from './case-editor'
import type { Dataset, InvocationSample, TestCase } from './types'

const contract = { waitInputNodes: [],
  fingerprint: 'same',
  inputSchema: { type: 'object', properties: { question: { type: 'string', title: '问题' } }, required: ['question'] },
  outputSchema: { type: 'object', properties: { result: { type: 'string', title: '答案' } }, required: ['result'] },
}
const dataset: Dataset = { id: 'existing', applicationId: 'app', name: '已有测试集', contract, revision: 3, version: 1, sourceWorkflowVersionId: 'v1', compatibleVersions: ['v1'], caseCount: 0, incompleteCount: 0, status: 'active' }
const sample: InvocationSample = { waitInputs: {}, invocationId: 'invocation', workflowVersionId: 'v1', input: { question: '你好' }, expectedOutput: { result: '你好！' }, name: 'Invocation invocation', referenceConfirmed: false, contract, validationErrors: [] }
const posts: { path: string; body: Record<string, unknown> }[] = []
let failValidation = false

function show(element: React.ReactNode) {
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}>{element}</QueryClientProvider>)
}

beforeEach(async () => {
  await i18n.changeLanguage('zh-CN'); posts.length = 0; failValidation = false
  vi.stubGlobal('fetch', vi.fn(async (path: string, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body ?? '{}'))
    if (path.endsWith('/test-case-validation')) {
      if (failValidation) return new Response(JSON.stringify({ code: 'RUNTIME_UNAVAILABLE', message: 'Runtime unavailable' }), { status: 503 })
      return Response.json({ validationErrors: body.expectedOutput?.result ? [] : [{ field: 'expectedOutput/result', code: 'REQUIRED', message: 'result is required' }] })
    }
    posts.push({ path, body })
    return Response.json(path.endsWith('/datasets') ? { ...dataset, id: 'created' } : { id: 'case' })
  }))
})

describe('invocation dataset import', () => {
  it('validates on entry and confirms a complete sample without a checkbox or a second name form', async () => {
    const onSaved = vi.fn(async () => {}), onClose = vi.fn()
    show(<InvocationImport applicationId="app" sample={sample} datasets={[dataset]} onSaved={onSaved} onClose={onClose} />)
    const submit = screen.getByRole('button', { name: '确认加入' })
    expect(submit).toBeDisabled()
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument()
    expect(screen.queryByLabelText('用例名称')).not.toBeInTheDocument()
    await waitFor(() => expect(submit).not.toBeDisabled())
    fireEvent.click(submit)
    await waitFor(() => expect(onSaved).toHaveBeenCalledWith('existing'))
    expect(posts).toEqual([{ path: '/api/v1/applications/app/datasets/existing/cases', body: { waitInputs: {}, name: '你好', input: { question: '你好' }, expectedOutput: { result: '你好！' }, sourceInvocationId: 'invocation', referenceConfirmed: true, expectedRevision: 3, tags: [] } }])
    expect(onClose).not.toHaveBeenCalled()
  })

  it('opens incomplete samples in the side panel immediately and clears current errors after correction', async () => {
    show(<InvocationImport applicationId="app" sample={{ ...sample, expectedOutput: {}, validationErrors: [{ field: 'expectedOutput/result', code: 'REQUIRED', message: 'result is required' }] }} datasets={[]} onSaved={async () => {}} onClose={() => {}} />)
    expect(screen.getByRole('dialog')).toHaveClass('right-0')
    expect(screen.getByLabelText('用例名称')).toHaveValue('你好')
    await screen.findByText('参考输出 · 答案：请填写此必填项。')
    fireEvent.change(screen.getByLabelText('测试集名称'), { target: { value: '我的测试集' } })
    fireEvent.change(screen.getByLabelText('答案'), { target: { value: '补全答案' } })
    await waitFor(() => expect(screen.getByRole('button', { name: '确认加入' })).not.toBeDisabled())
    expect(screen.queryByText(/请填写此必填项/)).not.toBeInTheDocument()
    expect(screen.getByLabelText('测试集名称')).toHaveValue('我的测试集')
    fireEvent.click(screen.getByRole('button', { name: '确认加入' }))
    await waitFor(() => expect(posts).toHaveLength(1))
    expect(posts[0]).toMatchObject({ path: '/api/v1/applications/app/datasets', body: { name: '我的测试集', workflowVersionId: 'v1', initialCase: { name: '你好', expectedOutput: { result: '补全答案' }, referenceConfirmed: true } } })
  })

  it('preserves the dataset name and data when expanding a valid preview for editing', async () => {
    show(<InvocationImport applicationId="app" sample={sample} datasets={[]} onSaved={async () => {}} onClose={() => {}} />)
    fireEvent.change(screen.getByLabelText('测试集名称'), { target: { value: '只填一次' } })
    fireEvent.click(screen.getByRole('button', { name: '编辑用例' }))
    expect(screen.getByLabelText('测试集名称')).toHaveValue('只填一次')
    expect(screen.getByLabelText('用例名称')).toHaveValue('你好')
    expect(screen.getByLabelText('问题')).toHaveValue('你好')
    expect(screen.getByLabelText('答案')).toHaveValue('你好！')
    expect(posts).toHaveLength(0)
  })

  it('saves an incomplete sample as a draft with confirmation still pending', async () => {
    const onSaved = vi.fn(async () => {})
    show(<InvocationImport applicationId="app" sample={{ ...sample, expectedOutput: {}, validationErrors: [{ field: 'expectedOutput/result', code: 'REQUIRED', message: 'required' }] }} datasets={[dataset]} onSaved={onSaved} onClose={() => {}} />)
    const save = await screen.findByRole('button', { name: '保存草稿' })
    await waitFor(() => expect(save).not.toBeDisabled())
    fireEvent.click(save)
    await waitFor(() => expect(onSaved).toHaveBeenCalledWith('existing'))
    expect(posts[0].body.referenceConfirmed).toBe(false)
  })

  it('blocks submission when the authoritative validation service is unavailable', async () => {
    failValidation = true
    show(<InvocationImport applicationId="app" sample={sample} datasets={[dataset]} onSaved={async () => {}} onClose={() => {}} />)
    await screen.findByRole('alert')
    expect(screen.getByRole('button', { name: '确认加入' })).toBeDisabled()
    expect(posts).toHaveLength(0)
  })

  it('revalidates an existing case rather than displaying saved English errors or requiring another checkbox', async () => {
    const selected: TestCase = { id: 'case', waitInputs: {}, caseKey: 'case', lineageId: 'origin', version: 1, sortOrder: 1, name: '保留名称', input: sample.input, expectedOutput: sample.expectedOutput, tags: [], pendingFields: [], sourceInvocationId: sample.invocationId, referenceConfirmed: false, validationErrors: [{ field: 'expectedOutput', code: 'REFERENCE_UNCONFIRMED', message: 'Confirm the reference answer before evaluation' }] }
    show(<CaseEditor applicationId="app" dataset={dataset} selected={selected} onSaved={async () => {}} onClose={() => {}} />)
    const save = screen.getByRole('button', { name: '确认并保存' })
    await waitFor(() => expect(save).not.toBeDisabled())
    expect(screen.getByLabelText('用例名称')).toHaveValue('保留名称')
    expect(screen.queryByText(/Confirm the reference answer/)).not.toBeInTheDocument()
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument()
    fireEvent.click(save)
    await waitFor(() => expect(posts).toHaveLength(1))
    expect(posts[0].body).toMatchObject({ name: '保留名称', referenceConfirmed: true, version: 1, expectedRevision: 3 })
  })
})
