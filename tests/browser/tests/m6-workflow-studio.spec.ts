import { expect, type Locator, type Page, test } from '@playwright/test'
import { readFile, writeFile } from 'node:fs/promises'
import { publishCompatibleChatMapping, useRuntimePortForward } from './playground-helpers'
import { fillMonaco, resizeLoop, showEdgeToolbar } from './workflow-studio-editor-helpers'
const password = 'agentx-e2e-admin-password'
const gatewayBase = process.env.AGENTX_E2E_RUNTIME_URL ?? ''
const echoBaseUrl = process.env.AGENTX_E2E_ECHO_BASE_URL ?? 'http://echo-mcp:8090'
const studioCredentialName = 'M6 Studio Fixture Credential'
const studioModelName = 'm6-studio-fixture-model'
const studioMcpName = 'M6 Studio Fixture MCP'
const studioSandboxName = 'M6 Studio Python Fixture'
const studioNetworkSandboxName = 'M6 Studio TCP Proxy Fixture'
const codeEgressHost = process.env.AGENTX_E2E_CODE_EGRESS_HOST ?? ''
const codeHttpPort = Number(process.env.AGENTX_E2E_CODE_HTTP_PORT ?? 0)
const codeTcpPort = Number(process.env.AGENTX_E2E_CODE_TCP_PORT ?? 0)
const resourceNames = [
  ['credential', studioCredentialName, undefined],
  ['model', studioModelName, undefined],
  ['mcp_server', studioMcpName, undefined],
  ['mcp_tool', 'Echo', studioMcpName],
  ['sandbox_profile', studioSandboxName, undefined],
] as const
const resourceTabs = { credential: '凭证', model: '模型', mcp_server: 'MCP 服务', mcp_tool: 'MCP 工具', sandbox_profile: '沙箱配置' } as const
type Execution = { id: string; status: string; errorCode?: string | null; output?: Record<string, unknown> | null }
type Approval = { id: string; executionId: string; status: string }
type Application = { id: string; name: string; slug: string }
type ApplicationDeployment = { id: string; status: string; publishErrorCode?: string | null; publishErrorMessage?: string | null }
type WorkflowVersion = { id: string; versionNumber: number }
type GatewayInvocation = { id: string; executionId?: string; status: string; error?: unknown | null }
type GatewayMessage = { role: string; parts: Array<{ content?: unknown }> }
type PageResponse<T> = { items: T[] }
type NamedResource = { id: string; name?: string; alias?: string; serverId?: string; version?: number }
type StudioDraft = {
  revision: number
  definition: {
    schemaVersion: string
    start: { inputs: unknown; contexts: Record<string, unknown> }
    settings: Record<string, unknown>
    nodes: Array<{ id: string; key: string; type: string; name: string; parentId?: string; parameters: Record<string, unknown>; resourceReferences: Array<{ bindingRole?: string }>; contextWrites: Array<{ operation: string; path: string; value: InputBinding }>; settings: Record<string, unknown> }>
    connections: Array<{ id: string; sourceNodeId: string; sourceHandle: string; targetNodeId: string; targetHandle: string; order: number }>
    end: { completion: 'first_return' | 'all_complete'; outputs: Record<string, unknown>; error: { outputs: Record<string, unknown> } }
  }
  editorDocument: Record<string, unknown>
}
async function login(page: Page) {
  const bootstrapStatus = await page.request.get('/api/v1/bootstrap/status')
  if (!bootstrapStatus.ok()) throw new Error(`bootstrap status: ${bootstrapStatus.status()} ${await bootstrapStatus.text()}`)
  if (((await bootstrapStatus.json()) as { required: boolean }).required) {
    await page.goto('/setup')
    await page.getByLabel('公司名称').fill('Agentx E2E')
    await page.getByLabel('用户名').fill('admin')
    await page.getByLabel('管理员姓名').fill('E2E Admin')
    await page.getByLabel('密码').fill(password)
    const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/bootstrap') && value.request().method() === 'POST')
    await page.getByRole('button', { name: '初始化并进入工作台' }).click()
    await expect(page).toHaveURL(/\/$/)
    const token = ((await (await response).json()) as { accessToken: string }).accessToken
    const me = await page.request.get('/api/v1/auth/me', { headers: { Authorization: `Bearer ${token}` } })
    expect(me.ok()).toBeTruthy()
    const user = (await me.json()) as { id: string; displayName: string }
    return { token, userName: user.displayName }
  }
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(password)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  const token = ((await (await response).json()) as { accessToken: string }).accessToken
  const me = await page.request.get('/api/v1/auth/me', { headers: { Authorization: `Bearer ${token}` } })
  expect(me.ok()).toBeTruthy()
  const user = (await me.json()) as { id: string; displayName: string }
  return { token, userName: user.displayName }
}

async function api<T>(page: Page, token: string, path: string): Promise<T> {
  const response = await page.request.get(`/api/v1${path}`, { headers: { Authorization: `Bearer ${token}` } })
  if (!response.ok()) {
    throw new Error(`${path}: ${response.status()} ${await response.text()}`)
  }
  return response.json() as Promise<T>
}

async function mutate<T>(page: Page, token: string, path: string, method: 'POST' | 'PUT' | 'PATCH', body: unknown): Promise<T> {
  const response = await page.request.fetch(`/api/v1${path}`, { method, data: body, headers: { Authorization: `Bearer ${token}` } })
  if (!response.ok()) throw new Error(`${path}: ${response.status()} ${await response.text()}`)
  return response.json() as Promise<T>
}

async function createWorkflow(page: Page, name: string) {
  await page.getByRole('link', { name: '工作流', exact: true }).click()
  await page.getByRole('button', { name: '新建工作流' }).click()
  const dialog = page.getByRole('dialog', { name: '新建工作流' })
  await dialog.getByLabel('工作流名称').fill(name)
  await dialog.getByLabel('描述').fill('M6 Workflow Studio Kubernetes E2E')
  await dialog.getByRole('button', { name: '保存' }).click()
  await expect(page).toHaveURL(/\/workflows\/[0-9a-f-]+$/)
  return page.url().split('/').at(-1) as string
}

async function grantResource(page: Page, resourceType: keyof typeof resourceTabs, resourceName: string, detail: string | undefined, workflowName: string) {
  await page.goto('/resource-grants')
  await page.getByRole('tab', { name: resourceTabs[resourceType], exact: true }).click()
  const search = page.getByRole('searchbox', { name: '搜索资源名称、类型或连接信息' })
  await search.fill(resourceName)
  const rows = page.getByRole('row').filter({ hasText: resourceName })
  const row = detail ? rows.filter({ hasText: detail }).first() : rows.first()
  await expect(row).toBeVisible()
  await row.getByRole('button', { name: '管理授权' }).click()
  const dialog = page.getByRole('dialog').filter({ hasText: resourceName })
  await expect(dialog).toBeVisible()
  const subject = dialog.getByRole('combobox').nth(1)
  await subject.click()
  await page.getByRole('option', { name: workflowName, exact: true }).click()
  const revokeButtons = dialog.getByRole('button', { name: '撤销授权' })
  const grantCount = await revokeButtons.count()
  await dialog.getByRole('button', { name: '添加授权' }).click()
  await expect(revokeButtons).toHaveCount(grantCount + 1)
  await dialog.getByRole('button', { name: '取消' }).click()
}

function flowNode(page: Page, role: string) {
  const labels: Record<string, RegExp> = {
    trigger: /手动触发|Manual Trigger/,
    agent: /智能体|Agent/,
    code: /代码|Code/,
    approval: /审批|Approval/,
    merge: /合并|Merge/,
  }
  return page.locator('.react-flow__node').filter({ hasText: labels[role] ?? new RegExp(role, 'i') }).first()
}

function studioRun(page: Page) {
  return page.locator('header').getByRole('button', { name: '运行', exact: true })
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
type InputBinding =
  | { kind: 'literal'; value: unknown }
  | { kind: 'reference'; selector: { namespace: string; sourceNodeId?: string; port?: string; path: Array<string | number> }; missingPolicy: { kind: string } }
  | { kind: 'template'; segments: Array<{ kind: 'text'; text: string } | Extract<InputBinding, { kind: 'reference' }> > }
  | { kind: 'array'; items: InputBinding[] }
  | { kind: 'object'; fields: Record<string, InputBinding> }

async function waitApplicationDeployment(page: Page, token: string, applicationId: string, deploymentId: string) {
  await expect.poll(async () => {
    const deployments = await api<ApplicationDeployment[]>(page, token, `/applications/${applicationId}/deployments`)
    const deployment = deployments.find((item) => item.id === deploymentId)
    if (deployment?.status === 'rejected') {
      throw new Error(`application deployment ${deploymentId} was rejected: ${deployment.publishErrorCode ?? ''} ${deployment.publishErrorMessage ?? ''}`)
    }
    return deployment?.status
  }, { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe('active')
}

async function ensureStudioResources(page: Page, token: string) {
  const departments = await api<Array<{ id: string; isRoot: boolean }>>(page, token, '/departments')
  const department = departments.find((item) => item.isRoot) ?? departments[0]
  if (!department) throw new Error('M6 Studio requires a Department created through the bootstrap API')

  const credentials = await api<PageResponse<NamedResource>>(page, token, `/credentials?pageSize=100&search=${encodeURIComponent(studioCredentialName)}`)
  const credential = credentials.items.find((item) => item.name === studioCredentialName)
    ?? await mutate<NamedResource>(page, token, '/credentials', 'POST', {
      name: studioCredentialName,
      credentialType: 'bearer',
      secret: 'm5-model-secret',
      ownerDepartmentId: department.id,
    })

  const models = await api<PageResponse<NamedResource>>(page, token, `/models/aliases?pageSize=100&search=${encodeURIComponent(studioModelName)}`)
  if (!models.items.some((item) => item.alias === studioModelName)) {
    await mutate(page, token, '/models/aliases', 'POST', {
      connectionName: 'M6 Studio Fixture Model',
      providerType: 'openai_compatible',
      endpoint: `${echoBaseUrl}/v1`,
      credentialId: credential.id,
      ownerDepartmentId: department.id,
      alias: studioModelName,
      modelName: 'echo-model-v1',
      maxInputTokens: 8192,
      maxOutputTokens: 2048,
      defaultParameters: { temperature: 0 },
      price: { currency: 'USD', inputPerMillion: '5', outputPerMillion: '30' },
    })
  }

  const servers = await api<PageResponse<NamedResource>>(page, token, `/mcp/servers?pageSize=100&search=${encodeURIComponent(studioMcpName)}`)
  const server = servers.items.find((item) => item.name === studioMcpName)
    ?? await mutate<NamedResource>(page, token, '/mcp/servers', 'POST', {
      name: studioMcpName,
      description: 'M6 API-first MCP fixture',
      ownerDepartmentId: department.id,
      transport: {
        kind: 'streamable_http',
        endpoint: `${echoBaseUrl}/mcp`,
        bearerCredentialId: credential.id,
      },
      configuration: {},
    })
  const tools = await api<PageResponse<NamedResource>>(page, token, `/mcp/tools?pageSize=100&search=Echo`)
  if (!tools.items.some((item) => item.name === 'echo' && item.serverId === server.id)) {
    await mutate(page, token, `/mcp/servers/${server.id}/discover`, 'POST', {})
  }
  await mutate(page, token, `/mcp/servers/${server.id}`, 'PATCH', {
    name: studioMcpName,
    description: 'M6 API-first MCP fixture',
    transport: {
      kind: 'streamable_http',
      endpoint: `${echoBaseUrl}/mcp`,
      bearerCredentialId: credential.id,
    },
    configuration: {},
    status: 'active',
    version: server.version,
  })

  const sandboxes = await api<PageResponse<NamedResource>>(page, token, '/sandbox-profiles?pageSize=100')
  for (const [name, egressMode] of [[studioSandboxName, 'none'], [studioNetworkSandboxName, 'tcp_proxy']] as const) {
    if (sandboxes.items.some((item) => item.name === name)) continue
    await mutate(page, token, '/sandbox-profiles', 'POST', {
      name,
      description: `M6 API-first OpenSandbox ${egressMode} fixture`,
      ownerDepartmentId: department.id,
      runner: 'python',
      imageDigest: 'opensandbox/code-interpreter@sha256:64cd01f03f54ba347d1a1310dcbc18ac5cb17d01714e23b4ea4b840fbb0d6623',
      cpuMillis: 500,
      memoryBytes: 536870912,
      pidsLimit: 256,
      diskBytes: 1073741824,
      timeoutSeconds: 120,
      outputLimitBytes: 1048576,
      networkPolicy: { defaultAction: 'deny', egressMode },
    })
  }
}

async function choose(page: Page, scope: Locator, option: string | RegExp) {
  const trigger = await scope.getAttribute('role') === 'combobox' ? scope : scope.getByRole('combobox')
  await trigger.click()
  await page.getByRole('option', { name: option }).click()
}

async function chooseAgentSessionPolicy(page: Page, details: Locator, mode: 'application_session' | 'invocation' = 'invocation') {
  const sessionPolicy = details.locator('[data-field-path="parameters.sessionPolicy"]')
  await choose(page, sessionPolicy, mode === 'application_session' ? /应用会话|Application session/ : /仅本次调用|Invocation only/)
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

async function chooseReference(page: Page, scope: Locator, namespace: RegExp, labels: string[], focusEditor = true) {
  const editor = scope.getByRole('textbox', { name: 'Value' }).first()
  if (focusEditor) await editor.click()
  await selectOpenReference(page, namespace, labels)
}

async function chooseDirectReference(page: Page, scope: Locator, namespace: RegExp, labels: string[]) {
  const referenceButton = scope.getByTestId('reference-input').getByRole('button').first()
  if (await referenceButton.count()) await referenceButton.click()
  else await scope.getByRole('textbox', { name: 'Value' }).first().click()
  await selectOpenReference(page, namespace, labels)
}

async function selectOpenReference(page: Page, namespace: RegExp, labels: string[]) {
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

async function dismissReferencePicker(page: Page) {
  const picker = page.getByTestId('reference-picker')
  if (!await picker.isVisible().catch(() => false)) return
  await page.keyboard.press('Escape')
  await expect(picker).toBeHidden()
}

async function setExitContractField(page: Page, panel: Locator, name: string, required: boolean, type = 'string') {
  await panel.getByRole('button', { name: /添加字段|Add field/ }).first().click()
  const dialog = page.getByRole('dialog', { name: /输出字段|Output field/ })
  const outputName = dialog.getByLabel(/输出名称|Output name/)
  await outputName.fill(name)
  await outputName.blur()
  if (type !== 'string') {
    await dialog.getByLabel(/类型|Type/).click()
    await page.getByRole('option', { name: /对象|Object/ }).click()
  }
  if (required) await dialog.getByLabel(/必填|Required/).check()
  await dialog.getByRole('button', { name: /保存|Save/ }).click()
  await expect(page.getByTestId(`exit-mapping-${name}`)).toBeVisible()
}

async function openExitPanel(page: Page, key = 'exit') {
  await page.getByTestId(`exit-node-${key}`).click()
  const panel = page.getByTestId('exit-panel')
  await expect(panel).toBeVisible()
  return panel
}

async function setEndOutput(page: Page, nodeKey: string, fieldPath = 'json', port = 'main', required = true, type = 'string') {
  const panel = await openExitPanel(page)
  await setExitContractField(page, panel, 'answer', required, type)
  const mapping = panel.getByTestId('exit-mapping-answer')
  const fields = fieldPath.split('.').filter((field) => field !== 'json')
  await chooseReference(page, mapping, /输出|Outputs/, [nodeKey, port, 'current', ...fields])
  await expect(mapping.locator('[data-agentx-variable]')).toBeVisible()
  await panel.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
}

async function setEndContextOutput(page: Page) {
  const panel = await openExitPanel(page)
  await setExitContractField(page, panel, 'context_workflow_name', false)
  const mapping = panel.getByTestId('exit-mapping-context_workflow_name')
  await chooseReference(page, mapping, /全局变量|Global variables|Contexts/, ['session_note'])
  await panel.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
}

async function setStartInputs(page: Page) {
  await page.getByTestId('workflow-start').click()
  const panel = page.getByTestId('start-panel')
  await expect(panel).toBeVisible()
  await panel.getByRole('button', { name: /添加字段|Add field/ }).first().click()
  let dialog = page.getByRole('dialog', { name: /添加字段|Add field/ })
  const fieldName = dialog.getByLabel(/字段名|Field name/)
  await fieldName.fill('question')
  await fieldName.blur()
  await dialog.getByLabel(/标题|Title/).fill('Question')
  await dialog.getByLabel(/必填|Required/).check()
  await dialog.getByRole('button', { name: /保存|Save/ }).click()
  await panel.getByRole('button', { name: /添加字段|Add field/ }).first().click()
  dialog = page.getByRole('dialog', { name: /添加字段|Add field/ })
  const attachmentsName = dialog.getByLabel(/字段名|Field name/)
  await attachmentsName.fill('attachments')
  await attachmentsName.blur()
  await dialog.getByLabel(/类型|Type/).click()
  await page.getByRole('option', { name: /文件数组|File array/ }).click()
  await dialog.getByLabel(/标题|Title/).fill('Attachments')
  await dialog.getByLabel(/允许的文件类型|Allowed content types/).fill('application/pdf, image/*')
  await dialog.getByLabel(/最大文件大小|Maximum file size/).fill('1048576')
  await dialog.getByLabel(/最少文件数|Minimum files/).fill('0')
  await dialog.getByLabel(/最多文件数|Maximum files/).fill('3')
  await dialog.getByLabel(/文件总大小上限|Maximum total file size/).fill('2097152')
  await dialog.getByRole('button', { name: /保存|Save/ }).click()
  const contextSection = panel.locator('section').filter({ hasText: /全局变量|Global variables/ }).last()
  await contextSection.getByRole('button', { name: /添加全局变量|Add global variable/ }).click()
  const contextDialog = page.getByRole('dialog', { name: /添加全局变量|Add global variable/ })
  const contextName = contextDialog.getByLabel(/全局变量名称|Global variable name/)
  await contextName.fill('session_note')
  await contextName.blur()
  await contextDialog.getByLabel(/默认值|Default value/).fill('initial')
  await contextDialog.getByRole('button', { name: /保存|Save/ }).click()
  await panel.getByRole('button', { name: /^(关闭|Close)$/ }).click()
}

async function setStartArrayInput(page: Page, name = 'items') {
  await page.getByTestId('workflow-start').click()
  const panel = page.getByTestId('start-panel')
  await panel.getByRole('button', { name: /添加字段|Add field/ }).first().click()
  const dialog = page.getByRole('dialog', { name: /添加字段|Add field/ })
  const fieldName = dialog.getByLabel(/字段名|Field name/)
  await fieldName.fill(name)
  await fieldName.blur()
  await dialog.getByLabel(/类型|Type/).click()
  await page.getByRole('option', { name: /^(数组|Array)$/ }).click()
  await dialog.getByLabel(/标题|Title/).fill('Items')
  await dialog.getByLabel(/必填|Required/).check()
  await dialog.getByRole('button', { name: /保存|Save/ }).click()
  await panel.getByRole('button', { name: /^(关闭|Close)$/ }).click()
}

async function configureReferenceIfCase(page: Page, details: Locator, workflowName: string) {
  const panel = details.getByTestId('if-panel')
  const branch = panel.getByTestId('condition-branch-0')
  await expect(branch.getByTestId('condition-comparison-row')).toHaveCount(1)
  const comparison = branch.getByTestId('condition-comparison-row').first()
  await chooseDirectReference(page, comparison, /运行信息|Execution information/, ['运行信息', '工作流', '工作流名称'])
  await comparison.getByRole('textbox', { name: 'Value' }).last().fill(workflowName)
  await expect(comparison.locator('[data-agentx-variable]')).toContainText(/工作流名称|Workflow name/)
  await expect(comparison.getByRole('textbox', { name: 'Value' }).last()).toHaveText(workflowName)
}

async function configureSetResult(details: Locator) {
  const rows = details.getByTestId('set-value-rows')
  await rows.getByRole('button', { name: /新建|Create/ }).click()
  const row = rows.locator(':scope > div').first()
  await row.getByRole('textbox', { name: /名称|Name/ }).fill('branch_result')
  await row.getByRole('textbox', { name: 'Value' }).pressSequentially('matched')
  await expect(row.getByRole('textbox', { name: 'Value' })).toHaveText('matched')
  await expect(row.getByRole('textbox', { name: /名称|Name/ })).toHaveValue('branch_result')
}

async function configureContextWrite(page: Page, details: Locator) {
  await expect(details.getByRole('button', { name: /自定义输出|custom output/i })).toHaveCount(0)
  await details.getByText(/高级配置|Advanced configuration/).click()
  await details.getByRole('button', { name: /添加全局变量写入|Add global variable write/ }).click()
  const writeDialog = page.getByRole('dialog', { name: /添加全局变量写入|Add global variable write/ })
  await writeDialog.getByLabel(/全局变量|Global variable/).click()
  await page.getByRole('option', { name: 'session_note' }).click()
  await chooseReference(page, writeDialog, /运行信息|Execution information/, ['运行信息', '工作流', '工作流名称'])
  await writeDialog.getByRole('button', { name: /保存|Save/ }).click()
}

async function setEndErrorOutput(page: Page) {
  const panel = await openExitPanel(page)
  const errorFields = panel.locator('section').filter({ hasText: /错误字段|Error fields/ }).last()
  await errorFields.getByRole('button', { name: /添加字段|Add field/ }).click()
  const dialog = page.getByRole('dialog', { name: /输出字段|Output field/ })
  const outputName = dialog.getByLabel(/输出名称|Output name/)
  await outputName.fill('failure_message')
  await outputName.blur()
  await dialog.getByRole('button', { name: /保存|Save/ }).click()
  await expect(panel.getByTestId('exit-mapping-failure_message')).toBeVisible()
  await chooseReference(page, panel.getByTestId('exit-mapping-failure_message'), /当前数据|Current data/, ['当前错误', '错误消息'])
  await panel.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
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

async function startDebug(page: Page, action: () => Promise<void>, confirm = true) {
  if (confirm) page.once('dialog', (dialog) => dialog.accept())
  const response = page.waitForResponse((value) => value.url().includes('/debug-executions') && value.request().method() === 'POST', { timeout: 60_000 })
  const parameters = page.getByRole('dialog', { name: /运行工作流|调试输入|Run workflow|Debug input/ })
  const submitParameters = async (timeout: number) => {
    const visible = await parameters.waitFor({ state: 'visible', timeout }).then(() => true).catch(() => false)
    if (!visible) return false
    const question = parameters.getByLabel(/Question|问题/)
    if (await question.isVisible().catch(() => false)) {
      await question.fill('Run the workflow from the Studio.')
    } else {
      const input = parameters.getByRole('textbox', { name: /输入|Input/ })
      if (await input.isVisible().catch(() => false)) await input.fill('{"question":"Run the workflow from the Studio."}')
    }
    await parameters.getByRole('button', { name: /^(开始运行|运行|Run workflow|Run)$/ }).click()
    return true
  }
  await action()
  const submitted = await submitParameters(10_000)
  let accepted = submitted ? undefined : await Promise.race([
    response,
    page.waitForTimeout(2_000).then(() => undefined),
  ])
  if (!submitted && !accepted && await studioRun(page).isVisible().catch(() => false)) {
    await action()
    await submitParameters(10_000)
  }
  accepted ??= await response
  if (accepted.status() !== 202) {
    throw new Error(`debug execution: ${accepted.status()} ${await accepted.text()}`)
  }
  return (await accepted.json() as { executionId: string }).executionId
}

async function waitExecution(page: Page, token: string, executionId: string, statuses: string[], timeout = 180_000) {
  let current: Execution | undefined
  await expect.poll(async () => {
    try {
      current = await api<Execution>(page, token, `/executions/${executionId}`)
    } catch (error) {
      if (error instanceof Error && (error.message.includes(': 404 ') || error.message.includes(': 503 '))) return false
      throw error
    }
    if (['succeeded', 'failed', 'cancelled', 'timed_out'].includes(current.status) && !statuses.includes(current.status)) {
      throw new Error(`Execution reached unexpected terminal state: ${JSON.stringify(current)}`)
    }
    return statuses.includes(current.status)
  }, { timeout, intervals: [500, 750, 1000, 2000] }).toBeTruthy()
  return current!
}

async function approveExecution(page: Page, token: string, executionId: string) {
  let approval: Approval | undefined
  await expect.poll(async () => {
    const result = await api<{ items: Approval[] }>(page, token, '/approvals?pageSize=100')
    approval = result.items.find((item) => item.executionId === executionId && item.status === 'pending')
    if (!approval) {
      const execution = await api<Execution>(page, token, `/executions/${executionId}`)
      if (['failed', 'cancelled', 'timed_out'].includes(execution.status)) throw new Error(`Execution stopped before Approval: ${JSON.stringify(execution)}`)
    }
    return approval?.id
  }, { timeout: 120_000 }).toBeTruthy()
  await page.goto(`/approvals/${approval!.id}`)
  await page.getByRole('button', { name: '领取' }).click()
  await expect(page.getByRole('button', { name: '通过' })).toBeVisible()
  await page.getByRole('button', { name: '通过' }).click()
  const dialog = page.getByRole('dialog', { name: '通过' })
  await dialog.getByRole('button', { name: '通过' }).click()
  await expect.poll(async () => {
    const result = await api<{ items: Approval[] }>(page, token, '/approvals?pageSize=100')
    return result.items.find((item) => item.id === approval!.id)?.status
  }).toBe('decided')
}

async function selectDebugMode(page: Page, mode: 'full' | 'single_node' | 'to_node' | 'from_node') {
  const labels = { full: '完整流程', single_node: '单节点', to_node: '运行到节点', from_node: '从节点继续' }
  await page.getByRole('combobox', { name: '调试输入' }).click()
  await page.getByRole('option', { name: labels[mode], exact: true }).click()
}

async function setLocale(page: Page, locale: 'zh-CN' | 'en-US') {
  if (await page.locator('html').getAttribute('lang') === locale) return
  await page.locator('header').getByRole('button', { name: /更多.*操作|More.*actions/i }).click()
  await page.getByRole('menuitemradio', { name: locale === 'zh-CN' ? '简体中文' : 'English' }).click()
  await expect(page.locator('html')).toHaveAttribute('lang', locale)
}

async function setTheme(page: Page, theme: 'light' | 'dark') {
  await page.locator('header').getByRole('button', { name: /更多.*操作|More.*actions/i }).click()
  await page.getByRole('menuitemradio', { name: theme === 'light' ? /^(浅色|Light)$/ : /^(深色|Dark)$/ }).click()
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
}

test('M6 Studio builds and reopens Start to IF to Set to Merge to Exit entirely through visible UI', async ({ page }, testInfo) => {
  const { token } = await login(page)
  const workflowName = `M6 UI Branch Merge ${Date.now()}`
  const workflowId = await createWorkflow(page, workflowName)

  await page.goto(`/workflows/${workflowId}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  await addFromCreator(page, 'palette-action-if')
  const ifNode = page.locator('.react-flow__node-manifest').filter({ hasText: /条件分支|IF/ }).first()
  const ifDetails = await openNodeDetails(page, ifNode)
  await configureReferenceIfCase(page, ifDetails, workflowName)
  await ifDetails.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  await connect(page, page.getByTestId('workflow-start'), 'main', ifNode, 'main')

  await ifNode.getByRole('button', { name: /条件 1.*添加节点|Add node after Case 1/i }).click()
  await page.getByRole('textbox', { name: /搜索节点|Search nodes/ }).fill('set')
  await page.getByTestId('palette-action-set').click()
  const setNode = page.locator('.react-flow__node-manifest').filter({ hasText: /编辑字段|Set/ }).first()
  const setDetails = await openNodeDetails(page, setNode)
  await configureSetResult(setDetails)
  await setDetails.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()

  await addFromCreator(page, 'palette-action-merge')
  const mergeNode = flowNode(page, 'merge')
  const mergeDetails = await openNodeDetails(page, mergeNode)
  await mergeDetails.getByTestId('mode-card-append').click()
  await mergeDetails.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()

  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  await connect(page, setNode, 'main', mergeNode, 'main')
  await connect(page, mergeNode, 'main', page.getByTestId('exit-node-exit'), 'main')

  const draft = await saveAndReadDraft(page, token, workflowId)
  const byType = Object.fromEntries(draft.definition.nodes.map((node) => [node.type, node]))
  expect(draft.definition.nodes.map((node) => node.type).sort()).toEqual(['exit', 'if', 'merge', 'set'])
  expect(draft.definition.connections).toEqual(expect.arrayContaining([
    expect.objectContaining({ sourceNodeId: '__start__', targetNodeId: byType.if.id }),
    expect.objectContaining({ sourceNodeId: byType.if.id, sourceHandle: 'case:case_1', targetNodeId: byType.set.id }),
    expect.objectContaining({ sourceNodeId: byType.set.id, targetNodeId: byType.merge.id }),
    expect.objectContaining({ sourceNodeId: byType.merge.id, targetNodeId: byType.exit.id }),
  ]))
  expect(draft.definition.connections).toHaveLength(4)
  expect(byType.set.parameters.values).toMatchObject({ kind: 'object', fields: { branch_result: { kind: 'literal', value: 'matched' } } })

  await page.reload()
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  const reopenedIf = page.getByTestId(`studio-node-${byType.if.id}`)
  const reopenedSet = page.getByTestId(`studio-node-${byType.set.id}`)
  const reopenedMerge = page.getByTestId(`studio-node-${byType.merge.id}`)
  await expect(reopenedIf.locator('.studio-branch-row').first()).toContainText('IF')
  await expect(reopenedSet.locator('.studio-card-body')).toContainText(/设置 1 个字段|Set 1 field/)
  await expect(reopenedMerge.locator('.studio-card-body')).toContainText(/追加|Append/)
  const reopenedIfDetails = await openNodeDetails(page, reopenedIf)
  const reopenedComparison = reopenedIfDetails.getByTestId('condition-comparison-row')
  await expect(reopenedComparison.locator('[data-agentx-variable]')).toContainText(/工作流名称|Workflow name/)
  await expect(reopenedComparison.getByRole('textbox', { name: 'Value' }).last()).toHaveText(workflowName)
  await reopenedIfDetails.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()

  const executionId = await startDebug(page, () => studioRun(page).click())
  await waitExecution(page, token, executionId, ['succeeded'])
  const runs = await api<{ items: Array<{ nodeId: string; status: string; output?: Record<string, Array<{ json: Record<string, unknown> }>> }> }>(page, token, `/executions/${executionId}/nodes`)
  expect(runs.items.find((run) => run.nodeId === byType.if.id)?.status).toBe('succeeded')
  expect(runs.items.find((run) => run.nodeId === byType.set.id)?.output?.main?.[0].json).toMatchObject({ branch_result: 'matched' })
  expect(runs.items.find((run) => run.nodeId === byType.merge.id)?.status).toBe('succeeded')
  await page.screenshot({ path: testInfo.outputPath('ui-start-if-set-merge-exit.png'), fullPage: true })
})

test('M6 Studio creates, debugs, versions and publishes a manifest-driven Workflow', async ({ context, page }, testInfo) => {
  const { token, userName } = await login(page)
  await ensureStudioResources(page, token)
  const workflowName = `M6 Studio ${Date.now()}`
  const workflowId = await createWorkflow(page, workflowName)

  for (const [resourceType, resourceName, detail] of resourceNames) await grantResource(page, resourceType, resourceName, detail, workflowName)

  await page.goto(`/workflows/${workflowId}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  await setStartInputs(page)
  await expect(page.getByTestId('node-creator')).toBeVisible()
  await expect(page.getByTestId('node-creator-trigger')).toHaveCount(0)
  await expect(page.getByTestId('node-creator-rail')).toHaveCount(0)
  await addFromCreator(page, 'palette-action-agent')
  await expect(page.locator('.react-flow__edge')).toHaveCount(0)
  for (const type of ['code', 'approval']) await addFromCreator(page, `palette-action-${type}`)
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })

  const agent = flowNode(page, 'agent')
  const code = flowNode(page, 'code')
  const approval = flowNode(page, 'approval')
  await expect(approval).toBeVisible()

  const agentConfigDetails = await openNodeDetails(page, agent)
  await choose(page, agentConfigDetails.getByTestId('agent-inspector-model'), new RegExp(studioModelName))
  await choose(page, agentConfigDetails.getByTestId('agent-inspector-workspace_sandbox'), new RegExp(studioSandboxName))
  await choose(page, agentConfigDetails.getByTestId('agent-inspector-mcp_tools'), new RegExp(studioMcpName))
  for (const slot of ['skills', 'knowledge', 'long_term_memory']) await expect(agentConfigDetails.getByTestId(`agent-inspector-${slot}`)).toBeVisible()
  await chooseAgentSessionPolicy(page, agentConfigDetails)
  await expect(agentConfigDetails.getByText('节点参数', { exact: true })).toHaveCount(0)
  const prompt = agentConfigDetails.getByTestId('parameter-systemPrompt')
  await expect(prompt.locator('[contenteditable="true"]')).toHaveCount(1)
  await fillNativeText(page, prompt, 'Use the attached resources and return a concise result.')
  const question = agentConfigDetails.getByTestId('parameter-userQuestion')
  await expect(question.locator('[contenteditable="true"]')).toHaveCount(1)
  await chooseReference(page, question, /输入|Inputs/, ['question'])
  await expect(agentConfigDetails.getByText('高级配置', { exact: true })).toBeVisible()
  await expect(agentConfigDetails.getByText('不支持的 UI 控件')).toHaveCount(0)
  await agentConfigDetails.getByTestId('agent-budget-section').getByRole('button', { name: /展开或收起高级配置|Expand or collapse advanced settings/ }).click()
  await expect(agentConfigDetails.getByTestId('parameter-maxDurationSeconds')).toContainText('秒')
  await expect(agentConfigDetails.getByTestId('parameter-maxTotalTokens')).toContainText('Token')

  const configuredCodeDetails = await openNodeDetails(page, code)
  await openNodeBasics(configuredCodeDetails)
  const codeName = configuredCodeDetails.locator('[data-field-path="name"] input')
  await codeName.fill('M6 Python Code')
  await codeName.blur()
  await expect(code).toContainText('M6 Python Code')
  await choose(page, page.getByTestId('parameter-runner'), /^Python$/)
  const codeInputs = configuredCodeDetails.getByTestId('parameter-inputs')
  const addCodeInput = async (name: string, namespace: RegExp, labels: string[]) => {
    await codeInputs.getByRole('button', { name: /添加字段|Add field/ }).click()
    const row = codeInputs.getByTestId('mapper-control').locator(':scope > div').last()
    const key = row.getByRole('textbox', { name: /键|Key/ })
    await key.fill(name)
    await key.blur()
    await chooseReference(page, row, namespace, labels)
  }
  await addCodeInput('message', /输入|Inputs/, ['question'])
  await addCodeInput('workflow_name', /运行信息|Execution information/, ['运行信息', '工作流', '工作流名称'])
  await fillMonaco(page, page.getByTestId('parameter-source'), 'def main(**inputs):\n    return {\n        "message": inputs["message"],\n        "workflow_name": inputs["workflow_name"],\n    }')
  await fillMonaco(page, page.getByTestId('code-output-example'), '{ message: "", workflow_name: "" }')
  await choose(page, page.getByTestId('resource-selector-sandbox_profile'), new RegExp(studioSandboxName))
  await configureContextWrite(page, configuredCodeDetails)

  await openNodeDetails(page, approval)
  await fillNativeText(page, page.getByTestId('parameter-title'), 'M6 Studio Approval')
  await fillNativeText(page, page.getByTestId('parameter-description'), 'Approve the Workflow created through the Studio UI.')
  await page.keyboard.press('Escape')
  await expect(page.getByTestId('reference-picker')).toBeHidden()
  await page.getByTestId('parameter-candidateUserId').getByRole('combobox').click()
  await page.getByRole('option', { name: userName }).click()

  await page.getByTestId('node-details-view').getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  await connect(page, page.getByTestId('workflow-start'), 'main', agent, 'main')
  await agent.locator('.react-flow__handle.source[data-handleid="main"]').hover()
  await expect(page.getByTestId('workflow-canvas')).toHaveAttribute('data-connection-state', 'source-hover')
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).hover()
  await expect(page.getByTestId('workflow-canvas')).toHaveAttribute('data-connection-state', 'idle')
  await connect(page, agent, 'main', code, 'main')
  await connect(page, code, 'main', approval, 'main')
  // Wiring is the policy: the error port edge alone routes code failures.
  await connect(page, code, 'error', page.getByTestId('exit-node-exit'), 'error')
  await connect(page, approval, 'decision:approved', page.getByTestId('exit-node-exit'), 'main')

  const approvalKey = (await saveAndReadDraft(page, token, workflowId)).definition.nodes.find((node) => node.type === 'approval')!.key
  await setEndOutput(page, approvalKey, 'json.decision', '通过', false)
  await setEndContextOutput(page)
  await setEndErrorOutput(page)

  await openNodeDetails(page, code)
  await page.getByRole('button', { name: '关闭' }).click()

  await page.getByTestId('node-creator').getByRole('button', { name: '便签' }).click()
  const note = page.locator('[data-testid^="studio-note-"]').first()
  await expect(note).toBeVisible()
  await note.dispatchEvent('dblclick')
  await note.getByRole('textbox').fill('M6 runtime notes')
  await note.getByRole('textbox').press('Control+Enter')
  await note.getByRole('button', { name: /便签颜色：blue/ }).click()

  await code.click()
  await page.keyboard.down('Shift')
  await approval.click({ force: true })
  await page.keyboard.up('Shift')
  await page.getByTestId('node-creator').getByRole('button', { name: '分组' }).click()
  const group = page.locator('[data-testid^="studio-group-"]').first()
  await expect(group).toBeVisible()
  await group.getByRole('button', { name: '折叠分组' }).click()
  await expect(group.getByRole('button', { name: '展开分组' })).toBeVisible()
  await group.getByRole('button', { name: '展开分组' }).click()

  const draft = await saveAndReadDraft(page, token, workflowId)
  expect(draft.definition.schemaVersion).toBe('8.0')
  expect(draft.definition.nodes.map((node) => node.type)).toEqual(expect.arrayContaining(['agent', 'code', 'approval']))
  expect(draft.definition.nodes.map((node) => node.type)).not.toContain('manual_trigger')
  expect(draft.definition.nodes.find((node) => node.type === 'code')?.name).toBe('M6 Python Code')
  expect(draft.definition.nodes.find((node) => node.type === 'agent')?.parameters.userQuestion).toMatchObject({ kind: 'reference', selector: expect.objectContaining({ namespace: 'inputs', path: ['question'] }) })
  expect(draft.definition.nodes.find((node) => node.type === 'agent')?.resourceReferences.map((item) => item.bindingRole)).toEqual(expect.arrayContaining(['model', 'workspace_sandbox', 'mcp_tools']))
  expect(draft.definition.nodes.find((node) => node.type === 'code')?.parameters.outputExample).toEqual({ message: '', workflow_name: '' })
  expect(draft.definition.nodes.find((node) => node.type === 'code')?.contextWrites).toContainEqual({ operation: 'set', path: 'session_note', value: expect.objectContaining({ kind: 'reference', selector: expect.objectContaining({ namespace: 'execution', path: ['workflow', 'name'] }) }) })
  expect(draft.definition.start.contexts).toMatchObject({ session_note: { default: 'initial', mutable: true } })
  const startInputs = draft.definition.start.inputs as { properties: Record<string, Record<string, unknown>> }
  expect(startInputs.properties.attachments).toMatchObject({
    type: 'array',
    minItems: 0,
    maxItems: 3,
    'x-agentx-artifact': true,
    'x-agentx-artifact-array': true,
    'x-agentx-content-types': ['application/pdf', 'image/*'],
    'x-agentx-max-size-bytes': 1048576,
    'x-agentx-max-total-size-bytes': 2097152,
  })
  const exitParameters = draft.definition.nodes.find((node) => node.type === 'exit')?.parameters as { outputs: Record<string, unknown>; errorOutputs: Record<string, unknown> }
  expect(exitParameters.outputs.answer).toMatchObject({ kind: 'reference', selector: { namespace: 'outputs', port: 'decision:approved', path: ['decision'] } })
  expect(exitParameters.outputs.context_workflow_name).toMatchObject({ kind: 'reference', selector: { namespace: 'contexts', path: ['session_note'] } })
  expect(exitParameters.errorOutputs.failure_message).toMatchObject({ kind: 'reference', selector: { namespace: 'item', path: ['message'] } })
  expect(draft.definition.connections).toContainEqual(expect.objectContaining({ sourceNodeId: draft.definition.nodes.find((node) => node.type === 'code')!.id, sourceHandle: 'error', targetHandle: 'error' }))
  // Slot model: the Agent carries the MCP tool as a slot reference directly;
  // canvas attachment nodes and binding edges no longer exist.
  expect(draft.editorDocument.bindingEdges ?? []).toHaveLength(0)

  const retryDefinition = structuredClone(draft.definition)
  const retryCode = retryDefinition.nodes.find((node) => node.type === 'code')!
  retryCode.settings = { ...retryCode.settings, retryOnFail: true, maxTries: 2, waitBetweenTriesMs: 500 }
  const retrySave = await page.request.put(`/api/v1/workflows/${workflowId}/draft`, {
    headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': `m6-code-retry-${Date.now()}` },
    data: { expectedRevision: draft.revision, definition: retryDefinition, editorDocument: draft.editorDocument },
  })
  expect(retrySave.ok(), await retrySave.text()).toBeTruthy()
  await page.reload()
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()

  const firstExecution = await startDebug(page, () => studioRun(page).click())
  const approvalPage = await context.newPage()
  await approveExecution(approvalPage, token, firstExecution)
  const completedExecution = await waitExecution(page, token, firstExecution, ['succeeded'])
  expect(completedExecution.output).toMatchObject({ context_workflow_name: workflowName })

  const me = await api<{ departmentName: string; roles: string[] }>(page, token, '/auth/me')
  const codeNodeId = draft.definition.nodes.find((node) => node.type === 'code')!.id
  const runtimeNodes = await api<{ items: Array<{ nodeId: string; output?: { main?: Array<{ json?: Record<string, unknown> }> } }> }>(page, token, `/executions/${firstExecution}/nodes`)
  const runtimeInformation = runtimeNodes.items.find((node) => node.nodeId === codeNodeId)?.output?.main?.[0].json
  const structuredInformation = runtimeInformation?.structuredOutput as Record<string, unknown> | undefined
  expect(structuredInformation).toMatchObject({
    workflow_name: workflowName,
  })
  const executionContextEvidencePath = process.env.AGENTX_EXECUTION_CONTEXT_EVIDENCE_OUTPUT
  if (!executionContextEvidencePath) throw new Error('AGENTX_EXECUTION_CONTEXT_EVIDENCE_OUTPUT is required')
  await writeFile(executionContextEvidencePath, JSON.stringify({
    executionId: firstExecution,
    workflowName,
    departmentName: me.departmentName,
    roleCodes: me.roles,
    contextWorkflowName: completedExecution.output?.context_workflow_name,
  }))

  const details = await openNodeDetails(page, code)
  await details.getByRole('tab', { name: '输出' }).click()
  await expect(details.getByRole('textbox', { name: '输出' })).toHaveValue(/Run the workflow from the Studio\./, { timeout: 30_000 })
  expect(JSON.parse(await details.getByRole('textbox', { name: '输出' }).inputValue()).main[0].json).toMatchObject({
    stdout: '', stderr: '', exitCode: 0,
    structuredOutput: { message: 'Run the workflow from the Studio.' },
  })
  await details.getByRole('tab', { name: 'Trace' }).click()
  await expect(details.getByRole('heading', { name: '输入与解析参数' })).toBeVisible({ timeout: 30_000 })
  await expect(details.getByRole('heading', { name: '语义输出' })).toBeVisible()
  await expect(details).toContainText('Run the workflow from the Studio.')
  const runtimeRail = page.getByTestId('runtime-rail')
  type ExecutionEvent = { sequence: number; eventType: string; status: string }
  let executionEvents: { items: ExecutionEvent[]; nextCursor?: number | null } | undefined
  await expect.poll(async () => {
    executionEvents = await api<{ items: ExecutionEvent[]; nextCursor?: number | null }>(page, token, `/executions/${firstExecution}/events?after=0&limit=200`)
    return executionEvents.items.at(-1)?.eventType
  }, { timeout: 30_000, intervals: [250, 500, 1_000] }).toBe('execution.succeeded')
  expect(executionEvents!.items.map((event) => event.sequence)).toEqual(executionEvents!.items.map((event) => event.sequence).sort((left, right) => left - right))
  expect(new Set(executionEvents!.items.map((event) => event.sequence)).size).toBe(executionEvents!.items.length)
  const incrementalAfter = executionEvents!.items[Math.min(3, executionEvents!.items.length - 2)].sequence
  const incrementalEvents = await api<{ items: ExecutionEvent[]; nextCursor?: number | null }>(page, token, `/executions/${firstExecution}/events?after=${incrementalAfter}&limit=2`)
  expect(incrementalEvents.items).toHaveLength(2)
  expect(incrementalEvents.items.every((event) => event.sequence > incrementalAfter)).toBe(true)
  expect(incrementalEvents.nextCursor).toBe(incrementalEvents.items.at(-1)?.sequence)
  await runtimeRail.getByRole('tab', { name: '事件', exact: true }).click()
  const executionEventsPanel = runtimeRail.getByTestId('execution-events')
  await expect(executionEventsPanel).toContainText('execution.succeeded')
  await expect(executionEventsPanel).not.toContainText('未知（reserved）')
  expect(await executionEventsPanel.evaluate((panel) => getComputedStyle(panel).overflowY)).toMatch(/auto|scroll/)
  await runtimeRail.getByRole('tab', { name: 'Trace' }).click()
  await expect(runtimeRail).toHaveCSS('height', '560px')
  const resizeHandle = runtimeRail.getByRole('button', { name: '调整运行面板高度' })
  const [runtimeRailBoxForHandle, resizeHandleBox] = await Promise.all([runtimeRail.boundingBox(), resizeHandle.boundingBox()])
  expect(runtimeRailBoxForHandle).toBeTruthy()
  expect(resizeHandleBox).toBeTruthy()
  expect(resizeHandleBox!.y).toBeLessThan(runtimeRailBoxForHandle!.y)
  expect(await resizeHandle.evaluate((handle) => {
    const box = handle.getBoundingClientRect()
    const hit = document.elementFromPoint(box.x + box.width / 2, box.y + 1)
    return hit === handle || handle.contains(hit)
  })).toBe(true)
  const executionToolbar = runtimeRail.getByTestId('execution-toolbar')
  await expect(executionToolbar).toBeVisible()
  await expect(executionToolbar).toContainText('执行记录')
  await expect(executionToolbar).toContainText('不会筛选 Trace 状态')
  await expect(executionToolbar).toHaveCSS('border-top-width', '1px')
  await expect(executionToolbar).toHaveCSS('border-bottom-width', '1px')
  const executionSelector = executionToolbar.getByRole('combobox', { name: '切换执行' })
  await expect(executionSelector).toContainText(/成功|运行中|失败|当前执行不在最近记录中/)
  if (await executionSelector.getByText(/当前执行不在最近记录中/).count()) await expect(executionSelector).toContainText(firstExecution)
  await expect(executionSelector).not.toContainText('—')
  const traceNodeView = runtimeRail.getByTestId('trace-node-view')
  await expect(traceNodeView).toBeVisible({ timeout: 30_000 })
  expect(await traceNodeView.evaluate((view) => {
    view.scrollTop = view.scrollHeight
    return getComputedStyle(view).overflowY === 'auto' && Math.ceil(view.scrollTop + view.clientHeight) >= view.scrollHeight
  })).toBe(true)
  await expect(runtimeRail.getByTestId('trace-start-boundary')).toBeVisible()
  await expect(runtimeRail.getByTestId('trace-end-boundary')).toBeVisible()
  await runtimeRail.getByRole('tab', { name: '高级瀑布' }).click()
  await expect(runtimeRail.getByRole('treegrid', { name: 'Trace 层级瀑布' })).toBeVisible({ timeout: 30_000 })
  type TraceSpan = { spanId: string; spanKind: string; spanName: string; status: string; inputTokens?: number; outputTokens?: number; costMicros: number; resourceType?: string }
  let traceSnapshot: { complete: boolean; spans: TraceSpan[] } | undefined
  await expect.poll(async () => {
    traceSnapshot = await api<{ complete: boolean; spans: TraceSpan[] }>(page, token, `/executions/${firstExecution}/trace?limit=200`)
    const kinds = new Set(traceSnapshot.spans.map((span) => span.spanKind))
    return ['execution', 'boundary', 'node', 'attempt', 'agent_run', 'agent_iteration', 'runtime_call', 'sandbox', 'wait'].filter((kind) => !kinds.has(kind))
  }, { timeout: 60_000, intervals: [500, 1_000, 2_000] }).toEqual([])
  expect(traceSnapshot?.complete).toBe(true)
  const modelCall = traceSnapshot?.spans.find((span) => span.spanKind === 'runtime_call' && span.spanName === 'Model call')
  const mcpCall = traceSnapshot?.spans.find((span) => span.spanKind === 'runtime_call' && span.resourceType === 'mcp' && span.spanName === 'echo')
  expect(modelCall).toMatchObject({ status: 'succeeded', resourceType: 'model' })
  expect(modelCall?.costMicros ?? 0).toBeGreaterThan(0)
  expect((modelCall?.inputTokens ?? 0) + (modelCall?.outputTokens ?? 0)).toBeGreaterThan(0)
  let runtimeDetails: { calls: Array<{ resourceType?: string; costMicros: number; costCurrency?: string }> } | undefined
  await expect.poll(async () => {
    try {
      const details = await api<{ calls: Array<{ resourceType?: string; costMicros: number; costCurrency?: string }> }>(page, token, `/executions/${firstExecution}/runtime-details`)
      runtimeDetails = details
      return details.calls.some((call) => call.resourceType === 'model')
    } catch {
      return false
    }
  }, { timeout: 30_000, intervals: [500, 1_000, 2_000] }).toBeTruthy()
  const runtimeModelCall = runtimeDetails!.calls.find((call) => call.resourceType === 'model')
  expect(runtimeModelCall).toMatchObject({ costCurrency: 'USD' })
  expect(runtimeModelCall?.costMicros).toBe(modelCall?.costMicros)
  expect(mcpCall).toMatchObject({ status: 'succeeded', resourceType: 'mcp' })
  expect(traceSnapshot?.spans.some((span) => span.spanKind === 'sandbox' && span.status === 'succeeded')).toBe(true)
  expect(traceSnapshot?.spans.find((span) => span.spanKind === 'wait')).toMatchObject({ status: 'decided' })
  const codeTraceRow = runtimeRail.getByRole('row').filter({ hasText: 'M6 Python Code' }).first()
  await expect(codeTraceRow).toBeVisible()
  await codeTraceRow.click()
  await page.screenshot({ path: testInfo.outputPath('trace-waterfall-skywalking.png'), fullPage: true })
  const waterfall = runtimeRail.getByTestId('trace-waterfall')
  await waterfall.getByRole('tab', { name: '原始数据' }).click()
  const rawTracePanel = waterfall.getByRole('tabpanel')
  await expect(rawTracePanel).toBeVisible()
  await expect(rawTracePanel).toContainText(workflowName)
  const rawTraceBox = await rawTracePanel.boundingBox()
  const runtimeRailBox = await runtimeRail.boundingBox()
  expect(rawTraceBox!.y + rawTraceBox!.height).toBeLessThanOrEqual(runtimeRailBox!.y + runtimeRailBox!.height + 1)
  expect(await rawTracePanel.evaluate((panel) => {
    panel.scrollTop = panel.scrollHeight
    return Math.ceil(panel.scrollTop + panel.clientHeight) >= panel.scrollHeight
  })).toBe(true)
  await waterfall.getByRole('tab', { name: '事件' }).click()
  await expect(waterfall.getByRole('tabpanel')).toContainText(/node\.(finished|completed)/)
  await runtimeRail.getByRole('button', { name: '折叠执行轨道' }).click()
  await expect(runtimeRail.getByRole('button', { name: '展开执行轨道' })).toBeVisible()
  await runtimeRail.getByRole('button', { name: '展开执行轨道' }).click()

  const executionPage = await context.newPage()
  await executionPage.goto(`/executions/${firstExecution}`)
  const executionTraceTab = executionPage.getByRole('tab', { name: 'Trace', exact: true })
  await expect(executionTraceTab).toHaveAttribute('aria-selected', 'true')
  await expect(executionPage.getByTestId('trace-node-view')).toBeVisible({ timeout: 30_000 })
  await executionPage.getByRole('tab', { name: '高级瀑布' }).click()
  await expect(executionPage.getByRole('treegrid', { name: 'Trace 层级瀑布' })).toBeVisible({ timeout: 30_000 })
  await executionPage.getByRole('row').filter({ hasText: 'M6 Python Code' }).first().click()
  await expect(executionPage.getByTestId('trace-detail').getByRole('tab')).toHaveCount(5)
  await executionPage.getByRole('tab', { name: '恢复', exact: true }).click()
  await expect(executionPage.getByRole('complementary', { name: '执行节点大纲' })).toHaveCount(0)
  await expect(executionPage.getByText(/检查点|Checkpoint/).first()).toBeVisible()
  await executionPage.close()

  const stableAgent = page.getByTestId(`rf__node-${draft.definition.nodes.find((node) => node.type === 'agent')!.id}`)
  const agentDetails = await openNodeDetails(page, stableAgent)
  await agentDetails.getByRole('tab', { name: '输出' }).click()
  await agentDetails.getByRole('textbox', { name: '输出' }).fill(JSON.stringify({ main: [{ json: { text: 'incomplete-agent-output' } }] }))
  const invalidMockResponse = page.waitForResponse((value) => value.url().includes('/debug-overlays/') && value.request().method() === 'PUT')
  await agentDetails.getByRole('button', { name: '模拟', exact: true }).click()
  expect((await invalidMockResponse).ok()).toBeTruthy()
  const contractFailureId = await startDebug(page, () => studioRun(page).click())
  const contractFailure = await waitExecution(page, token, contractFailureId, ['failed'], 30_000)
  expect(contractFailure.errorCode).toBe('NODE_OUTPUT_SCHEMA_VALIDATION_FAILED')
  const failedNodes = await api<{ items: Array<{ nodeId: string; status: string; errorCode?: string | null }> }>(page, token, `/executions/${contractFailureId}/nodes`)
  expect(failedNodes.items.filter((node) => node.nodeId === draft.definition.nodes.find((item) => item.type === 'agent')!.id)).toEqual([
    expect.objectContaining({ status: 'failed', errorCode: 'NODE_OUTPUT_SCHEMA_VALIDATION_FAILED' }),
  ])

  const restoredAgentDetails = await openNodeDetails(page, stableAgent)
  await restoredAgentDetails.getByRole('tab', { name: '输出' }).click()
  await restoredAgentDetails.getByRole('textbox', { name: '输出' }).fill(JSON.stringify({
    main: [{
      json: {
        text: 'm6-mock-output',
        reasoningContent: null,
        structuredOutput: null,
        files: [],
        citations: [],
        usage: { inputTokens: 0, outputTokens: 0, totalTokens: 0, costMicros: 0 },
        finishReason: 'stop', partial: false,
      },
    }],
  }))
  const mockResponse = page.waitForResponse((value) => value.url().includes('/debug-overlays/') && value.request().method() === 'PUT')
  await restoredAgentDetails.getByRole('button', { name: '模拟', exact: true }).click()
  expect((await mockResponse).ok()).toBeTruthy()

  const failedCodeDetails = await openNodeDetails(page, code)
  await failedCodeDetails.getByRole('tab', { name: '输出' }).click()
  await expect(failedCodeDetails.getByRole('button', { name: '固定', exact: true })).toBeDisabled()

  const restoredExecution = await startDebug(page, () => studioRun(page).click())
  await approveExecution(approvalPage, token, restoredExecution)
  await waitExecution(page, token, restoredExecution, ['succeeded'])

  const codeDetails = await openNodeDetails(page, code)
  await codeDetails.getByRole('tab', { name: '输出' }).click()
  await expect(codeDetails.getByRole('textbox', { name: '输出' })).toHaveValue(/Run the workflow from the Studio\./, { timeout: 30_000 })
  await expect(codeDetails.getByRole('button', { name: '固定', exact: true })).toBeEnabled()
  const pinResponse = page.waitForResponse((value) => value.url().includes('/debug-overlays/') && value.request().method() === 'PUT')
  await codeDetails.getByRole('button', { name: '固定', exact: true }).click()
  expect((await pinResponse).ok()).toBeTruthy()

  let failedEventPolls = 0
  await page.route('**/api/v1/executions/*/events?*', async (route) => {
    if (failedEventPolls++ < 2) await route.abort('connectionfailed')
    else await route.continue()
  })
  const overlayExecution = await startDebug(page, () => studioRun(page).click())
  await approveExecution(approvalPage, token, overlayExecution)
  await waitExecution(page, token, overlayExecution, ['succeeded'])
  await page.unroute('**/api/v1/executions/*/events?*')
  const overlayEvents = await api<{ items: Array<{ eventType: string }> }>(page, token, `/executions/${overlayExecution}/events?after=0&limit=200`)
  expect(overlayEvents.items.filter((item) => item.eventType === 'node.debug_overlay_applied')).toHaveLength(2)
  await expect(runtimeRail.getByRole('tab', { name: '事件', exact: true }).first()).toBeVisible()

  await runtimeRail.getByRole('tab', { name: 'Trace' }).click()
  const failedTraceRequest = page.waitForResponse((response) => response.url().includes(`/api/v1/executions/${contractFailureId}/trace?`) && response.request().method() === 'GET')
  await executionSelector.click()
  await page.locator(`[role="option"][data-option-value="${contractFailureId}"]`).click()
  await failedTraceRequest
  await expect(executionSelector).toContainText('失败')
  await expect(runtimeRail.getByTestId('trace-node-view')).toBeVisible()
  await expect(runtimeRail.getByTestId('trace-node-view')).toContainText('NODE_OUTPUT_SCHEMA_VALIDATION_FAILED')
  await runtimeRail.getByRole('tab', { name: '高级瀑布' }).click()
  await expect(runtimeRail.getByRole('treegrid', { name: 'Trace 层级瀑布' })).toBeVisible()

  await openNodeDetails(page, code)
  await selectDebugMode(page, 'single_node')
  const singleExecution = await startDebug(page, () => studioRun(page).click())
  await waitExecution(page, token, singleExecution, ['succeeded'])

  await selectDebugMode(page, 'to_node')
  const toExecution = await startDebug(page, () => studioRun(page).click())
  await waitExecution(page, token, toExecution, ['succeeded'])

  await selectDebugMode(page, 'from_node')
  const fromExecution = await startDebug(page, () => studioRun(page).click())
  await approveExecution(approvalPage, token, fromExecution)
  await waitExecution(page, token, fromExecution, ['succeeded'])

  await page.getByTestId('runtime-rail').getByRole('tab', { name: '检查点' }).click()
  const forkButton = page.getByRole('button', { name: 'Fork' }).last()
  await expect(forkButton).toBeVisible({ timeout: 30_000 })
  const forkResponse = page.waitForResponse((value) => value.url().endsWith(`/executions/${fromExecution}/fork`) && value.request().method() === 'POST')
  await forkButton.click()
  const forkAccepted = await forkResponse
  expect(forkAccepted.status()).toBe(202)
  const forkExecution = (await forkAccepted.json() as { executionId: string }).executionId
  await waitExecution(page, token, forkExecution, ['waiting', 'waiting_approval'])
  await approveExecution(approvalPage, token, forkExecution)
  await waitExecution(page, token, forkExecution, ['succeeded'])

  await selectDebugMode(page, 'full')
  const stoppedExecution = await startDebug(page, () => studioRun(page).click())
  await waitExecution(page, token, stoppedExecution, ['waiting', 'waiting_approval'])
  await page.getByRole('button', { name: '停止', exact: true }).click()
  await waitExecution(page, token, stoppedExecution, ['cancelled'])

  await page.getByRole('button', { name: '版本', exact: true }).click()
  const versionDialog = page.getByRole('dialog', { name: '工作流版本' })
  await versionDialog.getByRole('button', { name: '创建版本' }).click()
  await expect(page.getByText('版本已创建', { exact: true })).toBeVisible()
  const versions = await api<Array<{ id: string; definition: unknown }>>(page, token, `/workflows/${workflowId}/versions`)
  expect(versions).toHaveLength(1)
  expect(JSON.stringify(versions[0].definition)).not.toMatch(/pin_data|mock_output|debugOverlay/)

  await page.getByRole('button', { name: '发布', exact: true }).click()
  const publishDialog = page.getByRole('dialog', { name: '发布工作流' })
  await choose(page, publishDialog.getByLabel('环境'), /Development/)
  await choose(page, publishDialog.getByLabel('版本'), /v1/)
  await publishDialog.getByRole('button', { name: '发布', exact: true }).click()
  await expect(page.getByText('工作流已发布', { exact: true })).toBeVisible()

  const viewports = [{ width: 1280, height: 800 }, { width: 1440, height: 900 }, { width: 1920, height: 1080 }]
  for (const locale of ['zh-CN', 'en-US'] as const) {
    await setLocale(page, locale)
    await runtimeRail.getByRole('tab', { name: 'Trace' }).click()
    await runtimeRail.getByRole('tab', { name: locale === 'zh-CN' ? '高级瀑布' : 'Advanced waterfall' }).click()
    for (const theme of ['light', 'dark'] as const) {
      await setTheme(page, theme)
      for (const viewport of viewports) {
        await page.setViewportSize(viewport)
        await expect(page.getByTestId('workflow-canvas')).toBeVisible()
        await expect(runtimeRail.getByTestId('trace-waterfall')).toBeVisible()
        await expect(runtimeRail.getByRole('treegrid', { name: locale === 'zh-CN' ? 'Trace 层级瀑布' : 'Trace hierarchy waterfall' })).toBeVisible()
        await expect(runtimeRail.getByTestId('trace-detail')).toBeVisible()
        await expect(page.locator('body')).not.toContainText(/studio\.[A-Za-z]/)
        await expect(page.getByText('Invalid Date', { exact: true })).toHaveCount(0)
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy()
        await page.screenshot({ path: testInfo.outputPath(`studio-${viewport.width}x${viewport.height}-${locale}-${theme}.png`), fullPage: true })
      }
    }
    const versionsEndpoint = `/api/v1/workflows/${workflowId}/versions`
    await page.route(versionsEndpoint, (route) => route.fulfill({
      status: 422,
      contentType: 'application/json',
      body: JSON.stringify({ code: 'INVALID_WORKFLOW_DEFINITION', message: locale === 'zh-CN' ? 'Workflow definition is invalid' : '工作流定义无效', requestId: crypto.randomUUID() }),
    }))
    await page.getByRole('button', { name: locale === 'zh-CN' ? '版本' : 'Version', exact: true }).click()
    const localizedVersionDialog = page.getByRole('dialog', { name: locale === 'zh-CN' ? '工作流版本' : 'Workflow versions' })
    await localizedVersionDialog.getByRole('button', { name: locale === 'zh-CN' ? '创建版本' : 'Create version' }).click()
    await expect(page.getByText(locale === 'zh-CN' ? '工作流定义无效' : 'The workflow definition is invalid', { exact: true })).toBeVisible()
    await localizedVersionDialog.getByRole('button', { name: locale === 'zh-CN' ? '关闭' : 'Close' }).click()
    await page.unroute(versionsEndpoint)
  }

  const concurrent = await context.newPage()
  await concurrent.goto(`/workflows/${workflowId}/editor`)
  await expect(concurrent.getByTestId('workflow-canvas')).toBeVisible()
  const concurrentDetails = await openNodeDetails(concurrent, flowNode(concurrent, 'code'))
  await concurrentDetails.getByRole('tab', { name: 'Parameters' }).click()
  await openNodeBasics(concurrentDetails)
  await concurrentDetails.getByRole('textbox', { name: 'Name', exact: true }).fill('code concurrent')
  await concurrent.getByRole('button', { name: 'Save', exact: true }).click()
  await expect(concurrent.locator('header').getByText(/Revision \d+ · Saved/)).toBeVisible()
  const localDetails = await openNodeDetails(page, code)
  await localDetails.getByRole('tab', { name: 'Parameters' }).click()
  await openNodeBasics(localDetails)
  await localDetails.getByRole('textbox', { name: 'Name', exact: true }).fill('code local')
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await expect(page.getByRole('dialog', { name: 'Draft revision conflict' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Keep local copy' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Load server revision' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Overwrite server draft' })).toBeVisible()

  await concurrent.close()
  await approvalPage.close()
})

test('M6 Code executes JavaScript and Shell named-input contracts in OpenSandbox', async ({ page }, testInfo) => {
  const { token } = await login(page)
  await ensureStudioResources(page, token)
  const cases = [
    {
      runner: 'javascript',
      option: /JavaScript/,
      source: 'async function main({ name, count }) { return { name, count }; }',
    },
    {
      runner: 'shell',
      option: /Shell/,
      source: 'cat "$AGENTX_INPUT_PATH" > "$AGENTX_OUTPUT_PATH"',
    },
  ] as const

  for (const codeCase of cases) {
    await page.goto('/')
    const workflowName = `M6 Code ${codeCase.runner} ${Date.now()}`
    const workflowId = await createWorkflow(page, workflowName)
    await grantResource(page, 'sandbox_profile', studioSandboxName, undefined, workflowName)
    await page.goto(`/workflows/${workflowId}/editor`)
    await expect(page.getByTestId('workflow-canvas')).toBeVisible()
    await addFromCreator(page, 'palette-action-code')
    const code = flowNode(page, 'code')
    const details = await openNodeDetails(page, code)
    await choose(page, details.getByTestId('parameter-runner'), codeCase.option)
    const sourceControl = details.getByTestId('parameter-source')
    await expect(sourceControl.getByText(new RegExp(`^${codeCase.runner}$`, 'i')).first()).toBeVisible()
    const inputs = details.getByTestId('parameter-inputs')
    for (const [name, value] of [['name', 'Ada'], ['count', '2']] as const) {
      await inputs.getByRole('button', { name: /添加字段|Add field/ }).click()
      const key = inputs.getByRole('textbox', { name: /键|Key/ }).last()
      await key.fill(name)
      await key.blur()
      await inputs.getByRole('textbox', { name: 'Value' }).last().fill(value)
    }
    await fillMonaco(page, sourceControl, codeCase.source)
    await fillMonaco(page, details.getByTestId('code-output-example'), '{ name: "", count: 0 }')
    await choose(page, details.getByTestId('resource-selector-sandbox_profile'), new RegExp(studioSandboxName))
    await details.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()

    await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
    await connect(page, page.getByTestId('workflow-start'), 'main', code, 'main')
    await connect(page, code, 'main', page.getByTestId('exit-node-exit'), 'main')
    const draft = await saveAndReadDraft(page, token, workflowId)
    const codeDefinition = draft.definition.nodes.find((node) => node.type === 'code')!
    expect(codeDefinition.parameters).toMatchObject({
      runner: codeCase.runner,
      inputs: { kind: 'object', fields: {
        name: { kind: 'literal', value: 'Ada' },
        count: { kind: 'literal', value: 2 },
      } },
      source: codeCase.source,
      outputExample: { name: '', count: 0 },
    })

    await page.reload()
    await expect(page.getByTestId('workflow-canvas')).toBeVisible()
    const reopenedCode = page.getByTestId(`studio-node-${codeDefinition.id}`)
    await expect(reopenedCode.locator('.studio-card-body')).toContainText(codeCase.option)
    const executionId = await startDebug(page, () => studioRun(page).click())
    await waitExecution(page, token, executionId, ['succeeded'], 180_000)
    const runs = await api<{ items: Array<{ nodeId: string; output?: { main?: Array<{ json?: Record<string, unknown> }> } }> }>(page, token, `/executions/${executionId}/nodes`)
    const result = runs.items.find((node) => node.nodeId === codeDefinition.id)?.output?.main?.[0].json
    expect(result).toMatchObject({
      stdout: '',
      stderr: '',
      exitCode: 0,
      structuredOutput: { name: 'Ada', count: 2 },
    })
    await page.screenshot({ path: testInfo.outputPath(`code-${codeCase.runner}-real.png`), fullPage: true })
  }
})

test('M6 Code routes Python JavaScript and Shell HTTP and raw TCP through the allowlisted Egress Gateway', async ({ page }, testInfo) => {
  expect(codeEgressHost).toBeTruthy()
  expect(codeHttpPort).toBeGreaterThan(0)
  expect(codeTcpPort).toBeGreaterThan(0)
  const { token } = await login(page)
  await ensureStudioResources(page, token)
  const python = String.raw`import base64, json, os, socket, ssl, time
from urllib.parse import urlparse

def tunnel_once(host, port):
    proxy = urlparse(os.environ["AGENTX_TCP_PROXY_URL"])
    context = ssl.create_default_context(cafile=os.environ["SSL_CERT_FILE"])
    raw = socket.create_connection((proxy.hostname, proxy.port), timeout=20)
    connection = context.wrap_socket(raw, server_hostname=proxy.hostname)
    auth = base64.b64encode(f"{proxy.username}:{proxy.password}".encode()).decode()
    request = f"CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
    connection.sendall(request.encode())
    response = b""
    while b"\r\n\r\n" not in response:
        response += connection.recv(4096)
    if not response.startswith(b"HTTP/1.1 200"):
        raise RuntimeError(response.split(b"\r\n", 1)[0].decode())
    return connection

def tunnel(host, port):
    for attempt in range(3):
        try:
            return tunnel_once(host, port)
        except (OSError, TimeoutError):
            if attempt == 2:
                raise
            time.sleep(1)

def receive(socket):
    chunks = []
    while True:
        chunk = socket.recv(4096)
        if not chunk:
            return b"".join(chunks)
        chunks.append(chunk)

def main(**inputs):
    http = tunnel(inputs["host"], inputs["http_port"])
    http.sendall(b"GET /health HTTP/1.1\r\nHost: fixture\r\nConnection: close\r\n\r\n")
    body = receive(http).split(b"\r\n\r\n", 1)[1]
    http.close()
    tcp = tunnel(inputs["host"], inputs["tcp_port"])
    tcp.sendall(b"ping\n")
    tcp_value = tcp.recv(4096).decode().strip()
    tcp.close()
    return {"http": json.loads(body)["status"], "tcp": tcp_value}`
  const javascript = String.raw`const fs = require('fs')
const https = require('https')

function tunnelOnce(host, port) {
  return new Promise((resolve, reject) => {
    const proxy = new URL(process.env.AGENTX_TCP_PROXY_URL)
    const auth = Buffer.from(decodeURIComponent(proxy.username) + ':' + decodeURIComponent(proxy.password)).toString('base64')
    const request = https.request({ hostname: proxy.hostname, port: proxy.port, method: 'CONNECT', path: host + ':' + port, ca: fs.readFileSync(process.env.NODE_EXTRA_CA_CERTS), headers: { 'Proxy-Authorization': 'Basic ' + auth } })
    request.on('connect', (response, socket) => response.statusCode === 200 ? resolve(socket) : reject(new Error('CONNECT ' + response.statusCode)))
    request.on('error', reject)
    request.setTimeout(20000, () => request.destroy(new Error('proxy TLS timeout')))
    request.end()
  })
}

async function tunnel(host, port) {
  let error
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try { return await tunnelOnce(host, port) } catch (cause) { error = cause }
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }
  throw error
}

function receive(socket) {
  return new Promise((resolve, reject) => {
    const chunks = []
    socket.on('data', (chunk) => chunks.push(chunk))
    socket.on('end', () => resolve(Buffer.concat(chunks)))
    socket.on('error', reject)
  })
}

async function main(inputs) {
  const httpSocket = await tunnel(inputs.host, inputs.http_port)
  const httpDone = receive(httpSocket)
  httpSocket.write('GET /health HTTP/1.1\r\nHost: fixture\r\nConnection: close\r\n\r\n')
  const httpResponse = (await httpDone).toString()
  const separator = httpResponse.indexOf('\r\n\r\n')
  if (separator < 0) throw new Error('HTTP fixture returned no header boundary')
  const body = httpResponse.slice(separator + 4)
  const tcpSocket = await tunnel(inputs.host, inputs.tcp_port)
  const tcpResponse = await new Promise((resolve, reject) => {
    tcpSocket.once('data', (chunk) => { resolve(chunk.toString().trim()); tcpSocket.destroy() })
    tcpSocket.once('error', reject)
    tcpSocket.write('ping\n')
  })
  return { http: JSON.parse(body).status, tcp: tcpResponse }
}`
  const shell = String.raw`host="$(python3 -c 'import json,os; print(json.load(open(os.environ["AGENTX_INPUT_PATH"]))["host"])')"
http_port="$(python3 -c 'import json,os; print(json.load(open(os.environ["AGENTX_INPUT_PATH"]))["http_port"])')"
tcp_port="$(python3 -c 'import json,os; print(json.load(open(os.environ["AGENTX_INPUT_PATH"]))["tcp_port"])')"
http_value="$(curl --silent --show-error --fail --retry 2 --retry-delay 1 --retry-all-errors --connect-timeout 20 --proxy "$AGENTX_TCP_PROXY_URL" --proxytunnel "http://$host:$http_port/health")"
printf '%s' "$http_value" | grep -q '"status":"ok"'
tcp_value="$(python3 - "$host" "$tcp_port" <<'PY'
import base64, os, socket, ssl, sys, time
from urllib.parse import urlparse
host, port = sys.argv[1], int(sys.argv[2])
proxy = urlparse(os.environ["AGENTX_TCP_PROXY_URL"])
for attempt in range(3):
    try:
        raw = socket.create_connection((proxy.hostname, proxy.port), timeout=20)
        stream = ssl.create_default_context(cafile=os.environ["SSL_CERT_FILE"]).wrap_socket(raw, server_hostname=proxy.hostname)
        break
    except (OSError, TimeoutError):
        if attempt == 2:
            raise
        time.sleep(1)
auth = base64.b64encode(f"{proxy.username}:{proxy.password}".encode()).decode()
stream.sendall(f"CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\nProxy-Authorization: Basic {auth}\r\n\r\n".encode())
response = b""
while b"\r\n\r\n" not in response:
    response += stream.recv(4096)
if not response.startswith(b"HTTP/1.1 200"):
    raise RuntimeError(response.split(b"\r\n", 1)[0].decode())
stream.sendall(b"ping\n")
print(stream.recv(4096).decode().strip())
PY
)"
printf '{"http":"ok","tcp":"%s"}' "$tcp_value" > "$AGENTX_OUTPUT_PATH"`
  const workflowName = `M6 Gateway runners ${Date.now()}`
  const workflowId = await createWorkflow(page, workflowName)
  await grantResource(page, 'sandbox_profile', studioNetworkSandboxName, undefined, workflowName)
  await page.goto(`/workflows/${workflowId}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  await addFromCreator(page, 'palette-action-code')
  const code = flowNode(page, 'code')
  const details = await openNodeDetails(page, code)
  const inputs = details.getByTestId('parameter-inputs')
  for (const [name, value] of [['host', codeEgressHost], ['http_port', String(codeHttpPort)], ['tcp_port', String(codeTcpPort)]] as const) {
    await inputs.getByRole('button', { name: /添加字段|Add field/ }).click()
    const row = inputs.getByTestId('mapper-control').locator(':scope > div').last()
    const key = row.getByRole('textbox', { name: /键|Key/ })
    await key.fill(name)
    await key.blur()
    await row.getByRole('textbox', { name: 'Value' }).fill(value)
  }
  const network = details.getByTestId('code-network-policy')
  await network.getByTestId('mode-card-allowlist').click()
  await network.getByPlaceholder(/api\.example\.com/).fill(codeEgressHost)
  const ports = network.getByPlaceholder(/443, 8000-8010/)
  await ports.fill(`${codeHttpPort}, ${codeTcpPort}`)
  await ports.blur()
  await network.getByText(/查看代理使用示例|Show proxy example/).click()
  await expect(network.getByRole('textbox', { name: 'Editor content' })).toBeVisible()
  await fillMonaco(page, details.getByTestId('parameter-source'), python, true)
  await fillMonaco(page, details.getByTestId('code-output-example'), '{ http: "", tcp: "" }')
  await choose(page, details.getByTestId('resource-selector-sandbox_profile'), new RegExp(studioNetworkSandboxName))
  await details.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  await connect(page, page.getByTestId('workflow-start'), 'main', code, 'main')
  await connect(page, code, 'main', page.getByTestId('exit-node-exit'), 'main')
  let activeDraft = await saveAndReadDraft(page, token, workflowId)
  const codeNodeId = activeDraft.definition.nodes.find((node) => node.type === 'code')!.id
  const fullPolicy = { mode: 'allowlist', destinations: [{ target: codeEgressHost, ports: [{ from: codeHttpPort, to: codeHttpPort }, { from: codeTcpPort, to: codeTcpPort }] }] }
  expect(activeDraft.definition.nodes.find((node) => node.id === codeNodeId)!.parameters).toMatchObject({ runner: 'python', source: python, networkPolicy: fullPolicy })

  const replaceCode = async (runner: string, source: string, networkPolicy: unknown) => {
    const definition = structuredClone(activeDraft.definition)
    const codeDefinition = definition.nodes.find((node) => node.id === codeNodeId)!
    codeDefinition.parameters = { ...codeDefinition.parameters, runner, source, networkPolicy }
    const response = await page.request.put(`/api/v1/workflows/${workflowId}/draft`, {
      headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': `m6-egress-runner-${runner}-${Date.now()}` },
      data: { expectedRevision: activeDraft.revision, definition, editorDocument: activeDraft.editorDocument },
    })
    if (!response.ok()) throw new Error(`replace Code runner: ${response.status()} ${await response.text()}`)
    activeDraft = await response.json() as StudioDraft
  }
  const runCode = async (runner: string) => {
    await page.reload()
    await expect(page.getByTestId('workflow-canvas')).toBeVisible()
    const executionId = await startDebug(page, () => studioRun(page).click())
    await waitExecution(page, token, executionId, ['succeeded'], 180_000)
    const runs = await api<{ items: Array<{ nodeId: string; output?: { main?: Array<{ json?: Record<string, unknown> }> } }> }>(page, token, `/executions/${executionId}/nodes`)
    const result = runs.items.find((node) => node.nodeId === codeNodeId)?.output?.main?.[0].json
    expect(result?.structuredOutput).toEqual({ http: 'ok', tcp: 'tcp:ping' })
    expect(JSON.stringify(result)).not.toContain('eyJ')
    await page.screenshot({ path: testInfo.outputPath(`code-gateway-${runner}.png`), fullPage: true })
  }

  await runCode('python')
  await replaceCode('javascript', javascript, fullPolicy)
  await runCode('javascript')
  await replaceCode('shell', shell, fullPolicy)
  await runCode('shell')
  await replaceCode('python', python, { mode: 'allowlist', destinations: [{ target: codeEgressHost, ports: [{ from: codeHttpPort, to: codeHttpPort }] }] })
  await page.reload()
  const deniedExecution = await startDebug(page, () => studioRun(page).click())
  const denied = await waitExecution(page, token, deniedExecution, ['failed'], 180_000)
  expect(denied.errorCode).toBeTruthy()

  for (const target of ['169.254.169.254', 'kubernetes.default.svc', 'agentx-egress-gateway.agentx-deps.svc']) {
    const definition = structuredClone(activeDraft.definition)
    definition.nodes.find((node) => node.type === 'code')!.parameters.networkPolicy = {
      mode: 'allowlist', destinations: [{ target, ports: [{ from: 443, to: 443 }] }],
    }
    const response = await page.request.put(`/api/v1/workflows/${workflowId}/draft`, {
      headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': `m6-protected-egress-${target}-${Date.now()}` },
      data: { expectedRevision: activeDraft.revision, definition, editorDocument: activeDraft.editorDocument },
    })
    const body = await response.json() as { fieldErrors?: Array<{ code: string }> }
    expect(response.status(), JSON.stringify(body)).toBe(422)
    expect(body.fieldErrors).toEqual(expect.arrayContaining([expect.objectContaining({ code: 'CODE_NETWORK_TARGET_FORBIDDEN' })]))
  }
})

test('M6 standalone Model sends prompt and question as ordered messages and exposes text', async ({ page }) => {
  const { token } = await login(page)
  await ensureStudioResources(page, token)
  const workflowName = `M6 Model Contract ${Date.now()}`
  const workflowId = await createWorkflow(page, workflowName)
  await grantResource(page, 'credential', studioCredentialName, undefined, workflowName)
  await grantResource(page, 'model', studioModelName, undefined, workflowName)

  await page.goto(`/workflows/${workflowId}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  await setStartInputs(page)
  await addFromCreator(page, 'palette-action-model')
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  const model = page.locator('.react-flow__node-manifest').filter({ hasText: /模型|Model/ }).first()
  const details = await openNodeDetails(page, model)
  await choose(page, details.getByTestId('resource-selector-model'), new RegExp(studioModelName))
  await fillNativeText(page, details.getByTestId('parameter-prompt'), '你叫 kakj\nTRACE_LARGE_RESPONSE')
  await chooseReference(page, details.getByTestId('parameter-userQuestion'), /输入|Inputs/, ['question'])
  await openNodeBasics(details)
  const modelKey = await details.getByLabel(/引用键|Reference key/).inputValue()
  await details.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()

  await connect(page, page.getByTestId('workflow-start'), 'main', model, 'main')
  await connect(page, model, 'main', page.getByTestId('exit-node-exit'), 'main')
  await setEndOutput(page, modelKey, 'json.text')
  const draft = await saveAndReadDraft(page, token, workflowId)
  const modelDefinition = draft.definition.nodes.find((node) => node.type === 'model')
  expect(modelDefinition?.parameters).toMatchObject({
    prompt: { kind: 'literal', value: '你叫 kakj\nTRACE_LARGE_RESPONSE' },
    userQuestion: { kind: 'reference', selector: expect.objectContaining({ namespace: 'inputs', path: ['question'] }) },
  })
  const modelExitParameters = draft.definition.nodes.find((node) => node.type === 'exit')?.parameters as { outputs: Record<string, unknown> }
  expect(modelExitParameters.outputs.answer).toMatchObject({
    kind: 'reference',
    selector: expect.objectContaining({ sourceNodeId: modelDefinition?.id, path: ['text'] }),
  })

  const executionId = await startDebug(page, () => studioRun(page).click())
  await waitExecution(page, token, executionId, ['succeeded'])
  let modelSpan: { spanId: string } | undefined
  await expect.poll(async () => {
    const trace = await api<{ spans: Array<{ spanId: string; spanKind: string; spanName: string }> }>(page, token, `/executions/${executionId}/trace?limit=100`)
    modelSpan = trace.spans.find((span) => span.spanKind === 'runtime_call' && span.spanName === 'Model call')
    return modelSpan?.spanId
  }, { timeout: 60_000, intervals: [500, 1_000, 2_000] }).toBeTruthy()
  const nodeRuns = await api<{ items: Array<{ nodeId: string; costMicros: number; costCurrency?: string; startedAt?: string; endedAt?: string; output?: { main?: Array<{ json?: Record<string, unknown> }> } }> }>(page, token, `/executions/${executionId}/nodes`)
  const modelRun = nodeRuns.items.find((node) => node.nodeId === modelDefinition?.id)
  const modelOutput = modelRun?.output?.main?.[0].json
  expect(modelOutput).toMatchObject({ text: expect.any(String), reasoningContent: null, structuredOutput: null, files: [], citations: [], partial: false })
  expect(modelRun).toMatchObject({ costMicros: 390, costCurrency: 'USD', startedAt: expect.any(String), endedAt: expect.any(String) })
  expect(modelOutput).not.toHaveProperty('message')
  expect(modelOutput).not.toHaveProperty('messages')
  expect(modelOutput).not.toHaveProperty('toolCalls')
  expect(modelOutput).not.toHaveProperty('providerRawResponse')

  let traceDetail: {
    contents: Array<{ kind: string; preview?: { messages?: Array<{ role: string; content: string }>; choices?: unknown[] }; contentRef?: string | null }>
  } | undefined
  await expect.poll(async () => {
    const latest = await api<NonNullable<typeof traceDetail>>(page, token, `/executions/${executionId}/trace/spans/${modelSpan!.spanId}`)
    traceDetail = latest
    return latest.contents.find((content) => content.kind === 'runtime_response')?.contentRef
  }, { timeout: 60_000, intervals: [250, 500, 1_000, 2_000] }).toMatch(/^[0-9a-f-]{36}$/)
  const settledTraceDetail = traceDetail!
  const requestContent = settledTraceDetail.contents.find((content) => content.kind === 'runtime_request')
  const responseContent = settledTraceDetail.contents.find((content) => content.kind === 'runtime_response')
  expect(requestContent?.preview?.messages).toEqual([
    { role: 'system', content: '你叫 kakj\nTRACE_LARGE_RESPONSE' },
    { role: 'user', content: 'Run the workflow from the Studio.' },
  ])
  expect(responseContent?.preview).toBeNull()
  expect(responseContent?.contentRef).toMatch(/^[0-9a-f-]{36}$/)
  const traceJson = JSON.stringify(settledTraceDetail)
  expect(traceJson).not.toContain('m5-model-secret')
  expect(traceJson.toLowerCase()).not.toContain('authorization')

  type ModelExecutionEvent = { sequence: number; eventType: string; status: string }
  let modelEvents: { items: ModelExecutionEvent[]; nextCursor?: number | null } | undefined
  await expect.poll(async () => {
    modelEvents = await api<{ items: ModelExecutionEvent[]; nextCursor?: number | null }>(page, token, `/executions/${executionId}/events?after=0&limit=200`)
    return modelEvents.items.at(-1)?.eventType
  }, { timeout: 30_000, intervals: [250, 500, 1_000] }).toBe('execution.succeeded')
  expect(modelEvents!.items.map((event) => event.sequence)).toEqual(modelEvents!.items.map((event) => event.sequence).sort((left, right) => left - right))
  const modelIncrementalEvents = await api<{ items: ModelExecutionEvent[]; nextCursor?: number | null }>(page, token, `/executions/${executionId}/events?after=4&limit=2`)
  expect(modelIncrementalEvents.items).toHaveLength(2)
  expect(modelIncrementalEvents.items.every((event) => event.sequence > 4)).toBe(true)
  expect(modelIncrementalEvents.nextCursor).toBe(modelIncrementalEvents.items.at(-1)?.sequence)

  const modelRuntimeRail = page.getByTestId('runtime-rail')
  await modelRuntimeRail.getByRole('tab', { name: '事件', exact: true }).click()
  await expect(modelRuntimeRail.getByTestId('execution-events')).toContainText('execution.succeeded')
  await expect(modelRuntimeRail.getByTestId('execution-events')).not.toContainText('未知（reserved）')
  await modelRuntimeRail.getByRole('tab', { name: 'Trace' }).click()
  await expect(modelRuntimeRail).toHaveCSS('height', '560px')
  await expect(modelRuntimeRail.getByTestId('trace-final-output')).toContainText(/kakj/i)
  await expect(modelRuntimeRail.getByTestId('trace-node-view')).toContainText(/text|kakj/i)
  const modelNodeCard = modelRuntimeRail.getByTestId('trace-node-card').filter({ hasText: /模型|Model/ }).first()
  await expect(modelNodeCard).toContainText(/\$0\.000390/)
  expect(await modelNodeCard.locator('button').first().getAttribute('class')).toContain('grid-cols-[minmax(160px,1fr)_auto_auto]')
  await modelRuntimeRail.getByRole('tab', { name: '高级瀑布' }).click()
  const modelWaterfall = modelRuntimeRail.getByTestId('trace-waterfall')
  const modelCallRow = modelWaterfall.getByRole('row').filter({ hasText: 'Model call' }).first()
  await expect(modelCallRow).toBeVisible({ timeout: 30_000 })
  await modelCallRow.click()
  await modelWaterfall.getByRole('tab', { name: 'Provider / Runtime 响应' }).click()
  const artifactDownload = page.waitForEvent('download')
  await modelWaterfall.getByRole('button', { name: /Artifact/ }).click()
  const artifact = await artifactDownload
  expect(artifact.suggestedFilename()).toBe(`${responseContent!.contentRef}.json`)
  const artifactPath = await artifact.path()
  expect(artifactPath).toBeTruthy()
  const artifactText = await readFile(artifactPath!, 'utf8')
  expect(artifactText.length).toBeGreaterThan(16 * 1024)
  expect((JSON.parse(artifactText) as { choices?: unknown[] }).choices).toHaveLength(1)
  expect(artifactText).toContain('trace-artifact-marker')
  expect(artifactText).not.toContain('m5-model-secret')
  expect(artifactText.toLowerCase()).not.toContain('authorization')
  await modelWaterfall.getByRole('tab', { name: '原始数据' }).click()
  const modelRawPanel = modelWaterfall.getByRole('tabpanel')
  const modelRawBox = await modelRawPanel.boundingBox()
  const modelRailBox = await modelRuntimeRail.boundingBox()
  expect(modelRawBox!.y + modelRawBox!.height).toBeLessThanOrEqual(modelRailBox!.y + modelRailBox!.height + 1)
  expect(await modelRawPanel.evaluate((panel) => {
    panel.scrollTop = panel.scrollHeight
    return Math.ceil(panel.scrollTop + panel.clientHeight) >= panel.scrollHeight
  })).toBe(true)
})

test('M6 Model validates native JSON Schema output without text parsing fallback', async ({ page }) => {
  const { token } = await login(page)
  await ensureStudioResources(page, token)
  const workflowName = `M6 Structured Model ${Date.now()}`
  const workflowId = await createWorkflow(page, workflowName)
  await grantResource(page, 'credential', studioCredentialName, undefined, workflowName)
  await grantResource(page, 'model', studioModelName, undefined, workflowName)

  await page.goto(`/workflows/${workflowId}/editor`)
  await setStartInputs(page)
  await addFromCreator(page, 'palette-action-model')
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  const model = page.locator('.react-flow__node-manifest').filter({ hasText: /模型|Model/ }).first()
  const details = await openNodeDetails(page, model)
  await choose(page, details.getByTestId('resource-selector-model'), new RegExp(studioModelName))
  await fillNativeText(page, details.getByTestId('parameter-prompt'), 'Return the requested JSON object.')
  await chooseReference(page, details.getByTestId('parameter-userQuestion'), /输入|Inputs/, ['question'])
  await details.getByTestId('parameter-responseMode').getByRole('combobox').click()
  await page.getByRole('option', { name: /JSON Schema/i }).click()
  await details.getByTestId('model-schema-template').click()
  await openNodeBasics(details)
  const modelKey = await details.getByLabel(/引用键|Reference key/).inputValue()
  await details.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()

  await connect(page, page.getByTestId('workflow-start'), 'main', model, 'main')
  await connect(page, model, 'main', page.getByTestId('exit-node-exit'), 'main')
  await setEndOutput(page, modelKey, 'json.structuredOutput.answer')
  const draft = await saveAndReadDraft(page, token, workflowId)
  const modelDefinition = draft.definition.nodes.find((node) => node.type === 'model')
  expect(modelDefinition?.parameters).toMatchObject({ responseMode: 'json_schema', structuredSchema: { type: 'object', required: ['answer'] } })

  const executionId = await startDebug(page, () => studioRun(page).click())
  await waitExecution(page, token, executionId, ['succeeded'])
  const runs = await api<{ items: Array<{ nodeId: string; output?: { main?: Array<{ json?: Record<string, unknown> }> } }> }>(page, token, `/executions/${executionId}/nodes`)
  expect(runs.items.find((node) => node.nodeId === modelDefinition?.id)?.output?.main?.[0].json?.structuredOutput).toEqual({ answer: 'structured-value' })
})

test('M6 standalone Approval resumes with the frozen decision output contract', async ({ page }) => {
  const { token, userName } = await login(page)
  const workflowName = `M6 Approval Contract ${Date.now()}`
  const workflowId = await createWorkflow(page, workflowName)
  await page.goto(`/workflows/${workflowId}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  await setStartInputs(page)
  await addFromCreator(page, 'palette-action-approval')
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  const approval = page.locator('.react-flow__node-manifest').filter({ hasText: /审批|Approval/ }).first()
  const details = await openNodeDetails(page, approval)
  await fillNativeText(page, details.getByTestId('parameter-title'), 'M6 Standalone Approval')
  await page.keyboard.press('Escape')
  await expect(page.getByTestId('reference-picker')).toBeHidden()
  await details.getByTestId('parameter-candidateUserId').getByRole('combobox').click()
  await page.getByRole('option', { name: userName }).click()
  await details.getByTestId('buttons-editor').getByRole('button', { name: /添加按钮|Add button/ }).click()
  await details.getByTestId('buttons-editor').getByLabel(/按钮文本|Button label/).last().fill('升级处理')
  await details.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()

  await saveAndReadDraft(page, token, workflowId)
  await page.goto(`/workflows/${workflowId}/editor`)
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  const configuredApproval = page.locator('.react-flow__node-manifest').filter({ hasText: /审批|Approval/ }).first()
  await connect(page, page.getByTestId('workflow-start'), 'main', configuredApproval, 'main')
  const thirdHandle = await configuredApproval.locator('.react-flow__handle.source[data-handleid^="decision:decision_"]').getAttribute('data-handleid')
  expect(thirdHandle).toBeTruthy()
  await configuredApproval.getByRole('button', { name: /升级处理.*添加节点|Add node after 升级处理/i }).click()
  await expect(page.getByTestId('palette-exit')).toBeVisible()
  await page.getByTestId('palette-exit').click()
  const decisionExit = page.getByTestId('exit-node-exit_2')
  await expect(decisionExit).toBeVisible()
  await page.getByTestId('exit-panel').getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
  await expect(page.locator('.react-flow__edge')).toHaveCount(2)
  const draft = await saveAndReadDraft(page, token, workflowId)
  const approvalDefinition = draft.definition.nodes.find((node) => node.type === 'approval')
  expect(approvalDefinition?.parameters).toMatchObject({ title: { kind: 'literal', value: 'M6 Standalone Approval' }, buttons: expect.arrayContaining([expect.objectContaining({ id: thirdHandle!.slice('decision:'.length), label: '升级处理' })]) })
  expect(draft.definition.connections).toEqual(expect.arrayContaining([expect.objectContaining({ sourceNodeId: approvalDefinition?.id, sourceHandle: thirdHandle, targetHandle: 'main' })]))

  const executionId = await startDebug(page, () => studioRun(page).click())
  let task: Approval | undefined
  await expect.poll(async () => {
    const result = await api<{ items: Approval[] }>(page, token, '/approvals?pageSize=100')
    task = result.items.find((item) => item.executionId === executionId && item.status === 'pending')
    return task?.id
  }, { timeout: 120_000 }).toBeTruthy()
  await page.goto(`/approvals/${task!.id}`)
  await page.getByRole('button', { name: '领取' }).click()
  await page.getByRole('button', { name: '升级处理' }).click()
  await page.getByRole('dialog', { name: '升级处理' }).getByRole('button', { name: '升级处理' }).click()
  await expect.poll(async () => (await api<{ items: Approval[] }>(page, token, '/approvals?pageSize=100')).items.find((item) => item.id === task!.id)?.status).toBe('decided')
  await waitExecution(page, token, executionId, ['succeeded'])
  const runs = await api<{ items: Array<{ nodeId: string; output?: Record<string, Array<{ json?: Record<string, unknown> }>> }> }>(page, token, `/executions/${executionId}/nodes`)
  const decided = runs.items.find((node) => node.nodeId === approvalDefinition?.id)?.output?.[thirdHandle!]?.[0].json
  expect(decided).toMatchObject({ decision: thirdHandle!.slice('decision:'.length), taskId: expect.any(String), decidedBy: expect.any(String) })
})


test('M6 Studio renders all thirteen dedicated node panels in the focused editor', async ({ page }, testInfo) => {
  const { token, userName } = await login(page)
  await ensureStudioResources(page, token)

  // A fixed child version is a provider fixture for the Sub-workflow panel;
  // the Workflow and Version themselves are still created through visible UI.
  const childName = 'M6 Panel Child Golden'
  const childId = await createWorkflow(page, childName)
  await page.goto(`/workflows/${childId}/editor`)
  await saveAndReadDraft(page, token, childId)
  await page.getByRole('button', { name: '版本', exact: true }).click()
  const childVersions = page.getByRole('dialog', { name: '工作流版本' })
  await childVersions.getByRole('button', { name: '创建版本' }).click()
  await expect(page.getByText('版本已创建', { exact: true })).toBeVisible()
  await page.goto('/')

  const workflowName = 'M6 Panel Matrix Golden'
  const workflowId = await createWorkflow(page, workflowName)
  await grantResource(page, 'credential', studioCredentialName, undefined, workflowName)
  await grantResource(page, 'model', studioModelName, undefined, workflowName)
  await grantResource(page, 'sandbox_profile', studioSandboxName, undefined, workflowName)
  await page.goto(`/workflows/${workflowId}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()

  await setStartArrayInput(page)
  const exitPanel = await openExitPanel(page)
  await setExitContractField(page, exitPanel, 'panel_state', false)
  await exitPanel.getByTestId('exit-mapping-panel_state').getByRole('textbox', { name: 'Value' }).fill('configured')
  await exitPanel.getByRole('button', { name: /^(关闭|Close)$/ }).click()

  const panels = [
    ['model', 'model-panel'], ['agent', 'agent-panel'], ['if', 'if-panel'],
    ['loop_over_items', 'loop-panel'], ['merge', 'merge-panel'], ['approval', 'approval-panel'],
    ['code', 'code-panel'], ['set', 'set-panel'], ['list', 'list-panel'],
    ['declarative_http', 'http-panel'], ['sub_workflow', 'subworkflow-panel'],
  ] as const
  for (const [nodeType, panelId] of panels) {
    await addFromCreator(page, `palette-action-${nodeType}`)
    const details = page.getByTestId('node-details-view')
    const panel = details.getByTestId(panelId)
    const node = page.locator('.react-flow__node.selected').first()
    await expect(panel).toBeVisible()
    switch (nodeType) {
      case 'model':
        await choose(page, panel.getByTestId('resource-selector-model'), new RegExp(studioModelName))
        await choose(page, panel.getByTestId('parameter-responseMode'), /JSON Schema|结构化/)
        await panel.getByTestId('model-schema-template').click()
        await expect(node.locator('.studio-card-body')).toContainText(/结构化 JSON|Structured JSON/)
        break
      case 'agent':
        await choose(page, panel.getByTestId('agent-inspector-model'), new RegExp(studioModelName))
        await chooseAgentSessionPolicy(page, panel)
        await panel.getByTestId('agent-budget-section').getByRole('button', { name: /展开或收起高级配置|Expand or collapse advanced settings/ }).click()
        await panel.getByTestId('parameter-maxIterations').getByRole('spinbutton').fill('4')
        await expect(node.locator('.studio-card-body')).toContainText(/最多 4 轮|Max 4 iterations/)
        break
      case 'if':
        await configureReferenceIfCase(page, details, workflowName)
        await expect(node.locator('.studio-branch-row').first()).toContainText('IF')
        break
      case 'loop_over_items':
        const loopId = await node.getAttribute('data-id')
        await expect(node.getByText(/^(输出|Output)$/)).toBeVisible()
        await expect(node.getByText(/^(错误|Error)$/)).toBeVisible()
        await expect(node.getByRole('button', { name: /输出.*添加节点|Add node after.*Output/i })).toBeVisible()
        await expect(node.getByRole('button', { name: /错误.*添加节点|Add node after.*Error/i })).toBeVisible()
        await expect(page.locator('[data-testid^="studio-end-chip-"]').last().locator('.react-flow__handle.target')).toHaveCount(2)
        await details.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
        await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
        await expect(page.locator('[data-testid^="studio-chip-"]').last()).toBeVisible()
        await expect(page.locator('[data-testid^="studio-end-chip-"]').last()).toBeVisible()
        const loopFrame = page.getByTestId(`studio-loop-${loopId}`)
        const resizeHandle = loopFrame.locator('.studio-loop-resize-handle.right.bottom')
        await resizeLoop(page, loopFrame, resizeHandle)
        await page.getByRole('button', { name: /添加循环体节点|Add loop body node/ }).last().click()
        await addFromCreator(page, 'palette-action-set')
        const firstLoopStep = page.locator('.react-flow__node.selected')
        await firstLoopStep.getByRole('button', { name: /输出.*添加节点|Add node after.*Output/i }).click()
        await addFromCreator(page, 'palette-action-set')
        await openNodeDetails(page, page.locator(`.react-flow__node[data-id="${loopId}"]`))
        await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
        await page.waitForTimeout(750)
        await panel.getByTestId('parameter-input').getByRole('textbox', { name: 'Value' }).click()
        await expect(page.getByTestId('reference-picker')).toBeVisible()
        const loopPickerSnapshot = process.platform === 'linux' ? 'loop-variable-picker-light-zh-linux.png' : 'loop-variable-picker-light-zh.png'
        await expect(page.getByTestId('node-details-view')).toHaveScreenshot(loopPickerSnapshot, {
          animations: 'disabled',
          maxDiffPixelRatio: 0.002,
        })
        await selectOpenReference(page, /输入|Inputs/, ['items'])
        await chooseDirectReference(page, panel.getByTestId('parameter-outputSelector'), /循环|Loop/, ['items'])
        await panel.getByTestId('mode-card-continue').click()
        await panel.getByTestId('parameter-parallelism').getByRole('spinbutton').fill('2')
        await expect(panel.getByTestId('mode-card-continue')).toHaveClass(/border-primary/)
        await expect(page.locator('[data-testid^="studio-loop-parallel-"]').last()).toContainText('2')
        break
      case 'merge':
        await panel.getByTestId('mode-card-append').click()
        await expect(node.locator('.studio-card-body')).toContainText(/追加|Append/)
        break
      case 'approval':
        await fillNativeText(page, panel.getByTestId('parameter-title'), 'Panel matrix approval')
        await dismissReferencePicker(page)
        await choose(page, panel.getByTestId('parameter-candidateUserId'), new RegExp(userName))
        await panel.getByTestId('parameter-buttons').getByRole('button', { name: /添加按钮|Add button/ }).click()
        await panel.getByTestId('parameter-buttons').getByRole('textbox', { name: /按钮文本|Button label/ }).last().fill('升级处理')
        await expect(node.locator('.studio-branch-row').filter({ hasText: '升级处理' })).toHaveCount(1)
        break
      case 'code':
        await choose(page, panel.getByTestId('parameter-runner'), /^JavaScript$/)
        await fillMonaco(page, panel.getByTestId('parameter-source'), 'async function main(inputs) { return inputs; }')
        await expect(panel.getByTestId('code-output-example')).toContainText(/完整返回对象|complete returned object/)
        await fillMonaco(page, panel.getByTestId('code-output-example'), '{ result: "ok" }')
        await choose(page, panel.getByTestId('resource-selector-sandbox_profile'), new RegExp(studioSandboxName))
        await expect(node.locator('.studio-card-body')).toContainText('JavaScript')
        break
      case 'set':
        await configureSetResult(details)
        await expect(node.locator('.studio-card-body')).toContainText(/设置 1 个字段|Set 1 field/)
        break
      case 'list':
        await chooseDirectReference(page, panel.getByTestId('parameter-input'), /输入|Inputs/, ['items'])
        await panel.getByTestId('parameter-takeN').getByRole('spinbutton').fill('3')
        await expect(node.locator('.studio-card-body')).toContainText(/筛选 0 条.*排序 0 条|0 filters.*0 sorts/)
        await expect(node.locator('.studio-card-body')).toContainText(/取前 3 条|Take first 3/)
        break
      case 'declarative_http':
        await choose(page, panel.getByTestId('parameter-method'), /^POST$/)
        await fillNativeText(page, panel.getByTestId('parameter-url'), `${echoBaseUrl}/v1/plan5/items`)
        await expect(node.locator('.studio-card-body')).toContainText(/POST.*plan5\/items/)
        break
      case 'sub_workflow':
        await choose(page, panel.getByTestId('parameter-workflowVersionId'), new RegExp(childName))
        await expect(node.locator('.studio-card-body')).toContainText(/版本|Version/)
        break
    }
    await dismissReferencePicker(page)
    await details.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
  }
  await setTheme(page, 'light')
  await setLocale(page, 'zh-CN')
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.getByRole('button', { name: /^(自动布局|Auto layout)$/i }).click({ force: true })
  await page.waitForTimeout(750)
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  await page.waitForTimeout(750)
  await expect(page).toHaveScreenshot('all-node-panels-light-zh.png', {
    animations: 'disabled',
    fullPage: true,
    mask: [page.locator('header h1'), page.locator('.studio-card-row')],
    maxDiffPixelRatio: 0.002,
  })
  await setTheme(page, 'dark')
  await setLocale(page, 'en-US')
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  await page.waitForTimeout(750)
  await expect(page).toHaveScreenshot('all-node-panels-dark-en.png', {
    animations: 'disabled',
    fullPage: true,
    mask: [page.locator('header h1'), page.locator('.studio-card-row')],
    maxDiffPixelRatio: 0.002,
  })
  await setTheme(page, 'light')
  await setLocale(page, 'zh-CN')
  await page.setViewportSize({ width: 1440, height: 900 })

  const configuredDraft = await saveAndReadDraft(page, token, workflowId)
  const configuredLoop = configuredDraft.definition.nodes.find((item) => item.type === 'loop_over_items')!
  const loopChildren = configuredDraft.definition.nodes.filter((item) => item.parentId === configuredLoop.id)
  expect(loopChildren).toHaveLength(2)
  expect(configuredDraft.definition.connections.some((edge) => edge.sourceNodeId === loopChildren[0].id && edge.targetNodeId === loopChildren[1].id)).toBeTruthy()
  expect(new Set(configuredDraft.definition.nodes.map((node) => node.type))).toEqual(new Set([
    'model', 'agent', 'if', 'loop_over_items', 'merge', 'approval', 'code', 'set', 'list', 'declarative_http', 'sub_workflow', 'exit',
  ]))
  await page.reload()
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  for (const [nodeType, panelId] of panels) {
    const definition = configuredDraft.definition.nodes.find((node) => node.type === nodeType)!
    const details = await openNodeDetails(page, page.locator(`.react-flow__node[data-id="${definition.id}"]`))
    await expect(details.getByTestId(panelId)).toBeVisible()
    await details.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
  }

  // Clear a persisted node name through the visible Basics section so the
  // same isolated Workflow also captures a deterministic field-level error.
  const modelDefinition = configuredDraft.definition.nodes.find((node) => node.type === 'model')!
  const invalidModel = page.locator(`.react-flow__node[data-id="${modelDefinition.id}"]`)
  const invalidModelDetails = await openNodeDetails(page, invalidModel)
  await openNodeBasics(invalidModelDetails)
  await invalidModelDetails.locator('[data-field-path="name"] input').fill('')
  await invalidModelDetails.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
  const saveInvalid = page.locator('header').getByRole('button', { name: '保存', exact: true })
  await expect(saveInvalid).toBeEnabled()
  await saveInvalid.click()
  const validation = page.getByRole('dialog', { name: '校验失败' })
  await expect(validation).toBeVisible()
  await page.screenshot({ path: testInfo.outputPath('all-node-panels-error-state.png'), fullPage: true })
  await validation.getByRole('button', { name: '关闭' }).click()
  const persistedAfterError = await api<StudioDraft>(page, token, `/workflows/${workflowId}/draft`)
  expect(persistedAfterError.revision).toBe(configuredDraft.revision)
  const modelErrorDetails = await openNodeDetails(page, invalidModel)
  await openNodeBasics(modelErrorDetails)
  await expect(modelErrorDetails.locator('[data-field-path="name"] .text-danger').filter({ hasText: /节点名称|Node name/ })).toBeVisible()
  await modelErrorDetails.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
})

test('M6 Studio makes dual-Agent output selection explicit across serial, parallel and Merge topologies', async ({ page }, testInfo) => {
  await useRuntimePortForward(page)
  const { token } = await login(page)
  await ensureStudioResources(page, token)
  const workflowName = `M6 Multi Agent ${Date.now()}`
  const workflowId = await createWorkflow(page, workflowName)
  await grantResource(page, 'credential', studioCredentialName, undefined, workflowName)
  await grantResource(page, 'model', studioModelName, undefined, workflowName)

  await page.goto(`/workflows/${workflowId}/editor`)
  await setStartInputs(page)
  await addFromCreator(page, 'palette-action-agent')
  await addFromCreator(page, 'palette-action-agent')

  const agents = () => page.locator('.react-flow__node').filter({ hasText: /智能体|Agent/ })
  const firstAgent = agents().first()
  const secondAgent = agents().nth(1)
  const firstDetails = await openNodeDetails(page, firstAgent)
  await openNodeBasics(firstDetails)
  await firstDetails.getByLabel('名称').fill('Agent A')
  await choose(page, firstDetails.getByTestId('agent-inspector-model'), new RegExp(studioModelName))
  await chooseAgentSessionPolicy(page, firstDetails)
  await fillNativeText(page, firstDetails.getByTestId('parameter-systemPrompt'), 'Return the serial Agent A result.')
  const secondDetails = await openNodeDetails(page, secondAgent)
  await openNodeBasics(secondDetails)
  await secondDetails.getByLabel('名称').fill('Agent B')
  await choose(page, secondDetails.getByTestId('agent-inspector-model'), new RegExp(studioModelName))
  await chooseAgentSessionPolicy(page, secondDetails)
  await fillNativeText(page, secondDetails.getByTestId('parameter-systemPrompt'), 'Return the selected Agent B result.')
  await secondDetails.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()

  await connect(page, page.getByTestId('workflow-start'), 'main', firstAgent, 'main')
  await connect(page, firstAgent, 'main', secondAgent, 'main')
  await connect(page, secondAgent, 'main', page.getByTestId('exit-node-exit'), 'main')
  const draftBeforeEnd = await saveAndReadDraft(page, token, workflowId)
  const secondAgentKey = draftBeforeEnd.definition.nodes.find((node) => node.name === 'Agent B')!.key
  await setEndOutput(page, secondAgentKey, 'json.text')
  const serialDraft = await saveAndReadDraft(page, token, workflowId)
  expect(serialDraft.definition.nodes.filter((node) => node.type === 'agent')).toHaveLength(2)
  const serialExecution = await startDebug(page, () => studioRun(page).click())
  await waitExecution(page, token, serialExecution, ['succeeded'], 180_000)

  const firstAgentId = serialDraft.definition.nodes.find((node) => node.name === 'Agent A')!.id
  const secondAgentId = serialDraft.definition.nodes.find((node) => node.name === 'Agent B')!.id
  const serialConnection = serialDraft.definition.connections.find((connection) => connection.sourceNodeId === firstAgentId && connection.targetNodeId === secondAgentId)!
  const serialEdge = page.locator(`.react-flow__edge[data-testid="rf__edge-${serialConnection.id}"]`)
  await expect(serialEdge).toHaveCount(1)
  const serialToolbar = page.locator(`.studio-edge-toolbar[data-edge-id="${serialConnection.id}"]`)
  await showEdgeToolbar(serialEdge, serialToolbar)
  const deleteConnection = serialToolbar.getByRole('button', { name: /删除连线|Delete connection/ })
  await expect(deleteConnection).toBeVisible()
  await deleteConnection.dispatchEvent('click')
  await expect(serialEdge).toHaveCount(0)
  await expect(page.locator('.react-flow__edge:not([data-testid*="__start__"]):not([data-testid*="__end__"])')).toHaveCount(2)
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  await connect(page, page.getByTestId('workflow-start'), 'main', secondAgent, 'main')
  await connect(page, firstAgent, 'main', page.getByTestId('exit-node-exit'), 'main')
  const parallelDraft = await saveAndReadDraft(page, token, workflowId)
  expect(parallelDraft.definition.connections).toEqual(expect.arrayContaining([
    expect.objectContaining({ sourceNodeId: '__start__', targetNodeId: firstAgentId }),
    expect.objectContaining({ sourceNodeId: '__start__', targetNodeId: secondAgentId }),
  ]))

  const exitId = parallelDraft.definition.nodes.find((node) => node.type === 'exit')!.id
  for (const connection of parallelDraft.definition.connections.filter((edge) => edge.targetNodeId === exitId && [firstAgentId, secondAgentId].includes(edge.sourceNodeId))) {
    const edge = page.locator(`.react-flow__edge[data-testid="rf__edge-${connection.id}"]`)
    const toolbar = page.locator(`.studio-edge-toolbar[data-edge-id="${connection.id}"]`)
    await showEdgeToolbar(edge, toolbar)
    await toolbar.getByRole('button', { name: /删除连线|Delete connection/ }).dispatchEvent('click')
    await expect(edge).toHaveCount(0)
  }
  await addFromCreator(page, 'palette-action-merge')
  const merge = flowNode(page, 'merge')
  await page.getByTestId('node-details-view').getByRole('button', { name: /关闭|Close/ }).first().click()
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/i }).click({ force: true })
  await connect(page, firstAgent, 'main', merge, 'main')
  await connect(page, secondAgent, 'main', merge, 'main')
  await connect(page, merge, 'main', page.getByTestId('exit-node-exit'), 'main')
  const mergedDraft = await saveAndReadDraft(page, token, workflowId)
  expect(mergedDraft.definition.nodes.some((node) => node.type === 'merge')).toBeTruthy()
  const mergedExitParameters = mergedDraft.definition.nodes.find((node) => node.type === 'exit')?.parameters as { outputs: Record<string, unknown> }
  expect(mergedExitParameters.outputs.answer).toMatchObject({ kind: 'reference', selector: { namespace: 'outputs', port: 'main', path: ['text'] } })
  const mergedVersion = await mutate<WorkflowVersion>(page, token, `/workflows/${workflowId}/versions`, 'POST', { draftRevision: mergedDraft.revision })
  const application = await mutate<Application>(page, token, '/applications', 'POST', { workflowId, name: workflowName, slug: `m6-multi-agent-${Date.now()}`, description: 'Dual Agent output contract E2E', visibility: 'company' })
  const environments = await api<Array<{ id: string; code: string }>>(page, token, '/environments')
  const environment = environments.find((item) => item.code === 'development')
  expect(environment).toBeTruthy()
  await mutate<{ id: string }>(page, token, `/workflows/${workflowId}/deployments`, 'POST', { workflowVersionId: mergedVersion.id, environmentId: environment!.id })
  const deployment = await mutate<ApplicationDeployment>(page, token, `/applications/${application.id}/deployments`, 'POST', { workflowVersionId: mergedVersion.id, environmentId: environment!.id, sessionVersionPolicy: 'pinned' })
  expect(deployment.id).toBeTruthy()
  await waitApplicationDeployment(page, token, application.id, deployment.id)
  await publishCompatibleChatMapping(page, token, application.id, deployment.id)

  await page.goto('/playground')
  await page.getByRole('combobox').click()
  await page.getByRole('option', { name: workflowName, exact: true }).click()
  await page.getByRole('tab', { name: '对话测试' }).click()
  const sessionResponse = page.waitForResponse((value) => value.url().includes('/gateway/v1/applications/') && value.url().endsWith('/sessions') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '新建会话' }).click()
  const session = await (await sessionResponse).json() as { id: string }
  const invocationResponse = page.waitForResponse((value) => value.url().includes('/gateway/v1/sessions/') && value.url().endsWith('/messages') && value.request().method() === 'POST')
  const composer = page.getByPlaceholder('输入消息进行测试…')
  await expect(composer).toHaveJSProperty('tagName', 'TEXTAREA')
  await composer.fill('M6 dual Agent output')
  await page.getByRole('button', { name: '发送' }).click()
  const invocationHttpResponse = await invocationResponse
  if (!invocationHttpResponse.ok()) {
    throw new Error(`Session message failed with HTTP ${invocationHttpResponse.status()}: ${await invocationHttpResponse.text()}`)
  }
  const invocation = await invocationHttpResponse.json() as GatewayInvocation
  let completed: GatewayInvocation | undefined
  await expect.poll(async () => {
    const response = await page.request.get(`${gatewayBase}/gateway/v1/invocations/${invocation.id}`, { headers: { Authorization: `Bearer ${token}` } })
    completed = await response.json() as GatewayInvocation
    return completed.status
  }, { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe('completed')
  expect(completed?.error).toBeNull()
  const messagesResponse = await page.request.get(`${gatewayBase}/gateway/v1/sessions/${session.id}/messages`, { headers: { Authorization: `Bearer ${token}` } })
  const messages = await messagesResponse.json() as GatewayMessage[]
  const sessionTitle = page.locator('aside').getByText('M6 dual Agent output', { exact: true })
  await expect(sessionTitle).toBeVisible({ timeout: 30_000 })
  await sessionTitle.hover()
  await expect(page.getByRole('tooltip')).toHaveText('M6 dual Agent output')
  const assistant = messages.find((message) => message.role === 'assistant')
  expect(assistant).toBeTruthy()
  const nodeRuns = await api<{ items: Array<{ nodeId: string; status: string; output?: { main?: Array<{ json?: { text?: string } }> } }> }>(page, token, `/executions/${completed!.executionId}/nodes`)
  const selectedRun = nodeRuns.items.find((item) => item.nodeId === secondAgentId && item.status === 'succeeded')
  const selectedJson = selectedRun?.output?.main?.[0]?.json
  const selectedText = selectedJson?.text
  expect(selectedText).toBeTruthy()
  expect(assistant!.parts.some((part) => part.content === selectedText)).toBeTruthy()

  await page.screenshot({ path: testInfo.outputPath('dual-agent-output-semantics.png'), fullPage: true })
})
