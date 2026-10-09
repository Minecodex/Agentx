import { expect, type Page, test } from '@playwright/test'

import { useRuntimePortForward } from './playground-helpers'

const networkError = '无法连接服务。请检查网络、服务地址或浏览器的安全提示后重试。'

async function api(page: Page, token: string, path: string, method = 'GET', data?: unknown) {
  const response = await page.request.fetch(`/api/v1${path}`, { method, data, headers: { Authorization: `Bearer ${token}` } })
  expect(response.ok(), `${method} ${path}: ${await response.text()}`).toBeTruthy()
  return response.json()
}

test('publishes untitled chat fields and explains blocked parameter and session requests', async ({ page }, testInfo) => {
  await useRuntimePortForward(page)
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill('agentx-e2e-admin-password')
  const loggedIn = page.waitForResponse((response) => response.url().endsWith('/api/v1/auth/login') && response.request().method() === 'POST')
  await page.getByRole('button', { name: '登录', exact: true }).click()
  const token = (await (await loggedIn).json()).accessToken
  await expect(page).toHaveURL(/\/$/)

  const runId = process.env.AGENTX_E2E_RUN_ID ?? String(Date.now())
  const workflow = await api(page, token, '/workflows', 'POST', { name: `Playground errors ${runId}`, visibility: 'company' })
  const draft = await api(page, token, `/workflows/${workflow.id}/draft`)
  const definition = draft.definition
  definition.start.inputs = { type: 'object', properties: { question: { type: 'string', title: '' } }, required: ['question'], additionalProperties: false }
  const exit = definition.nodes.find((node: { type: string }) => node.type === 'exit')
  expect(exit).toBeTruthy()
  exit.parameters.outputs = { result: { kind: 'reference', selector: { namespace: 'inputs', run: { kind: 'current' }, item: { kind: 'current' }, path: ['question'] }, missingPolicy: { kind: 'error' } } }
  definition.end.outputs = { result: { schema: { type: 'string', title: '  ' }, required: true, sensitive: false } }
  definition.connections = [{ id: 'start-exit', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: exit.id, targetHandle: 'main', order: 0 }]
  await api(page, token, `/workflows/${workflow.id}/draft`, 'PUT', { expectedRevision: draft.revision, definition })
  const latest = await api(page, token, `/workflows/${workflow.id}/draft`)
  const version = await api(page, token, `/workflows/${workflow.id}/versions`, 'POST', { draftRevision: latest.revision })
  const environments = await api(page, token, '/environments')
  const environmentId = environments.find((item: { code: string }) => item.code === 'development').id
  await api(page, token, `/workflows/${workflow.id}/deployments`, 'POST', { environmentId, workflowVersionId: version.id })
  const application = await api(page, token, '/applications', 'POST', { name: `Playground errors ${runId}`, slug: `playground-${runId}`, workflowId: workflow.id, visibility: 'company' })
  const deployment = await api(page, token, `/applications/${application.id}/deployments`, 'POST', { environmentId, workflowVersionId: version.id, sessionVersionPolicy: 'pinned' })

  await expect.poll(async () => {
    const deployments = await api(page, token, `/applications/${application.id}/deployments`)
    return deployments.find((item: { id: string }) => item.id === deployment.id)?.status
  }, { timeout: 60_000 }).toBe('active')
  await page.goto(`/playground?applicationId=${application.id}&mode=conversation`)
  await expect(page.getByText('尚未配置对话映射', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: '参数映射', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '对话参数映射', exact: true })
  await expect(dialog.getByRole('combobox', { name: /问题输入/ })).toHaveText('question · string')
  await expect(dialog.getByRole('combobox', { name: /回答输出/ })).toHaveText('result · string')
  await page.screenshot({ path: testInfo.outputPath('playground-mapping-fields.png'), fullPage: true })
  const mappingSaved = page.waitForResponse((response) => response.url().endsWith('/playground-config') && response.request().method() === 'PUT')
  await dialog.getByRole('button', { name: '保存并发布', exact: true }).click()
  const mappingResponse = await mappingSaved
  expect(mappingResponse.status()).toBe(202)
  expect((await mappingResponse.json()).mapping).toEqual({ questionInput: 'question', fileInput: null, answerOutput: 'result', answerFilesOutput: null })
  await expect(page.getByText('映射 v1', { exact: true })).toBeVisible({ timeout: 60_000 })

  await page.getByRole('button', { name: '新建会话', exact: true }).click()
  const composer = page.getByPlaceholder('输入消息进行测试…')
  await expect(composer).toBeEnabled()
  await composer.fill('playground mapping acceptance')
  await page.getByRole('button', { name: '发送', exact: true }).click()
  await expect(page.getByText('playground mapping acceptance', { exact: true })).toHaveCount(2, { timeout: 60_000 })
  await expect(page.getByText('已完成', { exact: true })).toBeVisible()

  const blockedInvocations = `**/gateway/v1/applications/${application.slug}/invocations`
  await page.route(blockedInvocations, (route) => route.abort('failed'))
  await page.getByRole('tab', { name: '参数测试', exact: true }).click()
  await page.getByLabel('Question', { exact: true }).fill('blocked parameter request')
  await page.getByRole('button', { name: '运行测试', exact: true }).click()
  await expect(page.getByText(networkError, { exact: true })).toBeVisible()
  await expect(page.getByText(/请求 ID/)).toHaveCount(0)
  await expect(page.getByText('还没有参数测试记录', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: '运行测试', exact: true })).toBeEnabled()
  await page.screenshot({ path: testInfo.outputPath('playground-parameter-network-error.png'), fullPage: true })
  await page.unroute(blockedInvocations)

  const blockedSessions = `**/gateway/v1/applications/${application.slug}/sessions`
  await page.route(blockedSessions, (route) => route.abort('failed'))
  await page.getByRole('tab', { name: '对话测试', exact: true }).click()
  await page.getByRole('button', { name: '新建会话', exact: true }).click()
  await expect(page.getByText(networkError, { exact: true })).toBeVisible()
  await expect(page.getByText(/请求 ID/)).toHaveCount(0)
  await page.screenshot({ path: testInfo.outputPath('playground-session-network-error.png'), fullPage: true })
  await page.unroute(blockedSessions)
})
