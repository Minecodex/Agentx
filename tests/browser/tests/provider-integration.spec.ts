import { expect, type Locator, type Page, test } from '@playwright/test'
import { fillMonaco, showEdgeToolbar } from './workflow-studio-editor-helpers'

const password = 'agentx-e2e-admin-password'
const company = 'Agentx E2E'
const echoBaseUrl = process.env.AGENTX_E2E_ECHO_BASE_URL ?? 'http://echo-mcp:8090'
const lightRagBaseUrl = process.env.AGENTX_E2E_LIGHTRAG_BASE_URL ?? 'http://lightrag:9621'
const mem0BaseUrl = process.env.AGENTX_E2E_MEM0_BASE_URL ?? 'http://mem0:8000'
const ragflowBaseUrl = process.env.AGENTX_E2E_RAGFLOW_BASE_URL ?? ''
const ragflowRejectedBaseUrl = process.env.AGENTX_E2E_RAGFLOW_REJECTED_BASE_URL ?? 'http://host.docker.internal:19380'
const ragflowAliasBaseUrl = process.env.AGENTX_E2E_RAGFLOW_ALIAS_BASE_URL ?? ''
const sandboxImage = process.env.AGENTX_E2E_SANDBOX_IMAGE ?? 'opensandbox/code-interpreter@sha256:64cd01f03f54ba347d1a1310dcbc18ac5cb17d01714e23b4ea4b840fbb0d6623'
const sandboxUiCreate = process.env.AGENTX_E2E_SANDBOX_UI_CREATE === '1'
const providerCredentialName = 'Provider Integration Credential'
const providerRagKeyName = 'Provider Integration LightRAG Key'
const ragflowCredentialName = 'Provider Integration RAGFlow Key'
const lightRagApiKey = process.env.AGENTX_E2E_LIGHTRAG_API_KEY ?? 'agentx-v2-04-rag-key'
const providerModelAlias = 'provider-integration-model'

type Execution = { id: string; status: string; errorCode?: string | null }
type PageResponse<T> = { items: T[] }
type NamedResource = { id: string; name?: string; alias?: string }
type StudioDraft = {
  revision: number
  definition: {
    nodes: Array<{ id: string, key: string, type: string, name: string, resourceReferences: Array<{ bindingRole?: string, resourceType?: string, resourceVersionId?: string }> }>
    connections: Array<{ sourceNodeId: string, sourceHandle: string }>
  }
}

async function login(page: Page) {
  const bootstrapStatus = await page.request.get('/api/v1/bootstrap/status')
  if (!bootstrapStatus.ok()) throw new Error(`bootstrap status: ${bootstrapStatus.status()} ${await bootstrapStatus.text()}`)
  if (((await bootstrapStatus.json()) as { required: boolean }).required) {
    await page.goto('/setup')
    await page.getByLabel('公司名称').fill(company)
    await page.getByLabel('用户名').fill('admin')
    await page.getByLabel('管理员姓名').fill('E2E Admin')
    await page.getByLabel('密码').fill(password)
    const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/bootstrap') && value.request().method() === 'POST')
    await page.getByRole('button', { name: '初始化并进入工作台' }).click()
    await expect(page).toHaveURL(/\/$/)
    return ((await (await response).json()) as { accessToken: string }).accessToken
  }
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(password)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

async function api<T>(page: Page, token: string, path: string): Promise<T> {
  const response = await page.request.get(`/api/v1${path}`, { headers: { Authorization: `Bearer ${token}` } })
  if (!response.ok()) throw new Error(`${path}: ${response.status()} ${await response.text()}`)
  return response.json() as Promise<T>
}

async function tokenAlive(page: Page, token: string) {
  const response = await page.request.get('/api/v1/auth/me', { headers: { Authorization: `Bearer ${token}` } })
  return response.ok()
}

async function freshApiToken(page: Page, token: string) {
  if (await tokenAlive(page, token)) return token
  const response = await page.request.post('/api/v1/auth/login', { data: { username: 'admin', password } })
  if (!response.ok()) throw new Error(`re-login: ${response.status()} ${await response.text()}`)
  return ((await response.json()) as { accessToken: string }).accessToken
}

async function ensureFreshEditorSession(page: Page, token: string, workflowId: string) {
  if (await tokenAlive(page, token)) return token
  const fresh = await login(page)
  await page.goto(`/workflows/${workflowId}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible({ timeout: 60_000 })
  return fresh
}

async function mutate<T>(page: Page, token: string, path: string, method: 'POST' | 'PUT' | 'PATCH', body: unknown): Promise<T> {
  const response = await page.request.fetch(`/api/v1${path}`, { method, data: body, headers: { Authorization: `Bearer ${token}` } })
  if (!response.ok()) throw new Error(`${path}: ${response.status()} ${await response.text()}`)
  return response.json() as Promise<T>
}

async function ensureModelResources(page: Page, token: string) {
  const departments = await api<Array<{ id: string; isRoot: boolean }>>(page, token, '/departments')
  const department = departments.find((item) => item.isRoot) ?? departments[0]
  if (!department) throw new Error('provider integration requires a Department created through bootstrap')
  const credentials = await api<PageResponse<NamedResource>>(page, token, `/credentials?pageSize=100&search=${encodeURIComponent(providerCredentialName)}`)
  const credential = credentials.items.find((item) => item.name === providerCredentialName)
    ?? await mutate<NamedResource>(page, token, '/credentials', 'POST', {
      name: providerCredentialName,
      credentialType: 'bearer',
      secret: 'm5-model-secret',
      ownerDepartmentId: department.id,
    })
  const ragCredentials = await api<PageResponse<NamedResource>>(page, token, `/credentials?pageSize=100&search=${encodeURIComponent(providerRagKeyName)}`)
  const ragCredential = ragCredentials.items.find((item) => item.name === providerRagKeyName)
    ?? await mutate<NamedResource>(page, token, '/credentials', 'POST', {
      name: providerRagKeyName,
      credentialType: 'bearer',
      secret: lightRagApiKey,
      ownerDepartmentId: department.id,
    })
  void ragCredential
  const ragflowCredentials = await api<PageResponse<NamedResource>>(page, token, `/credentials?pageSize=100&search=${encodeURIComponent(ragflowCredentialName)}`)
  const ragflowCredential = ragflowCredentials.items.find((item) => item.name === ragflowCredentialName)
    ?? await mutate<NamedResource>(page, token, '/credentials', 'POST', {
      name: ragflowCredentialName,
      credentialType: 'bearer',
      secret: process.env.AGENTX_E2E_RAGFLOW_API_KEY ?? 'ragflow-invalid-key',
      ownerDepartmentId: department.id,
    })
  void ragflowCredential
  const models = await api<PageResponse<NamedResource>>(page, token, `/models/aliases?pageSize=100&search=${encodeURIComponent(providerModelAlias)}`)
  if (!models.items.some((item) => item.alias === providerModelAlias)) {
    await mutate(page, token, '/models/aliases', 'POST', {
      connectionName: 'Provider Integration Model',
      providerType: 'openai_compatible',
      endpoint: `${echoBaseUrl}/v1`,
      credentialId: credential.id,
      ownerDepartmentId: department.id,
      alias: providerModelAlias,
      modelName: 'echo-model-v1',
      maxInputTokens: 8192,
      maxOutputTokens: 2048,
      defaultParameters: { temperature: 0 },
      price: { currency: 'USD', inputPerMillion: '5', outputPerMillion: '30' },
    })
  }
  return department
}

async function dialog(page: Page, title: string) {
  const value = page.getByRole('dialog', { name: title })
  await expect(value).toBeVisible()
  return value
}

async function field(container: Locator, label: string) {
  return container.locator('label').filter({ hasText: label }).locator('input, textarea').first()
}

async function select(container: Locator, label: string, option: string) {
  const direct = container.getByRole('combobox', { name: label, exact: true })
  const target = (await direct.count()) > 0
    ? direct
    : container.locator('label').filter({ hasText: label }).getByRole('combobox').first()
  await expect(target).toBeVisible({ timeout: 20_000 })
  await target.click()
  const escaped = option.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const exact = container.page().getByRole('option', {
    name: new RegExp(`^\\s*${escaped}(?:\\s|$)`, 'i'),
  }).last()
  await expect(exact, `Option ${option} was not available for ${label}`).toBeVisible({ timeout: 5_000 })
  await exact.click()
}

async function submit(container: Locator, name: string) {
  await container.getByRole('button', { name, exact: true }).click()
  await expect(container).toBeHidden()
}

async function createKnowledgeViaUi(page: Page, connectionName: string, endpoint: string, healthPath: string, resourceName: string, externalId: string, credentialName?: string, provider?: 'lightrag' | 'ragflow') {
  await page.getByRole('link', { name: '知识库' }).click()
  await page.getByRole('button', { name: '接入知识库' }).click()
  const form = await dialog(page, '接入知识库')
  await (await field(form, '连接名称')).fill(connectionName)
  if (provider === 'ragflow') await select(form, '服务协议', 'RAGFlow')
  await (await field(form, 'Endpoint')).fill(endpoint)
  if (healthPath !== '/health') await (await field(form, '健康检查路径')).fill(healthPath)
  if (credentialName) await select(form, '凭证', credentialName)
  await select(form, '所属部门', company)
  await (await field(form, '资源名称')).fill(resourceName)
  await (await field(form, '外部资源 ID')).fill(externalId)
  await submit(form, '保存')
}

async function createMemoryViaUi(page: Page, connectionName: string, endpoint: string, healthPath: string, resourceName: string, namespace: string) {
  await page.getByRole('link', { name: '记忆' }).click()
  await page.getByRole('button', { name: '接入记忆服务' }).click()
  const form = await dialog(page, '接入记忆服务')
  await (await field(form, '连接名称')).fill(connectionName)
  await (await field(form, 'Endpoint')).fill(endpoint)
  await (await field(form, '健康检查路径')).fill(healthPath)
  await select(form, '所属部门', company)
  await (await field(form, '资源名称')).fill(resourceName)
  await (await field(form, '记忆命名空间')).fill(namespace)
  await submit(form, '保存')
}

async function openResourceDetail(page: Page, navName: string, resourceName: string) {
  await page.getByRole('link', { name: navName, exact: true }).click()
  const row = page.getByRole('row').filter({ hasText: resourceName })
  await expect(row).toBeVisible()
  await row.getByRole('link', { name: '详情' }).click()
  await expect(page).toHaveURL(new RegExp(`/${navName === '知识库' ? 'knowledge' : 'memory'}/[0-9a-f-]+$`))
}

async function verifyConnectionTest(page: Page, navName: string, resourceName: string) {
  await openResourceDetail(page, navName, resourceName)
  await page.getByRole('button', { name: '测试连接' }).click()
  await expect(page.getByText(/连接正常 · \d+ ms/)).toBeVisible({ timeout: 30_000 })
  await page.screenshot({ path: `provider-${navName === '知识库' ? 'knowledge' : 'memory'}-connection-test-${Date.now()}.png`, fullPage: true })
}

async function createSandboxViaUi(page: Page, name: string) {
  await page.getByRole('link', { name: '沙箱配置' }).click()
  await page.getByRole('button', { name: '新建沙箱配置' }).click()
  const form = await dialog(page, '新建沙箱配置')
  await (await field(form, '名称')).fill(name)
  await select(form, '所属部门', company)
  await select(form, 'Runner', 'python')
  await (await field(form, '镜像 Digest')).fill(sandboxImage)
  await (await field(form, 'CPU（milliCPU，1000 = 1 核）')).fill('500')
  await (await field(form, '内存（GB）')).fill('0.5')
  await (await field(form, '进程数（个）')).fill('256')
  await (await field(form, '磁盘（GB）')).fill('1')
  await (await field(form, '最长运行时间（秒）')).fill('180')
  await (await field(form, '输出上限（KB）')).fill('1024')
  await submit(form, '保存')
  await expect(page.getByRole('row').filter({ hasText: name })).toBeVisible()
}

async function ensureSandboxProfile(page: Page, token: string, departmentId: string, name: string) {
  if (sandboxUiCreate) {
    await createSandboxViaUi(page, name)
    return
  }
  const sandboxes = await api<PageResponse<NamedResource>>(page, token, '/sandbox-profiles?pageSize=100')
  if (!sandboxes.items.some((item) => item.name === name)) {
    await mutate(page, token, '/sandbox-profiles', 'POST', {
      name,
      description: 'Provider integration OpenSandbox fixture',
      ownerDepartmentId: departmentId,
      runner: 'python',
      imageDigest: sandboxImage,
      cpuMillis: 500,
      memoryBytes: 536870912,
      pidsLimit: 256,
      diskBytes: 1073741824,
      timeoutSeconds: 180,
      outputLimitBytes: 1048576,
      networkPolicy: { defaultAction: 'deny', egressMode: 'none' },
    })
  }
}

async function createWorkflow(page: Page, name: string) {
  await page.getByRole('link', { name: '工作流', exact: true }).click()
  await page.getByRole('button', { name: '新建工作流' }).click()
  const dialogBox = page.getByRole('dialog', { name: '新建工作流' })
  await dialogBox.getByLabel('工作流名称').fill(name)
  await dialogBox.getByLabel('描述').fill('Provider integration Kubernetes E2E')
  await dialogBox.getByRole('button', { name: '保存' }).click()
  await expect(page).toHaveURL(/\/workflows\/[0-9a-f-]+$/)
  return page.url().split('/').at(-1) as string
}

const resourceTabs = { credential: '凭证', model: '模型', sandbox_profile: '沙箱配置', rag: '知识库', memory: '记忆' } as const

async function grantResource(page: Page, resourceType: keyof typeof resourceTabs, resourceName: string, workflowName: string) {
  await page.goto('/resource-grants')
  await page.getByRole('tab', { name: resourceTabs[resourceType], exact: true }).click()
  const search = page.getByRole('searchbox', { name: '搜索资源名称、类型或连接信息' })
  await search.fill(resourceName)
  const row = page.getByRole('row').filter({ hasText: resourceName }).first()
  await expect(row).toBeVisible()
  await row.getByRole('button', { name: '管理授权' }).click()
  const grantDialog = page.getByRole('dialog').filter({ hasText: resourceName })
  await expect(grantDialog).toBeVisible()
  const subject = grantDialog.getByRole('combobox').nth(1)
  await subject.click()
  await page.getByRole('option', { name: workflowName, exact: true }).click()
  const revokeButtons = grantDialog.getByRole('button', { name: '撤销授权' })
  const grantCount = await revokeButtons.count()
  await grantDialog.getByRole('button', { name: '添加授权' }).click()
  await expect(revokeButtons).toHaveCount(grantCount + 1)
  await grantDialog.getByRole('button', { name: '取消' }).click()
}

function flowNode(page: Page, role: string) {
  const labels: Record<string, RegExp> = { agent: /智能体|Agent/, code: /代码|Code/ }
  return page.locator('.react-flow__node').filter({ hasText: labels[role] ?? new RegExp(role, 'i') }).first()
}

async function addFromCreator(page: Page, testId: string) {
  const initialEdge = page.locator('.react-flow__edge[data-testid="rf__edge-start-exit"]')
  if (await page.locator('.react-flow__node-manifest').count() === 0) {
    if (await initialEdge.count() === 0) {
      await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
    }
    await expect(initialEdge).toHaveCount(1)
    const toolbar = page.locator('.studio-edge-toolbar[data-edge-id="start-exit"]')
    await showEdgeToolbar(initialEdge, toolbar)
    await toolbar.getByRole('button', { name: /删除连线|Delete connection/ }).dispatchEvent('click')
    await expect(initialEdge).toHaveCount(0)
  }
  await expect(page.getByTestId('node-creator')).toBeVisible()
  await page.getByRole('textbox', { name: '搜索节点' }).fill(testId.replace(/^palette-(action|binding)-/, ''))
  await page.getByTestId(testId).click()
  await expect(page.getByTestId('node-creator')).toBeVisible()
}

async function openNodeDetails(page: Page, node: Locator) {
  const details = page.getByTestId('node-details-view')
  if (await details.isVisible()) await details.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
  await node.dispatchEvent('click')
  await expect(details).toBeVisible()
  return details
}

async function openNodeBasics(details: Locator) {
  const basics = details.getByTestId('node-basics')
  if (await basics.getAttribute('open') === null) await basics.locator('summary').click()
  await expect(basics).toHaveAttribute('open', '')
}

async function choose(page: Page, scope: Locator, option: string | RegExp) {
  const trigger = await scope.getAttribute('role') === 'combobox' ? scope : scope.getByRole('combobox')
  await trigger.click()
  await page.getByRole('option', { name: option }).click()
}

async function fillNativeText(page: Page, scope: Locator, value: string, index = 0) {
  await scope.scrollIntoViewIfNeeded()
  const editor = scope.getByRole('textbox').nth(index)
  await expect(editor).toBeVisible()
  await editor.fill(value)
  if (await editor.getAttribute('contenteditable') === 'true') await expect(editor).toHaveText(value)
  else await expect(editor).toHaveValue(value)
  await page.keyboard.press('Escape')
}

async function chooseReference(page: Page, scope: Locator, namespace: RegExp, labels: string[]) {
  const editor = scope.getByRole('textbox', { name: 'Value' }).first()
  await editor.click()
  const picker = page.getByTestId('reference-picker')
  await expect(picker).toBeVisible()
  await picker.getByRole('button', { name: namespace }).click()
  const referenceTree = picker.locator(':scope > div').nth(1)
  for (const [index, label] of labels.entries()) {
    const row = referenceTree.getByRole('button', { name: new RegExp(label.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'i') }).first()
    if (index === 0 && await row.count() === 0) continue
    await expect(row).toBeVisible()
    const toggle = row.locator('[data-tree-toggle]')
    if (index < labels.length - 1 && await toggle.count()) await toggle.click()
    else await row.click()
  }
  await expect(picker).toBeHidden()
}

async function connect(page: Page, source: Locator, sourceHandle: string, target: Locator, targetHandle: string) {
  const edges = page.locator('.react-flow__edge')
  const edgeCount = await edges.count()
  const from = source.locator(`.react-flow__handle.source[data-handleid="${sourceHandle}"]`)
  const to = target.locator(`.react-flow__handle.target[data-handleid="${targetHandle}"]`)
  await expect(from).toBeVisible()
  await expect(to).toBeVisible()
  for (let attempt = 0; attempt < 3 && await edges.count() === edgeCount; attempt += 1) {
    let boxes: Awaited<ReturnType<Locator['boundingBox']>>[] | undefined
    await expect.poll(async () => {
      const next = await Promise.all([from.boundingBox(), to.boundingBox()])
      if (next.some((box) => box === null)) return false
      boxes = next
      return true
    }).toBeTruthy()
    const [fromBox, toBox] = boxes!
    await page.mouse.move(fromBox!.x + fromBox!.width / 2, fromBox!.y + fromBox!.height / 2)
    await page.mouse.down()
    await page.mouse.move(toBox!.x + toBox!.width / 2, toBox!.y + toBox!.height / 2, { steps: 20 })
    await page.waitForTimeout(125)
    await page.mouse.up()
    await page.waitForTimeout(200)
  }
  await expect(edges).toHaveCount(edgeCount + 1)
}

async function setStartQuestionInput(page: Page) {
  await page.getByTestId('workflow-start').click()
  const panel = page.getByTestId('start-panel')
  await expect(panel).toBeVisible()
  await panel.getByRole('button', { name: /添加字段|Add field/ }).first().click()
  const dialogBox = page.getByRole('dialog', { name: /添加字段|Add field/ })
  const fieldName = dialogBox.getByLabel(/字段名|Field name/)
  await fieldName.fill('question')
  await fieldName.blur()
  await dialogBox.getByLabel(/标题|Title/).fill('Question')
  await dialogBox.getByLabel(/必填|Required/).check()
  await dialogBox.getByRole('button', { name: /保存|Save/ }).click()
  await panel.getByRole('button', { name: /^(关闭|Close)$/ }).click()
}

async function saveAndReadDraft(page: Page, token: string, workflowId: string) {
  const save = page.getByRole('button', { name: '保存', exact: true })
  if (await save.isEnabled()) {
    const responsePromise = page.waitForResponse((response) => response.url().endsWith(`/api/v1/workflows/${workflowId}/draft`) && response.request().method() === 'PUT')
    await save.click()
    const response = await responsePromise
    if (!response.ok()) throw new Error(`save workflow draft: ${response.status()} ${await response.text()}`)
  }
  await expect(page.locator('header').getByText(/修订号 \d+ · 已保存/)).toBeVisible({ timeout: 30_000 })
  return api<StudioDraft>(page, token, `/workflows/${workflowId}/draft`)
}

async function startDebug(page: Page, action: () => Promise<void>, question = 'Run the provider integration workflow.') {
  page.once('dialog', (dialogBox) => dialogBox.accept())
  const response = page.waitForResponse((value) => value.url().includes('/debug-executions') && value.request().method() === 'POST', { timeout: 60_000 })
  const parameters = page.getByRole('dialog', { name: /运行工作流|调试输入|Run workflow|Debug input/ })
  await action()
  const submitParameters = async (timeout: number) => {
    const visible = await parameters.waitFor({ state: 'visible', timeout }).then(() => true).catch(() => false)
    if (!visible) return false
    const questionField = parameters.getByLabel(/Question|问题/)
    if (await questionField.isVisible().catch(() => false)) await questionField.fill(question)
    else {
      const input = parameters.getByRole('textbox', { name: /输入|Input/ })
      if (await input.isVisible().catch(() => false)) await input.fill(`{"question":${JSON.stringify(question)}}`)
    }
    await parameters.getByRole('button', { name: /^(开始运行|运行|Run workflow|Run)$/ }).click()
    return true
  }
  const submitted = await submitParameters(10_000)
  let accepted = submitted ? undefined : await Promise.race([response, page.waitForTimeout(2_000).then(() => undefined)])
  if (!submitted && !accepted && await page.locator('header').getByRole('button', { name: '运行', exact: true }).isVisible().catch(() => false)) {
    await action()
    await submitParameters(10_000)
  }
  accepted ??= await response
  if (accepted.status() !== 202) throw new Error(`debug execution: ${accepted.status()} ${await accepted.text()}`)
  return (await accepted.json() as { executionId: string }).executionId
}

async function waitExecution(page: Page, token: string, executionId: string, statuses: string[], timeout = 240_000) {
  let current: Execution | undefined
  await expect.poll(async () => {
    try {
      current = await api<Execution>(page, token, `/executions/${executionId}`)
    } catch (error) {
      if (error instanceof Error && (error.message.includes(': 404 ') || error.message.includes(': 503 '))) return false
      throw error
    }
    if (['succeeded', 'failed', 'cancelled', 'timed_out'].includes(current.status) && !statuses.includes(current.status)) return current.status
    return statuses.includes(current.status)
  }, { timeout, intervals: [500, 750, 1000, 2000] }).toBeTruthy()
  return current!
}

type TraceSpan = { spanId: string, spanKind: string, spanName: string, status: string, resourceType?: string, errorCode?: string | null }

async function fetchTrace(page: Page, token: string, executionId: string) {
  const response = await page.request.get(`/api/v1/executions/${executionId}/trace?limit=200`, { headers: { Authorization: `Bearer ${token}` } })
  if (!response.ok()) throw new Error(`trace: ${response.status()} ${await response.text()}`)
  return (await response.json()) as { complete: boolean, spans: TraceSpan[] }
}

async function expectRuntimeTrace(page: Page, token: string, executionId: string, resourceType: string, status: 'succeeded' | 'failed') {
  let span: TraceSpan | undefined
  await expect.poll(async () => {
    const trace = await fetchTrace(page, token, executionId)
    span = trace.spans.find((item) => item.spanKind === 'runtime_call' && item.resourceType === resourceType)
    return span?.status ?? null
  }, { timeout: 60_000 }).toBe(status)
  return span!
}

async function buildAgentWorkflow(page: Page, token: string, workflowId: string, options: { knowledgeName: string, memoryName: string | null, sandboxName: string | null }) {
  await page.goto(`/workflows/${workflowId}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  await setStartQuestionInput(page)
  await addFromCreator(page, 'palette-action-agent')
  await expect(page.locator('.react-flow__edge')).toHaveCount(0)
  if (options.sandboxName) await addFromCreator(page, 'palette-action-code')
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })

  const agent = flowNode(page, 'agent')
  const agentDetails = await openNodeDetails(page, agent)
  await choose(page, agentDetails.getByTestId('agent-inspector-model'), new RegExp(providerModelAlias))
  await choose(page, agentDetails.getByTestId('agent-inspector-knowledge'), new RegExp(options.knowledgeName))
  if (options.memoryName) await choose(page, agentDetails.getByTestId('agent-inspector-long_term_memory'), new RegExp(options.memoryName))
  const sessionPolicy = agentDetails.locator('[data-field-path="parameters.sessionPolicy"]')
  await choose(page, sessionPolicy, /仅本次调用|Invocation only/)
  await fillNativeText(page, agentDetails.getByTestId('parameter-systemPrompt'), 'Use the attached knowledge and memory, then answer briefly.')
  await chooseReference(page, agentDetails.getByTestId('parameter-userQuestion'), /输入|Inputs/, ['question'])

  let codeKey: string | null = null
  if (options.sandboxName) {
    const code = flowNode(page, 'code')
    const codeDetails = await openNodeDetails(page, code)
    await openNodeBasics(codeDetails)
    const codeName = codeDetails.locator('[data-field-path="name"] input')
    await codeName.fill('Provider Sandbox Code')
    await codeName.blur()
    await choose(page, page.getByTestId('parameter-runner'), /^Python$/)
    const codeInputs = codeDetails.getByTestId('parameter-inputs')
    await codeInputs.getByRole('button', { name: /添加字段|Add field/ }).click()
    const row = codeInputs.getByTestId('mapper-control').locator(':scope > div').last()
    const key = row.getByRole('textbox', { name: /键|Key/ })
    await key.fill('message')
    await key.blur()
    await chooseReference(page, row, /输入|Inputs/, ['question'])
    await fillMonaco(page, page.getByTestId('parameter-source'), 'def main(**inputs):\n    return {"message": inputs["message"], "sandbox": "opensandbox"}')
    await fillMonaco(page, page.getByTestId('code-output-example'), '{ message: "", sandbox: "" }')
    await choose(page, page.getByTestId('resource-selector-sandbox_profile'), new RegExp(options.sandboxName))
  }

  await page.getByTestId('node-details-view').getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  await connect(page, page.getByTestId('workflow-start'), 'main', agent, 'main')
  if (options.sandboxName) {
    const code = flowNode(page, 'code')
    await connect(page, agent, 'main', code, 'main')
    await connect(page, code, 'main', page.getByTestId('exit-node-exit'), 'main')
    await connect(page, code, 'error', page.getByTestId('exit-node-exit'), 'error')
  } else {
    await connect(page, agent, 'main', page.getByTestId('exit-node-exit'), 'main')
  }

  const draft = await saveAndReadDraft(page, token, workflowId)
  codeKey = draft.definition.nodes.find((node) => node.type === 'code')?.key ?? null
  await page.screenshot({ path: `provider-${options.sandboxName ?? 'agent'}-canvas.png`, fullPage: true })
  return { draft, codeKey }
}

test.describe.serial('Provider integration', () => {
  test('LightRAG knowledge, Mem0 memory and OpenSandbox code node run in one workflow', async ({ page }, testInfo) => {
    test.setTimeout(600_000)
    let token = await login(page)
    const department = await ensureModelResources(page, token)

    const stamp = Date.now()
    const knowledgeName = `Provider LightRAG ${stamp}`
    const memoryName = `Provider Mem0 ${stamp}`
    const sandboxName = `Provider Sandbox ${stamp}`
    await createKnowledgeViaUi(page, `Provider LightRAG Connection ${stamp}`, lightRagBaseUrl, '/health', knowledgeName, `pv_integration_${stamp}`, providerRagKeyName)
    await createMemoryViaUi(page, `Provider Mem0 Connection ${stamp}`, mem0BaseUrl, '/openapi.json', memoryName, `pv_integration_${stamp}`)
    await verifyConnectionTest(page, '知识库', knowledgeName)
    await verifyConnectionTest(page, '记忆', memoryName)
    await ensureSandboxProfile(page, token, department.id, sandboxName)

    const workflowName = `Provider Integration ${stamp}`
    const workflowId = await createWorkflow(page, workflowName)
    const grants: Array<[keyof typeof resourceTabs, string]> = [
      ['credential', providerCredentialName],
      ['credential', providerRagKeyName],
      ['model', providerModelAlias],
      ['rag', knowledgeName],
      ['memory', memoryName],
      ['sandbox_profile', sandboxName],
    ]
    for (const [type, name] of grants) await grantResource(page, type, name, workflowName)

    const { draft } = await buildAgentWorkflow(page, token, workflowId, { knowledgeName, memoryName, sandboxName })
    const agentNode = draft.definition.nodes.find((node) => node.type === 'agent')!
    expect(agentNode.resourceReferences.map((item) => item.bindingRole)).toEqual(expect.arrayContaining(['model', 'knowledge', 'long_term_memory']))
    const codeNode = draft.definition.nodes.find((node) => node.type === 'code')!
    expect(codeNode.resourceReferences.some((item) => item.resourceType === 'sandbox_profile' && item.resourceVersionId)).toBe(true)

    token = await ensureFreshEditorSession(page, token, workflowId)
    const executionId = await startDebug(page, () => page.locator('header').getByRole('button', { name: '运行', exact: true }).click())
    token = await freshApiToken(page, token)
    await waitExecution(page, token, executionId, ['succeeded'])

    const runs = await api<{ items: Array<{ nodeId: string, nodeKey: string, status: string, output?: { main?: Array<{ json: Record<string, unknown> }> } }> }>(page, token, `/executions/${executionId}/nodes`)
    const agentRun = runs.items.find((run) => run.nodeKey === agentNode.key || run.nodeId === agentNode.id)
    expect(agentRun?.status).toBe('succeeded')
    const codeRun = runs.items.find((run) => run.nodeKey === codeNode.key || run.nodeId === codeNode.id)
    expect(codeRun?.status).toBe('succeeded')
    expect(codeRun?.output?.main?.[0].json).toMatchObject({
      exitCode: 0,
      structuredOutput: { message: 'Run the provider integration workflow.', sandbox: 'opensandbox' },
    })

    const ragSpan = await expectRuntimeTrace(page, token, executionId, 'rag', 'succeeded')
    let sandboxSpan: TraceSpan | undefined
    await expect.poll(async () => {
      const trace = await fetchTrace(page, token, executionId)
      sandboxSpan = trace.spans.find((span) => span.spanKind === 'sandbox')
      return sandboxSpan?.status ?? null
    }, { timeout: 60_000 }).toBe('succeeded')
    expect(ragSpan.spanName).toBeTruthy()

    await page.goto(`/executions/${executionId}`)
    await expect(page.locator('header').getByText('成功', { exact: true })).toBeVisible({ timeout: 60_000 })
    await page.screenshot({ path: testInfo.outputPath('provider-execution.png'), fullPage: true })

    // Long-term memory is subject-and-application scoped by design: memory
    // tools refuse to run outside an application session with a trusted
    // end-user subject (AGENT_LONG_TERM_MEMORY_SUBJECT_REQUIRED), so a debug
    // run must surface that refusal instead of touching the provider.
    token = await freshApiToken(page, token)
    const latestDraft = await api<{ revision: number, definition: { nodes: Array<{ id: string, type: string }> } }>(page, token, `/workflows/${workflowId}/draft`)
    const sideEffectDecisions = Object.fromEntries(latestDraft.definition.nodes.filter((node) => node.type !== 'exit').map((node) => [node.id, 'execute']))
    const refusalResponse = await page.request.post(`/api/v1/workflows/${workflowId}/debug-executions`, {
      headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': `provider-refusal-${stamp}` },
      data: { expectedRevision: latestDraft.revision, mode: 'full', input: { question: 'P3_MEMORY_RECALL recall the attached long-term memory' }, context: {}, overlayIds: [], sideEffectDecisions },
    })
    if (refusalResponse.status() !== 202) throw new Error(`refusal debug execution: ${refusalResponse.status()} ${await refusalResponse.text()}`)
    const memoryDebugExecutionId = ((await refusalResponse.json()) as { executionId: string }).executionId
    await waitExecution(page, token, memoryDebugExecutionId, ['succeeded'])
    await expect.poll(async () => {
      const trace = await fetchTrace(page, token, memoryDebugExecutionId)
      return trace.spans.find((span) => span.spanKind === 'runtime_call' && span.spanName === 'Agent tool operation' && span.status === 'failed')?.spanName ?? null
    }, { timeout: 60_000 }).toBeTruthy()

  })

  test('RAGflow endpoint is rejected by the provider egress policy', async ({ page }) => {
    test.skip(!ragflowRejectedBaseUrl, 'RAGflow rejected endpoint is not configured')
    test.setTimeout(300_000)
    await login(page)
    const stamp = Date.now()
    const knowledgeName = `Provider RAGflow ${stamp}`
    await createKnowledgeViaUi(page, `Provider RAGflow Connection ${stamp}`, ragflowRejectedBaseUrl, '/', knowledgeName, `ragflow_${stamp}`)
    await openResourceDetail(page, '知识库', knowledgeName)
    await page.getByRole('button', { name: '测试连接' }).click()
    await expect(page.getByText('连接正常')).toHaveCount(0)
    await expect(page.getByRole('status').filter({ hasText: /.+/ }).first()).toBeVisible({ timeout: 30_000 })
    await page.screenshot({ path: `provider-ragflow-policy-rejected-${Date.now()}.png`, fullPage: true })
  })

  test('RAGflow knowledge speaks the RAGFlow retrieval protocol end to end', async ({ page }, testInfo) => {
    test.skip(!ragflowBaseUrl, 'RAGflow endpoint is not configured')
    test.setTimeout(600_000)
    let token = await login(page)
    await ensureModelResources(page, token)
    const stamp = Date.now()
    const knowledgeName = `Provider RAGflow ${stamp}`
    // Provider=ragflow routes the runtime through POST /api/v1/retrieval with
    // a Bearer credential; the dataset id is the external resource id.
    await createKnowledgeViaUi(page, `Provider RAGflow Connection ${stamp}`, ragflowBaseUrl, '/v1/system/healthz', knowledgeName, process.env.AGENTX_E2E_RAGFLOW_DATASET_ID ?? '00000000-0000-0000-0000-000000000000', ragflowCredentialName, 'ragflow')
    const workflowName = `Provider RAGflow Workflow ${stamp}`
    const workflowId = await createWorkflow(page, workflowName)
    for (const [type, name] of [['credential', providerCredentialName], ['credential', ragflowCredentialName], ['model', providerModelAlias], ['rag', knowledgeName]] as Array<[keyof typeof resourceTabs, string]>) {
      await grantResource(page, type, name, workflowName)
    }
    await buildAgentWorkflow(page, token, workflowId, { knowledgeName, memoryName: null, sandboxName: null })
    token = await freshApiToken(page, token)
    const executionId = await startDebug(page, () => page.locator('header').getByRole('button', { name: '运行', exact: true }).click())
    token = await freshApiToken(page, token)
    await waitExecution(page, token, executionId, ['succeeded'])
    const ragSpan = await expectRuntimeTrace(page, token, executionId, 'rag', 'succeeded')
    const detail = await page.request.get(`/api/v1/executions/${executionId}/trace/spans/${ragSpan.spanId}`, { headers: { Authorization: `Bearer ${token}` } })
    if (!detail.ok()) throw new Error(`ragflow span detail: ${detail.status()}`)
    const span = (await detail.json()) as { contents?: Array<{ kind: string, preview: unknown }> }
    const request = span.contents?.find((content) => content.kind === 'runtime_request')?.preview as Record<string, unknown> | undefined
    expect(request?.dataset_ids).toBeTruthy()
    expect(request?.question).toBeTruthy()
    await page.goto(`/executions/${executionId}`)
    await expect(page.locator('header').getByText('成功', { exact: true })).toBeVisible({ timeout: 60_000 })
    await page.screenshot({ path: testInfo.outputPath('provider-ragflow-adapter-execution.png'), fullPage: true })
  })

  test('RAGflow cannot answer the LightRAG query protocol even when reachable', async ({ page }) => {
    test.skip(!ragflowAliasBaseUrl, 'RAGflow alias endpoint is not configured')
    test.setTimeout(600_000)
    let token = await login(page)
    await ensureModelResources(page, token)
    const stamp = Date.now()
    const knowledgeName = `Provider RAGflow Alias ${stamp}`
    await createKnowledgeViaUi(page, `Provider RAGflow Alias Connection ${stamp}`, ragflowAliasBaseUrl, '/', knowledgeName, `ragflow-alias-${stamp}`)
    const workflowName = `Provider RAGflow Workflow ${stamp}`
    const workflowId = await createWorkflow(page, workflowName)
    for (const [type, name] of [['credential', providerCredentialName], ['model', providerModelAlias], ['rag', knowledgeName]] as Array<[keyof typeof resourceTabs, string]>) {
      await grantResource(page, type, name, workflowName)
    }
    await buildAgentWorkflow(page, token, workflowId, { knowledgeName, memoryName: null, sandboxName: null })
    token = await freshApiToken(page, token)
    const executionId = await startDebug(page, () => page.locator('header').getByRole('button', { name: '运行', exact: true }).click())
    token = await freshApiToken(page, token)
    await waitExecution(page, token, executionId, ['succeeded'])
    // RAGflow wraps unknown LightRAG routes in HTTP 200 + code 100, so the
    // span may look successful while the knowledge result is always empty.
    const ragSpan = await expectRuntimeTrace(page, token, executionId, 'rag', 'succeeded')
    const detail = await page.request.get(`/api/v1/executions/${executionId}/trace/spans/${ragSpan.spanId}`, { headers: { Authorization: `Bearer ${token}` } })
    if (!detail.ok()) throw new Error(`ragflow span detail: ${detail.status()}`)
    const span = (await detail.json()) as { contents?: Array<{ kind: string, preview: unknown }> }
    const response = span.contents?.find((content) => content.kind === 'runtime_response')?.preview as Record<string, unknown> | undefined
    const documents = response?.data ?? response?.documents ?? response?.chunks
    expect(JSON.stringify(response ?? {})).toContain('100')
    expect(documents ?? []).toHaveLength(0)
    await page.goto(`/executions/${executionId}`)
    await page.screenshot({ path: `provider-ragflow-protocol-mismatch-${Date.now()}.png`, fullPage: true })
  })
})
