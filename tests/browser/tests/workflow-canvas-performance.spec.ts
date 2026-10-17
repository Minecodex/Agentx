import { fixtureVersionId, fixtureDraftId } from '../draft-fixtures'
import { expect, type APIResponse, type Page, test } from '@playwright/test'
import { writeFile } from 'node:fs/promises'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { deserializeDraft, serializeStudio } from '../../../src/web/src/features/workflow-designer/model/serializer'

// Recording video and DOM/screenshot traces adds work to every input frame.
// Functional suites keep recording; this benchmark exports its raw metrics.
test.use({ trace: 'off', video: 'off' })

const password = 'agentx-e2e-admin-password'
type Draft = { revision: number; updatedAt: string; [key: string]: unknown }
type Metric = { nodeCount: number; firstInteractiveMs: number; fixtureContractPreparationMs?: number; contractReadyMs?: number; medianFps: number; p95InputDelayMs: number; renderedNodes: number }
type CanvasSamples = { frames: number[]; inputDelays: number[]; inputEvents: Array<{ kind: string; at: number; delay: number }>; longTasks: Array<{ at: number; duration: number }>; started: number; lastFrame?: number }
type CatalogPage = { items: Array<{ nodeType: string; version: number; [key: string]: unknown }> }

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(password)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

async function request<T>(page: Page, token: string, path: string, method = 'GET', body?: unknown, timeout = 30_000): Promise<T> {
  const response = await page.request.fetch(`/api/v1${path}`, { method, data: body, timeout, headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })
  await expectResponse(response, path)
  return response.json() as Promise<T>
}

async function expectResponse(response: APIResponse, label: string) {
  if (!response.ok()) throw new Error(`${label}: ${response.status()} ${await response.text()}`)
}

async function createWorkflow(page: Page, token: string, nodeCount: number) {
  return request<{ id: string }>(page, token, '/workflows', 'POST', { name: `M6 Canvas ${nodeCount} ${Date.now()}`, description: 'Synthetic canvas performance benchmark', visibility: 'company' })
}

function syntheticDraft(base: Draft, nodeCount: number, nodeType: string, typeVersion: number, parameters: Record<string, unknown>) {
  const nodes = Array.from({ length: nodeCount }, (_, index) => ({
    id: `perf-${index}`, type: nodeType, typeVersion, name: `Pass Through ${index}`, disabled: false,
    parameters, resourceReferences: [], settings: {},
  }))
  const connections = nodes.slice(0, -1).map((value, index) => ({
    id: `perf-edge-${index}`, sourceNodeId: value.id, sourceHandle: 'main', targetNodeId: nodes[index + 1].id, targetHandle: 'main', order: 0,
  }))
  return {
    ...base,
    updatedAt: new Date().toISOString(),
    definition: { schemaVersion: '8.0', nodes, connections, settings: { executionOrder: 'deterministic', activationBudget: 10_000 } },
    editorDocument: {
      nodeLayouts: nodes.map((value, index) => ({ nodeId: value.id, x: 80 + (index % 50) * 150, y: 80 + Math.floor(index / 50) * 150 })),
      bindingLayouts: [], edges: connections.map((value) => ({ edgeId: value.id })), bindingEdges: [], annotations: [], groups: [],
      viewport: { x: 0, y: 0, zoom: 0.7 },
    },
  }
}

async function benchmark(page: Page, token: string, nodeCount: number, nodeType: string, typeVersion: number, parameters: Record<string, unknown>): Promise<Metric> {
  const workflow = await createWorkflow(page, token, nodeCount)
  const draftId = await fixtureDraftId(page, workflow.id)
  const base = await request<Draft>(page, token, `/workflows/${workflow.id}/drafts/${draftId}`)
  const draft = syntheticDraft(base, nodeCount, nodeType, typeVersion, parameters)
  let fixtureContractPreparationMs: number | undefined
  if (nodeType === 'acme.json_mapper') {
    const preparedAt = Date.now()
    const definition = JSON.parse(JSON.stringify(serializeStudio(deserializeDraft(draft as Parameters<typeof deserializeDraft>[0])).definition))
    const contracts = await request<{ nodes: Record<string, { status: string }> }>(page, token,
      `/workflows/${workflow.id}/drafts/${draftId}/resolve-plugins`, 'POST', { definition }, 60_000)
    expect(Object.keys(contracts.nodes)).toHaveLength(nodeCount)
    expect(Object.values(contracts.nodes).every(value => value.status === 'complete')).toBeTruthy()
    fixtureContractPreparationMs = Date.now() - preparedAt
    // Draft/catalog are already fixed fixtures. Feed the same actual SDK
    // result to the current batch endpoint, rather than timing cold server
    // compilation during the UI benchmark or fabricating a contract.
    await page.route(`**/api/v1/workflows/${workflow.id}/drafts/${draftId}/resolve-plugins`, async route => {
      expect(route.request().postDataJSON().definition).toEqual(definition)
      await route.fulfill({ json: contracts })
    })
  }
  await page.route(`**/api/v1/workflows/${workflow.id}/drafts/${draftId}`, async (route) => {
    if (route.request().method() === 'GET') await route.fulfill({ json: draft })
    else await route.fulfill({ json: { ...draft, revision: base.revision + 1, updatedAt: new Date().toISOString() } })
  })

  const started = Date.now()
  const contractReady = nodeType === 'acme.json_mapper'
    ? page.waitForResponse(response => response.request().method() === 'POST' && response.url().endsWith(`/workflows/${workflow.id}/drafts/${draftId}/resolve-plugins`))
    : undefined
  await page.goto(`/workflows/${workflow.id}/versions/${await fixtureVersionId(page, workflow.id)}/editor`)
  const canvas = page.getByTestId('workflow-canvas')
  await expect(canvas).toBeVisible()
  const firstNode = page.getByTestId('studio-node-perf-0')
  await expect(firstNode).toBeVisible()
  const firstInteractiveMs = Date.now() - started
  expect(firstInteractiveMs).toBeLessThanOrEqual(3_000)
  let contractReadyMs: number | undefined
  if (contractReady) {
    const response = await contractReady
    expect(response.ok(), await response.text()).toBeTruthy()
    const contracts = await response.json() as { nodes: Record<string, { status: string }> }
    expect(Object.keys(contracts.nodes)).toHaveLength(nodeCount)
    expect(Object.values(contracts.nodes).every(value => value.status === 'complete')).toBeTruthy()
    await expect(firstNode).toContainText('Label: performance')
    contractReadyMs = Date.now() - started
  }
  await page.evaluate(() => document.fonts.ready)
  if (nodeType === 'acme.json_mapper' && nodeCount === 100) {
    for (let index = 0; index < 5; index += 1) {
      await firstNode.click()
      await expect(page.getByTestId('node-details-view')).toBeVisible()
      await page.getByTestId('node-details-view').getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
    }
    // Design-only canvases render schema controls and never load plugin UI/CSS.
    await expect(page.locator('style[data-agentx-plugin]')).toHaveCount(0)
  }
  await expect(page.locator('.react-flow__minimap')).toHaveCount(nodeCount >= 300 ? 0 : 1)
  await page.waitForTimeout(750)

  const profiler = process.env.AGENTX_E2E_CANVAS_PROFILE === '1' && nodeCount === 300
    ? await page.context().newCDPSession(page) : undefined
  if (profiler) {
    await profiler.send('Profiler.enable')
    await profiler.send('Profiler.start')
  }

  await page.evaluate(() => {
    const target = window as Window & { __agentxPerf?: CanvasSamples }
    const metrics: CanvasSamples = { frames: [], inputDelays: [], inputEvents: [], longTasks: [], started: performance.now() }
    target.__agentxPerf = metrics
    const observer = new PerformanceObserver((entries) => entries.getEntries().forEach((entry) => {
      metrics.longTasks.push({ at: entry.startTime - metrics.started, duration: entry.duration })
    }))
    observer.observe({ type: 'longtask' })
    const input = (event: Event) => {
      const eventAt = performance.now()
      requestAnimationFrame(() => {
        const delay = performance.now() - eventAt
        metrics.inputDelays.push(delay)
        metrics.inputEvents.push({ kind: event.type, at: eventAt - metrics.started, delay })
      })
    }
    document.addEventListener('pointermove', input, { passive: true })
    document.addEventListener('wheel', input, { passive: true })
    const frame = (now: number) => {
      if (metrics.lastFrame !== undefined) metrics.frames.push(now - metrics.lastFrame)
      metrics.lastFrame = now
      if (now - metrics.started < 1_800) requestAnimationFrame(frame)
      else {
        document.removeEventListener('pointermove', input)
        document.removeEventListener('wheel', input)
        observer.disconnect()
      }
    }
    requestAnimationFrame(frame)
  })

  const box = await firstNode.boundingBox()
  expect(box).toBeTruthy()
  await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2)
  await page.mouse.down()
  for (let index = 0; index < 50; index += 1) {
    await page.mouse.move(box!.x + box!.width / 2 + index * 2, box!.y + box!.height / 2 + Math.sin(index / 4) * 20)
    await page.waitForTimeout(8)
  }
  await page.mouse.up()
  const canvasBox = await canvas.boundingBox()
  expect(canvasBox).toBeTruthy()
  await page.mouse.move(canvasBox!.x + canvasBox!.width / 2, canvasBox!.y + canvasBox!.height / 2)
  for (let index = 0; index < 12; index += 1) await page.mouse.wheel(0, index % 2 === 0 ? -80 : 80)
  await page.waitForTimeout(1_900)

  const samples = await page.evaluate(() => (window as Window & { __agentxPerf?: CanvasSamples }).__agentxPerf!)
  if (profiler) {
    const { profile } = await profiler.send('Profiler.stop')
    const profilePath = test.info().outputPath(`canvas-${nodeCount}.cpuprofile`)
    await writeFile(profilePath, JSON.stringify(profile), 'utf8')
    await test.info().attach(`canvas-${nodeCount}.cpuprofile`, { path: profilePath, contentType: 'application/json' })
    await profiler.detach()
  }
  const medianFrame = percentile(samples.frames, 0.5)
  const metric = {
    nodeCount,
    firstInteractiveMs,
    fixtureContractPreparationMs,
    contractReadyMs,
    medianFps: Number((1_000 / medianFrame).toFixed(1)),
    p95InputDelayMs: Number(percentile(samples.inputDelays, 0.95).toFixed(1)),
    renderedNodes: await page.locator('.react-flow__node-manifest').count(),
  }
  const metricPath = test.info().outputPath(`canvas-metric-${nodeCount}.json`)
  await writeFile(metricPath, JSON.stringify({ ...metric, samples }, null, 2), 'utf8')
  await test.info().attach(`canvas-metric-${nodeCount}.json`, { path: metricPath, contentType: 'application/json' })
  if (nodeCount >= 300) expect(metric.renderedNodes).toBeLessThan(nodeCount)
  else expect(metric.renderedNodes).toBe(nodeCount)
  expect(metric.medianFps).toBeGreaterThanOrEqual(nodeCount <= 500 ? 50 : 30)
  expect(metric.p95InputDelayMs).toBeLessThanOrEqual(nodeCount <= 500 ? 50 : 100)
  await page.unroute(`**/api/v1/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`)
  return metric
}

function percentile(values: number[], ratio: number) {
  expect(values.length).toBeGreaterThan(0)
  const sorted = [...values].sort((left, right) => left - right)
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(sorted.length * ratio) - 1))]
}

test('Workflow canvas meets the 100, 300, 500, and 1000 plugin-node interaction budgets', async ({ page }, testInfo) => {
  test.slow()
  const token = await login(page)
  let catalog = await request<CatalogPage>(page, token, '/node-definitions?pageSize=100')
  if (!catalog.items.some((item) => item.nodeType === 'acme.json_mapper')) {
    const bytes = await readFile(resolve(import.meta.dirname, '../../../src/plugins/templates/canvas-plugin/dist/acme-json-mapper.agentx-plugin'))
    const imported = await page.request.post('/api/v1/canvas-plugin-imports', { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() }, multipart: { file: { name: 'performance.agentx-plugin', mimeType: 'application/zip', buffer: bytes } } })
    const preview = await imported.json() as { id: string; bundleDigest: string }
    const installed = await page.request.post(`/api/v1/canvas-plugin-imports/${preview.id}/install`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() }, data: { bundleDigest: preview.bundleDigest, enable: true, setDefault: true } })
    expect(installed.ok(), await installed.text()).toBeTruthy()
    catalog = await request<CatalogPage>(page, token, '/node-definitions?pageSize=100')
  }
  const setNode = catalog.items.find((item) => item.nodeType === 'set' && item.version === 1)
  const pluginNode = catalog.items.find((item) => item.nodeType === 'acme.json_mapper' && item.version === 1)
  expect(setNode).toBeTruthy()
  expect(pluginNode).toBeTruthy()
  const setDetail = await request<Record<string, unknown>>(page, token, '/node-definitions/set/versions/1')
  const pluginDetail = await request<Record<string, unknown>>(page, token, '/node-definitions/acme.json_mapper/versions/1')
  await page.route('**/api/v1/node-definitions?pageSize=100', (route) => route.fulfill({ json: { items: [setNode, pluginNode] } }))
  await page.route('**/api/v1/node-definitions/set/versions/1', (route) => route.fulfill({ json: setDetail }))
  await page.route('**/api/v1/node-definitions/acme.json_mapper/versions/1', (route) => route.fulfill({ json: pluginDetail }))
  await page.route('**/api/v1/node-definitions/acme.json_mapper/versions/1/resolve', (route) => route.fulfill({ json: { status: 'complete', inputPorts: [{ name: 'main', kind: 'main', required: true, variadic: false }], outputPorts: [{ name: 'main', kind: 'main', required: false, variadic: false }, { name: 'error', kind: 'error', required: false, variadic: false }], outputSchema: { type: 'object', additionalProperties: true }, outputPortSchemas: {}, issues: [] } }))
  const metrics = [
    await benchmark(page, token, 100, 'acme.json_mapper', 1, { label: 'performance', includeMetadata: false }),
    await benchmark(page, token, 300, 'acme.json_mapper', 1, { label: 'performance', includeMetadata: false }),
    await benchmark(page, token, 500, 'set', 1, {}),
    await benchmark(page, token, 1_000, 'set', 1, {}),
  ]
  await expect(page.locator('style[data-agentx-plugin]')).toHaveCount(0)
  const evidence = JSON.stringify(metrics, null, 2)
  const evidencePath = testInfo.outputPath('workflow-canvas-performance.json')
  await writeFile(evidencePath, evidence, 'utf8')
  await testInfo.attach('workflow-canvas-performance.json', { path: evidencePath, contentType: 'application/json' })
})
