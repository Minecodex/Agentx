import { readFile } from 'node:fs/promises'

import { expect, type APIResponse, type Page, test } from '@playwright/test'

const password = 'agentx-e2e-admin-password'

type RuntimeContext = { workflowId: string; workflowVersionId: string }
type Environment = { id: string; code: string }
type Deployment = { id: string; status: string }

async function expectResponse(response: APIResponse, label: string) {
  if (!response.ok()) throw new Error(`${label}: ${response.status()} ${await response.text()}`)
}

async function request<T>(page: Page, token: string, path: string, method = 'GET', body?: unknown) {
  const response = await page.request.fetch(`/api/v1${path}`, {
    method,
    data: body,
    headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() },
  })
  await expectResponse(response, `${method} ${path}`)
  const text = await response.text()
  return (text ? JSON.parse(text) : undefined) as T
}

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(password)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

test('deployed Application exposes API key guide and keeps channel endpoint hidden before setup', async ({ page }) => {
  const contextPath = process.env.AGENTX_V2_08_CONTEXT_OUTPUT
  if (!contextPath) throw new Error('AGENTX_V2_08_CONTEXT_OUTPUT is required')
  const context = JSON.parse(await readFile(contextPath, 'utf8')) as RuntimeContext
  const token = await login(page)
  const environment = (await request<Environment[]>(page, token, '/environments')).find((item) => item.code === 'development')
  expect(environment).toBeTruthy()
  const suffix = Date.now()
  const application = await request<{ id: string; slug: string }>(page, token, '/applications', 'POST', {
    workflowId: context.workflowId,
    name: `Integration Docs ${suffix}`,
    slug: `integration-docs-${suffix}`,
    description: 'Deployed application documentation closure',
    visibility: 'company',
  })
  const deployment = await request<Deployment>(page, token, `/applications/${application.id}/deployments`, 'POST', {
    workflowVersionId: context.workflowVersionId,
    environmentId: environment!.id,
    sessionVersionPolicy: 'pinned',
  })
  await expect.poll(async () => {
    const values = await request<Deployment[]>(page, token, `/applications/${application.id}/deployments`)
    const status = values.find((item) => item.id === deployment.id)?.status
    if (status === 'rejected') throw new Error(`application deployment ${deployment.id} was rejected`)
    return status
  }, { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe('active')

  await page.goto(`/applications/${application.id}`)
  await page.getByRole('tab', { name: 'API Keys' }).click()
  await page.getByRole('button', { name: 'API 接入文档' }).click()
  const apiGuide = page.getByRole('dialog', { name: 'API Key 接入文档' })
  await expect(apiGuide.getByText(new RegExp(`/gateway/v1/applications/${application.slug}/invocations$`))).toBeVisible()
  await apiGuide.getByRole('tab', { name: '接口说明' }).click()
  await expect(apiGuide.getByText('responseMode', { exact: true })).toBeVisible()
  for (const language of ['Curl', 'Java', 'Go', 'Node.js', 'Python']) await expect(apiGuide.getByRole('tab', { name: language })).toBeVisible()
  await apiGuide.getByRole('tab', { name: '数据结构' }).click()
  await expect(apiGuide.getByText('message', { exact: true }).first()).toBeVisible()
  await expect(apiGuide.getByText('message', { exact: true })).toHaveCount(2)
  await apiGuide.getByRole('button', { name: '关闭' }).click()

  await page.getByRole('tab', { name: '渠道对接' }).click()
  await expect(page.getByRole('button', { name: 'Webhook 接入文档' })).toHaveCount(0)
  await expect(page.getByText('暂无渠道，请新增一个平台渠道。')).toBeVisible()
})
