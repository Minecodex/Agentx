import { expect, type Page, test } from '@playwright/test'

const password = 'agentx-e2e-admin-password'

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(password)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

async function runFixture(page: Page, name: string) {
  await page.getByRole('link', { name: '工作流', exact: true }).click()
  await page.getByRole('searchbox', { name: '搜索工作流' }).fill(name)
  const row = page.getByRole('row').filter({ hasText: name })
  await expect(row).toBeVisible()
  await row.getByRole('link', { name: '详情' }).click()
  await page.getByRole('button', { name: '测试运行' }).click()
  await expect(page).toHaveURL(/\/executions\/[0-9a-f-]+$/)
  return page.url().split('/').at(-1) as string
}

async function expectExecutionSucceeded(page: Page, timeout: number) {
  await expect(page.locator('header').getByText('成功', { exact: true })).toBeVisible({ timeout })
}

async function executionNodes(page: Page, token: string, execution: string) {
  const response = await page.request.get(`/api/v1/executions/${execution}/nodes`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })
  expect(response.ok()).toBeTruthy()
  return response.json() as Promise<{ items: Array<{ nodeName: string, errorCode?: string, output: { main?: Array<{ json: unknown }> } }> }>
}

async function expectRuntimeTrace(page: Page, token: string, execution: string, _eventType: string, resourceType: string) {
  await expect.poll(async () => {
    const response = await page.request.get(`/api/v1/executions/${execution}/trace`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })
    if (!response.ok()) return null
    const trace = await response.json() as { spans: Array<{ spanKind: string, spanName: string, status: string, resourceType?: string }> }
    return trace.spans.find((span) => span.spanKind === 'runtime_call' && span.resourceType === resourceType) ?? null
  }, { timeout: 30_000 }).toMatchObject({ spanKind: 'runtime_call', status: 'succeeded', resourceType })
}

test('M5 Agent uses the deterministic model and authorized MCP tool with a persistent ledger', async ({ page }) => {
  const token = await login(page)
  const execution = await runFixture(page, 'M5 Agent MCP Fixture')
  await expectExecutionSucceeded(page, 120_000)
  const response = await page.request.get(`/api/v1/executions/${execution}/runtime-details`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })
  expect(response.ok()).toBeTruthy()
  const details = await response.json() as { agentRuns: Array<{ status: string, stopReason: string, modelCallCount: number, toolCallCount: number }>, iterations: unknown[], calls: Array<{ callKind: string, requestFingerprint: string }> }
  expect(details.agentRuns).toHaveLength(1)
  expect(details.agentRuns[0]).toMatchObject({ status: 'succeeded', stopReason: 'stop', modelCallCount: 2, toolCallCount: 1 })
  expect(details.iterations).toHaveLength(0)
  expect(details.calls.map((call) => call.callKind)).toEqual(['model', 'mcp_tool', 'model'])
  expect(details.calls.every((call) => call.requestFingerprint.length === 64)).toBeTruthy()
  await expect(page.getByRole('treegrid', { name: 'Trace 层级瀑布' })).toBeVisible()
  await expect(page.getByRole('row').filter({ hasText: 'Model call' }).first()).toBeVisible()
})

test('M5 Agent repeated tool calls stop before another external request', async ({ page }) => {
  const token = await login(page)
  const execution = await runFixture(page, 'M5 Agent Loop Fixture')
  await expectExecutionSucceeded(page, 120_000)
  const details = await (await page.request.get(`/api/v1/executions/${execution}/runtime-details`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })).json() as { agentRuns: Array<{ status: string, stopReason: string, toolCallCount: number }>, calls: Array<{ callKind: string }> }
  expect(details.agentRuns[0]).toMatchObject({ status: 'failed', stopReason: 'repeated_tool_call', toolCallCount: 2 })
  expect(details.calls.filter((call) => call.callKind === 'mcp_tool')).toHaveLength(2)
})

test('M5 RAG and Memory nodes call the fixed Addons through Worker runtime ports', async ({ page }) => {
  const token = await login(page)

  const ragExecution = await runFixture(page, 'M5 RAG Query Fixture')
  await expectExecutionSucceeded(page, 180_000)
  const ragNodes = await executionNodes(page, token, ragExecution)
  const ragOutput = ragNodes.items.find((node) => node.nodeName === 'M5 RAG Query')?.output.main?.[0].json
  expect(JSON.stringify(ragOutput)).toContain('m5-worker-fixture.txt')
  expect(ragOutput).toMatchObject({ text: expect.any(String), documents: expect.any(Array), citations: expect.any(Array), recordIds: expect.any(Array) })
  expect(ragOutput).not.toHaveProperty('response')
  await expectRuntimeTrace(page, token, ragExecution, 'rag.call', 'rag')

  const memoryExecution = await runFixture(page, 'M5 Memory Search Fixture')
  await expectExecutionSucceeded(page, 180_000)
  const memoryNodes = await executionNodes(page, token, memoryExecution)
  const memoryOutput = memoryNodes.items.find((node) => node.nodeName === 'M5 Memory Search')?.output.main?.[0].json
  expect(JSON.stringify(memoryOutput)).toContain('direct Rust OpenSandbox adapter')
  expect(memoryOutput).toMatchObject({ text: expect.any(String), records: expect.any(Array), recordIds: expect.any(Array) })
  await expectRuntimeTrace(page, token, memoryExecution, 'memory.call', 'memory')
})

test('M5 RAG write with read scope fails before reaching the Addon', async ({ page }) => {
  const token = await login(page)
  const execution = await runFixture(page, 'M5 RAG Read Scope Fixture')
  await expect(page.locator('header').getByText('失败', { exact: true })).toBeVisible({ timeout: 120_000 })
  const nodes = await executionNodes(page, token, execution)
  expect(nodes.items.find((node) => node.nodeName === 'M5 RAG Read Scope')?.errorCode).toBe('RAG_WRITE_DENIED')
  await expect.poll(async () => {
    const response = await page.request.get(`/api/v1/executions/${execution}/trace`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })
    if (!response.ok()) return null
    const trace = await response.json() as { spans: Array<{ spanKind: string, resourceType?: string, errorCode?: string, status: string }> }
    return trace.spans.find((span) => span.spanKind === 'runtime_call' && span.resourceType === 'rag') ?? null
  }, { timeout: 30_000 }).toMatchObject({ status: 'failed', errorCode: 'RAG_WRITE_DENIED' })
})

test('M5 Code runs in OpenSandbox, returns the semantic Code contract, and terminates its lease', async ({ page }) => {
  const token = await login(page)
  const execution = await runFixture(page, 'M5 Code Fixture')
  await expectExecutionSucceeded(page, 180_000)
  const nodes = await (await page.request.get(`/api/v1/executions/${execution}/nodes`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })).json() as { items: Array<{ nodeName: string, output: { main?: Array<{ json: { stdout?: string, stderr?: string, exitCode?: number, structuredOutput?: unknown, files?: unknown[], partial?: boolean } }> } }> }
  const code = nodes.items.find((node) => node.nodeName === 'M5 Python Code')
  expect(code?.output.main?.[0].json.stdout).toContain('m5-sandbox-ok')
  expect(code?.output.main?.[0].json).toMatchObject({ stderr: expect.any(String), exitCode: 0, files: expect.any(Array), partial: false })
  const details = await (await page.request.get(`/api/v1/executions/${execution}/runtime-details`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })).json() as { sandboxes: Array<{ status: string, sandboxId: string, terminationAttempts: number, expiresAt: string }> }
  expect(details.sandboxes).toHaveLength(1)
  expect(details.sandboxes[0]).toMatchObject({ status: 'terminated', terminationAttempts: 0 })
  expect(details.sandboxes[0].sandboxId).toBeTruthy()
  expect(Number.isNaN(Date.parse(details.sandboxes[0].expiresAt))).toBe(false)
})

test('M5 command runners, Credential files, and network policies use isolated Sandboxes', async ({ page }) => {
  const token = await login(page)
  const fixtures = [
    ['M5 JavaScript Code Fixture', 'M5 JavaScript Code', 'm5-javascript-ok', true],
    ['M5 Shell Code Fixture', 'M5 Shell Code', 'm5-shell-ok', true],
    ['M5 Browser Code Fixture', 'M5 Browser Code', 'm5-browser-title:M5 Browser', true],
    ['M5 Credential Code Fixture', 'M5 Credential Code', 'm5-credential-ok', false],
    ['M5 Network Deny Fixture', 'M5 Network Deny', 'm5-network-denied', false],
    ['M5 Network Allow Fixture', 'M5 Network Allow', 'm5-network-allowed', false],
  ] as const
  for (const [workflow, nodeName, stdout, hasArtifact] of fixtures) {
    const execution = await runFixture(page, workflow)
    await expectExecutionSucceeded(page, 180_000)
    const nodes = await (await page.request.get(`/api/v1/executions/${execution}/nodes`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })).json() as { items: Array<{ nodeName: string, output: { main?: Array<{ json: { stdout?: string, files?: unknown[] } }> } }> }
    const code = nodes.items.find((node) => node.nodeName === nodeName)
    expect(code?.output.main?.[0].json.stdout).toContain(stdout)
    if (workflow === 'M5 Network Deny Fixture') {
      expect(code?.output.main?.[0].json.stdout).toContain('m5-ipv4-denied')
      expect(code?.output.main?.[0].json.stdout).toContain('m5-ipv6-denied')
    }
    expect(code?.output.main?.[0].json.stdout).not.toContain('m5-model-secret')
    if (hasArtifact) expect(code?.output.main?.[0].json.files).toEqual(expect.any(Array))
  }
})

test('M5 partial Sandbox output is bounded and linked to full Trace Artifacts', async ({ page }) => {
  const token = await login(page)
  const execution = await runFixture(page, 'M5 Partial Output Fixture')
  await expectExecutionSucceeded(page, 180_000)
  const nodes = await (await page.request.get(`/api/v1/executions/${execution}/nodes`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })).json() as { items: Array<{ nodeName: string, output: { main?: Array<{ json: { stdout?: string, stderr?: string, files?: Array<{ artifactId: string, fileName: string, contentType: string, sizeBytes: number, sha256: string }>, partial?: boolean } }> } }> }
  const output = nodes.items.find((node) => node.nodeName === 'M5 Partial Output Code')?.output.main?.[0]
  expect(output?.json.partial).toBe(true)
  expect(output?.json.stdout?.length).toBeLessThanOrEqual(32)
  expect(output?.json.stderr?.length).toBeLessThanOrEqual(32)
  expect(output?.json.files).toHaveLength(2)
  for (const file of output?.json.files ?? []) {
    expect(file).toMatchObject({ artifactId: expect.stringMatching(/^[0-9a-f-]{36}$/), fileName: expect.any(String), contentType: expect.any(String), sizeBytes: expect.any(Number), sha256: expect.stringMatching(/^[0-9a-f]{64}$/) })
  }

  await expect.poll(async () => {
    const response = await page.request.get(`/api/v1/executions/${execution}/trace`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })
    if (!response.ok()) return null
    const trace = await response.json() as { spans: Array<{ spanId: string, spanKind: string }> }
    const sandbox = trace.spans.find((span) => span.spanKind === 'sandbox')
    if (!sandbox) return null
    const detail = await page.request.get(`/api/v1/executions/${execution}/trace/spans/${sandbox.spanId}`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })
    return detail.ok() ? detail.json() : null
  }, { timeout: 30_000 }).toMatchObject({ span: { spanKind: 'sandbox', status: 'succeeded' } })
})
