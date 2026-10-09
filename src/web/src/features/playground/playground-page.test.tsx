import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import * as TooltipPrimitive from '@radix-ui/react-tooltip'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import type { JsonSchema } from '../../shared/components/schema-form'
import { ToastProvider } from '../../shared/ui/toast'
import { PlaygroundPage } from './playground-page'
import { projectHistoricalInput } from './playground-schema-values'

const authState = vi.hoisted(() => ({ canManage: true }))
vi.mock('../../app/providers/auth-provider', () => ({ useAuth: () => ({ hasPermission: (permission: string) => permission !== 'application:manage' || authState.canManage }) }))

describe('application playground sessions', () => {
  const requestedPaths: string[] = []

  beforeEach(async () => {
    requestedPaths.length = 0
    authState.canManage = true
    await i18n.changeLanguage('zh-CN')
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      requestedPaths.push(path)
      if (path === '/api/v1/applications') return jsonResponse({ items: [{ id: 'app-1', name: '客服应用', slug: 'support', status: 'active', activeDeploymentId: 'deployment-1' }], page: 1, pageSize: 100, total: 1 })
      if (path === '/api/v1/applications/app-1/deployments') return jsonResponse([{ id: 'deployment-1', applicationId: 'app-1', workflowVersionId: 'version-1', workflowVersionNumber: 3, sequenceNumber: 2, status: 'active', inputSchema: { type: 'object', properties: { prompt: { type: 'string' } }, required: ['prompt'] }, outputSchema: { type: 'object', properties: { answer: { type: 'string' } }, required: ['answer'] } }])
      if (path === '/api/v1/applications/app-1/deployments/deployment-1/playground-config') return jsonResponse({ applicationId: 'app-1', deploymentId: 'deployment-1', deploymentVersion: 2, version: 1, mapping: { questionInput: 'prompt', fileInput: null, answerOutput: 'answer', answerFilesOutput: null }, publishedVersion: 1, publishStatus: 'active', errorCode: null, errorMessage: null })
      if (path === '/api/v1/applications/app-1/sessions') return jsonResponse([
        session('session-1', '第一轮会话'),
        session('session-2', '第二轮会话'),
      ])
      if (path === '/gateway/v1/sessions/session-1/messages') return jsonResponse([message('message-1', '第一轮历史消息')])
      if (path === '/gateway/v1/sessions/session-2/messages') return jsonResponse([message('message-2', '第二轮历史消息')])
      return jsonResponse([])
    }))
  })

  it('opens a deep-linked session, shows history, and switches existing sessions', async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={client}><TooltipPrimitive.Provider><ToastProvider><MemoryRouter initialEntries={['/playground?applicationId=app-1&mode=conversation&sessionId=session-1']}><PlaygroundPage /></MemoryRouter></ToastProvider></TooltipPrimitive.Provider></QueryClientProvider>)

    expect(await screen.findByText('第一轮历史消息')).toBeVisible()
    expect(screen.getByPlaceholderText('输入消息进行测试…')).toBeEnabled()
    expect(screen.getByPlaceholderText('输入消息进行测试…').tagName).toBe('TEXTAREA')
    expect(screen.getByRole('button', { name: '添加附件' })).toBeDisabled()
    expect(requestedPaths).toContain('/api/v1/applications/app-1/sessions')

    fireEvent.click(screen.getByRole('button', { name: /第二轮会话/ }))

    expect(await screen.findByText('第二轮历史消息')).toBeVisible()
    await waitFor(() => expect(requestedPaths).toContain('/gateway/v1/sessions/session-2/messages'))
  })

  it('sends an uploaded image as a native image part from the conversation composer', async () => {
    let submitted: { parts: Array<{ partType: string; artifactId?: string }> } | undefined
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      if (path === '/api/v1/applications') return jsonResponse({ items: [application()], page: 1, pageSize: 100, total: 1 })
      if (path === '/api/v1/applications/app-1/deployments') return jsonResponse([deployment()])
      if (path.endsWith('/playground-config')) return jsonResponse(config({ questionInput: 'prompt', fileInput: 'attachments', answerOutput: 'answer', answerFilesOutput: null }))
      if (path === '/api/v1/applications/app-1/sessions') return jsonResponse([session('session-1', '第一轮会话')])
      if (path === '/gateway/v1/artifacts') return jsonResponse({ artifactId: 'image-1', contentType: 'image/png', sizeBytes: 8, sha256: 'image-sha' })
      if (path === '/gateway/v1/sessions/session-1/messages' && init?.method === 'POST') {
        submitted = JSON.parse(String(init.body))
        return jsonResponse({ id: 'image-invocation', status: 'completed' })
      }
      if (path === '/gateway/v1/invocations/image-invocation') return jsonResponse({ id: 'image-invocation', status: 'completed' })
      return jsonResponse([])
    }))
    const view = renderPlayground('/playground?applicationId=app-1&mode=conversation&sessionId=session-1')
    const composer = await screen.findByPlaceholderText('输入消息进行测试…')
    await waitFor(() => expect(screen.getByRole('button', { name: '添加附件' })).toBeEnabled())
    fireEvent.change(view.container.querySelector('input[type="file"]')!, { target: { files: [new File(['tiny png'], 'tiny.png', { type: 'image/png' })] } })
    expect(await screen.findByText('tiny.png')).toBeVisible()
    fireEvent.change(composer, { target: { value: 'Describe this image' } })
    fireEvent.click(screen.getByRole('button', { name: '发送' }))
    await waitFor(() => expect(submitted?.parts).toEqual(expect.arrayContaining([{ partType: 'image', artifactId: 'image-1', content: expect.objectContaining({ type: 'image', contentType: 'image/png' }) }])))
  })

  it('opens mapping configuration from the settings action and keeps publishing state', async () => {
    let saved: unknown
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      if (path === '/api/v1/applications') return jsonResponse({ items: [application()], page: 1, pageSize: 100, total: 1 })
      if (path === '/api/v1/applications/app-1/deployments') return jsonResponse([deployment()])
      if (path === '/api/v1/applications/app-1/deployments/deployment-1/playground-config' && init?.method === 'PUT') {
        saved = JSON.parse(String(init.body))
        return jsonResponse({ ...config(null), mapping: (saved as { mapping: unknown }).mapping, publishStatus: 'publishing', version: 2 })
      }
      if (path === '/api/v1/applications/app-1/deployments/deployment-1/playground-config') return jsonResponse(config(null))
      if (path === '/api/v1/applications/app-1/sessions') return jsonResponse([session('session-1', '第一轮会话')])
      if (path === '/gateway/v1/sessions/session-1/messages') return jsonResponse([])
      return jsonResponse([])
    }))
    renderPlayground('/playground?applicationId=app-1&mode=conversation&sessionId=session-1')

    fireEvent.click(await screen.findByRole('button', { name: '参数映射' }))
    expect(screen.getByRole('dialog', { name: '对话参数映射' })).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: '保存并发布' }))

    await waitFor(() => expect(saved).toMatchObject({ expectedVersion: 1, mapping: { questionInput: 'prompt', answerOutput: 'answer' } }))
    expect(await screen.findByText('映射发布中，发送已暂停')).toBeVisible()
    expect(screen.getByPlaceholderText('请先配置对话参数映射')).toBeVisible()
  })

  it('shows a read-only mapping permission hint to invoke-only users', async () => {
    authState.canManage = false
    vi.stubGlobal('fetch', playgroundFetch(config(null)))
    renderPlayground('/playground?applicationId=app-1&mode=conversation&sessionId=session-1')

    fireEvent.click(await screen.findByRole('button', { name: '参数映射' }))
    expect(screen.getByText('你可以使用已发布映射，但没有修改共享映射的权限。')).toBeVisible()
    expect(screen.getByRole('button', { name: '保存并发布' })).toBeDisabled()
  })

  it('clears a shared mapping and returns the composer to unconfigured state', async () => {
    let saved: unknown
    const active = { questionInput: 'prompt', fileInput: null, answerOutput: 'answer', answerFilesOutput: null }
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      if (path === '/api/v1/applications') return jsonResponse({ items: [application()], page: 1, pageSize: 100, total: 1 })
      if (path === '/api/v1/applications/app-1/deployments') return jsonResponse([deployment()])
      if (path === '/api/v1/applications/app-1/deployments/deployment-1/playground-config' && init?.method === 'PUT') { saved = JSON.parse(String(init.body)); return jsonResponse({ ...config(null), version: 2, publishStatus: 'publishing' }) }
      if (path === '/api/v1/applications/app-1/deployments/deployment-1/playground-config') return jsonResponse(config(active))
      if (path === '/api/v1/applications/app-1/sessions') return jsonResponse([session('session-1', '第一轮会话')])
      if (path === '/gateway/v1/sessions/session-1/messages') return jsonResponse([])
      return jsonResponse([])
    }))
    renderPlayground('/playground?applicationId=app-1&mode=conversation&sessionId=session-1')

    fireEvent.click(await screen.findByRole('button', { name: '参数映射' }))
    fireEvent.click(screen.getByRole('button', { name: '清除映射' }))

    await waitFor(() => expect(saved).toEqual({ expectedVersion: 1, mapping: null }))
    expect(await screen.findByPlaceholderText('请先配置对话参数映射')).toBeVisible()
  })

  it('renders Assistant Artifact parts as file cards', async () => {
    const active = { questionInput: 'prompt', fileInput: null, answerOutput: 'answer', answerFilesOutput: null }
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      if (path === '/api/v1/applications') return jsonResponse({ items: [application()], page: 1, pageSize: 100, total: 1 })
      if (path === '/api/v1/applications/app-1/deployments') return jsonResponse([deployment()])
      if (path.endsWith('/playground-config')) return jsonResponse(config(active))
      if (path === '/api/v1/applications/app-1/sessions') return jsonResponse([session('session-1', '第一轮会话')])
      if (path === '/gateway/v1/sessions/session-1/messages') return jsonResponse([{ id: 'assistant-file', role: 'assistant', invocationId: null, parts: [{ partType: 'file', artifactId: 'artifact-1', content: { artifactId: 'artifact-1', fileName: 'report.pdf' } }], createdAt: '2026-08-22T01:00:00Z' }])
      return jsonResponse([])
    }))
    renderPlayground('/playground?applicationId=app-1&mode=conversation&sessionId=session-1')

    expect(await screen.findByRole('button', { name: /report.pdf/ })).toBeVisible()
  })

  it('keeps sending disabled and surfaces a stable mapping publish failure', async () => {
    const failed = { ...config({ questionInput: 'prompt', fileInput: null, answerOutput: 'answer', answerFilesOutput: null }), publishStatus: 'failed', errorCode: 'RUNTIME_UNAVAILABLE', errorMessage: 'Runtime is unavailable' }
    vi.stubGlobal('fetch', playgroundFetch(failed))
    renderPlayground('/playground?applicationId=app-1&mode=conversation&sessionId=session-1')

    expect(await screen.findByText('RUNTIME_UNAVAILABLE: Runtime is unavailable')).toBeVisible()
    expect(screen.getByPlaceholderText('请先配置对话参数映射')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: '参数映射' }))
    expect(screen.getByRole('dialog', { name: '对话参数映射' })).toBeVisible()
  })

  it('loads stateless run history and opens persisted results', async () => {
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      if (path === '/api/v1/applications') return jsonResponse({ items: [application()], page: 1, pageSize: 100, total: 1 })
      if (path === '/api/v1/applications/app-1/deployments') return jsonResponse([deployment()])
      if (path === '/api/v1/executions') return jsonResponse({ items: [{ id: 'execution-1', status: 'succeeded', startedAt: '2026-08-22T01:00:00Z', durationMs: 12, inputTokens: 3, outputTokens: 4, costMicros: 5, traceId: 'trace-123456789' }], limit: 50, total: 1 })
      if (path === '/api/v1/executions/execution-1') return jsonResponse({ id: 'execution-1', status: 'succeeded', startedAt: '2026-08-22T01:00:00Z', input: { prompt: '历史问题', removedParameter: 'ignored' }, output: { answer: '历史答案' }, error: null })
      return jsonResponse([])
    }))
    renderPlayground('/playground?applicationId=app-1&mode=parameters')

    fireEvent.click(await screen.findByRole('button', { name: /execution-1/i }))
    expect(await screen.findByText(/历史答案/)).toBeVisible()
    expect(screen.getByLabelText('Prompt')).toHaveValue('历史问题')
    expect(screen.getByRole('link', { name: '执行详情 / Trace' })).toHaveAttribute('href', '/executions/execution-1')
  })

  it('refreshes stateless history when polling reaches a terminal state without SSE events', async () => {
    let executionSearches = 0
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      if (path === '/api/v1/applications') return jsonResponse({ items: [application()], page: 1, pageSize: 100, total: 1 })
      if (path === '/api/v1/applications/app-1/deployments') return jsonResponse([deployment()])
      if (path === '/api/v1/executions') {
        executionSearches += 1
        return jsonResponse({ items: executionSearches > 1 ? [{ id: 'execution-polled', status: 'succeeded', startedAt: '2026-08-22T01:00:00Z' }] : [], pageSize: 50, total: executionSearches > 1 ? 1 : 0 })
      }
      if (path === '/gateway/v1/applications/support/invocations' && init?.method === 'POST') return jsonResponse({ id: 'invocation-polled', executionId: 'execution-polled', status: 'queued' })
      if (path === '/gateway/v1/invocations/invocation-polled/events') return new Response('', { headers: { 'Content-Type': 'text/event-stream' } })
      if (path === '/gateway/v1/invocations/invocation-polled') return jsonResponse({ id: 'invocation-polled', executionId: 'execution-polled', status: 'completed', outputs: { answer: 'poll fallback' } })
      return jsonResponse([])
    }))
    renderPlayground('/playground?applicationId=app-1&mode=parameters')

    fireEvent.change(await screen.findByLabelText('Prompt'), { target: { value: 'poll fallback' } })
    fireEvent.click(screen.getByRole('button', { name: '运行测试' }))

    expect(await screen.findByRole('button', { name: /execution-polled/i })).toBeVisible()
    expect(executionSearches).toBeGreaterThan(1)
  })

  it('shows an explicit empty state when the application has no active deployment', async () => {
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      if (path === '/api/v1/applications') return jsonResponse({ items: [application()], page: 1, pageSize: 100, total: 1 })
      if (path === '/api/v1/applications/app-1/deployments') return jsonResponse([])
      return jsonResponse([])
    }))
    renderPlayground('/playground?applicationId=app-1&mode=parameters')

    expect(await screen.findByText('没有活动 Deployment')).toBeVisible()
  })

  it('keeps mapping save disabled when schemas expose no compatible chat fields', async () => {
    const incompatible = { ...deployment(), inputSchema: { type: 'object', properties: { count: { type: 'integer' } } }, outputSchema: { type: 'object', properties: { secret: { type: 'string', 'x-agentx-sensitive': true } } } }
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      if (path === '/api/v1/applications') return jsonResponse({ items: [application()], page: 1, pageSize: 100, total: 1 })
      if (path === '/api/v1/applications/app-1/deployments') return jsonResponse([incompatible])
      if (path.endsWith('/playground-config')) return jsonResponse(config(null))
      if (path === '/api/v1/applications/app-1/sessions') return jsonResponse([session('session-1', '第一轮会话')])
      if (path === '/gateway/v1/sessions/session-1/messages') return jsonResponse([])
      return jsonResponse([])
    }))
    renderPlayground('/playground?applicationId=app-1&mode=conversation&sessionId=session-1')

    fireEvent.click(await screen.findByRole('button', { name: '参数映射' }))
    expect(screen.getByRole('button', { name: '保存并发布' })).toBeDisabled()
    expect(screen.getAllByText('选择兼容字段')).toHaveLength(2)
  })

  it('shows field names for blank titles and publishes the selected chat mapping', async () => {
    let publishedMapping: unknown = null
    let submitted: unknown
    const untitled = {
      ...deployment(),
      inputSchema: { type: 'object', properties: { question: { type: 'string', title: '' } }, required: ['question'] },
      outputSchema: { type: 'object', properties: { result: { type: 'string', title: '  ' } } },
    }
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      if (path === '/api/v1/applications') return jsonResponse({ items: [application()], page: 1, pageSize: 100, total: 1 })
      if (path === '/api/v1/applications/app-1/deployments') return jsonResponse([untitled])
      if (path.endsWith('/playground-config')) {
        if (init?.method === 'PUT') {
          submitted = JSON.parse(String(init.body))
          publishedMapping = (submitted as { mapping: unknown }).mapping
        }
        return jsonResponse({ ...config(publishedMapping), version: publishedMapping ? 1 : 0, publishedVersion: publishedMapping ? 1 : 0 })
      }
      if (path === '/api/v1/applications/app-1/sessions') return jsonResponse([session('session-1', '第一轮会话')])
      return jsonResponse([])
    }))
    renderPlayground('/playground?applicationId=app-1&mode=conversation&sessionId=session-1')

    expect(await screen.findByText('尚未配置对话映射')).toBeVisible()
    expect(screen.getByRole('button', { name: '发送' })).toBeDisabled()
    fireEvent.click(screen.getByRole('button', { name: '参数映射' }))
    expect(screen.getByRole('combobox', { name: /问题输入/ })).toHaveTextContent('question · string')
    expect(screen.getByRole('combobox', { name: /回答输出/ })).toHaveTextContent('result · string')
    fireEvent.click(screen.getByRole('button', { name: '保存并发布' }))

    expect(await screen.findByText('映射 v1')).toBeVisible()
    expect(submitted).toEqual({ expectedVersion: 0, mapping: { questionInput: 'question', fileInput: null, answerOutput: 'result', answerFilesOutput: null } })
    expect(screen.getByPlaceholderText('输入消息进行测试…')).toBeEnabled()
  })
})

describe('historical parameter projection', () => {
  it('keeps current defaults and ignores removed or type-incompatible fields', () => {
    const schema: JsonSchema = {
      type: 'object',
      properties: {
        prompt: { type: 'string' },
        count: { type: 'integer', default: 2 },
        options: { type: 'object', properties: { active: { type: 'boolean' } } },
      },
    }
    expect(projectHistoricalInput(schema, {
      prompt: '历史问题',
      count: 'old-type',
      options: { active: true, removedNested: 'ignored' },
      removedParameter: 'ignored',
    })).toEqual({ prompt: '历史问题', count: 2, options: { active: true } })
  })
})

function renderPlayground(path: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(<QueryClientProvider client={client}><TooltipPrimitive.Provider><ToastProvider><MemoryRouter initialEntries={[path]}><PlaygroundPage /></MemoryRouter></ToastProvider></TooltipPrimitive.Provider></QueryClientProvider>)
}

function application() { return { id: 'app-1', name: '客服应用', slug: 'support', status: 'active', activeDeploymentId: 'deployment-1' } }
function deployment() { return { id: 'deployment-1', applicationId: 'app-1', workflowVersionId: 'version-1', workflowVersionNumber: 3, sequenceNumber: 2, status: 'active', inputSchema: { type: 'object', properties: { prompt: { type: 'string' } }, required: ['prompt'] }, outputSchema: { type: 'object', properties: { answer: { type: 'string' } }, required: ['answer'] } } }
function config(mapping: unknown) { return { applicationId: 'app-1', deploymentId: 'deployment-1', deploymentVersion: 2, version: 1, mapping, publishedVersion: mapping ? 1 : null, publishStatus: 'active', errorCode: null, errorMessage: null } }

function playgroundFetch(playgroundConfig: unknown) {
  return vi.fn(async (input: RequestInfo | URL) => {
    const path = new URL(String(input), 'http://agentx.test').pathname
    if (path === '/api/v1/applications') return jsonResponse({ items: [application()], page: 1, pageSize: 100, total: 1 })
    if (path === '/api/v1/applications/app-1/deployments') return jsonResponse([deployment()])
    if (path === '/api/v1/applications/app-1/deployments/deployment-1/playground-config') return jsonResponse(playgroundConfig)
    if (path === '/api/v1/applications/app-1/sessions') return jsonResponse([session('session-1', '第一轮会话')])
    if (path === '/gateway/v1/sessions/session-1/messages') return jsonResponse([])
    return jsonResponse([])
  })
}

function session(id: string, title: string) {
  return { id, applicationId: 'app-1', applicationDeploymentId: 'deployment-1', externalUserId: null, status: 'active', title, updatedAt: '2026-08-22T01:00:00Z', version: 1, versionPolicy: 'pinned', workflowVersionId: 'version-1' }
}

function message(id: string, content: string) {
  return { id, role: 'assistant', parts: [{ partType: 'text', content, artifactId: null }], createdAt: '2026-08-22T01:00:00Z' }
}

function jsonResponse(value: unknown) {
  return new Response(JSON.stringify(value), { headers: { 'Content-Type': 'application/json' } })
}
