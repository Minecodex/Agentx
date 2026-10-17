import { publishFixtureResponse } from '../draft-fixtures'
import { fixtureVersionId, fixtureDraftId } from '../draft-fixtures'
import { expect, test } from '@playwright/test'

import { connect, login } from '../support/canvas-plugin'
import { showEdgeToolbar } from './workflow-studio-editor-helpers'

test('propagates dynamic contracts across a plugin chain and preserves them after save and reopen', async ({ page }, testInfo) => {
  const token = await login(page)
  const headers = { Authorization: `Bearer ${token}` }
  await page.goto('/canvas-plugins')
  await page.getByRole('button', { name: '导入插件' }).click()
  await page.locator('input[type="file"]').setInputFiles(process.env.AGENTX_PLUGIN_BOUNDARY_PACKAGE!)
  const preview = page.getByRole('dialog')
  await expect(preview.getByText('acme/boundaries', { exact: false })).toBeVisible()
  await preview.getByRole('button', { name: '确认导入' }).click()
  await expect(preview).toBeHidden()
  const created = await page.request.post('/api/v1/workflows', { headers, data: { name: `Dynamic chain ${Date.now()}`, visibility: 'company' } })
  expect(created.ok(), await created.text()).toBeTruthy()
  const workflow = await created.json() as { id: string }
  const draftResponse = await page.request.get(`/api/v1/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`, { headers })
  const draft = await draftResponse.json() as { definition: { connections: Array<{ id: string }> } }
  await page.goto(`/workflows/${workflow.id}/versions/${await fixtureVersionId(page, workflow.id)}/editor`)
  const initialId = draft.definition.connections[0].id
  const initialEdge = page.locator(`.react-flow__edge[data-testid="rf__edge-${initialId}"]`)
  const toolbar = page.locator(`.studio-edge-toolbar[data-edge-id="${initialId}"]`)
  await showEdgeToolbar(initialEdge, toolbar)
  await toolbar.getByRole('button', { name: '删除连线' }).click()
  await expect(initialEdge).toHaveCount(0)
  const inspector = page.getByTestId('node-details-view')
  await page.getByRole('textbox', { name: '搜索节点' }).fill('Boundary Plugin')
  await page.getByTestId('palette-action-acme.boundaries').first().click()
  await inspector.getByLabel('Label', { exact: true }).fill('first')
  await inspector.getByRole('button', { name: '关闭', exact: true }).first().click()
  const upstream = page.locator('.react-flow__node-manifest').filter({ hasText: 'Boundary Plugin' }).nth(0)
  await expect(upstream).toBeVisible()
  await page.getByTestId('palette-action-acme.boundaries').first().click()
  await inspector.getByLabel('Label', { exact: true }).fill('follow')
  await inspector.getByRole('button', { name: '关闭', exact: true }).first().click()
  const downstream = page.locator('.react-flow__node-manifest').filter({ hasText: 'Boundary Plugin' }).nth(1)
  await page.getByRole('button', { name: '适应画布', exact: true }).click({ force: true })
  await connect(page, page.getByTestId('workflow-start'), 'main', upstream, 'main')
  await connect(page, upstream, 'main', downstream, 'main')
  await expect(downstream.locator('.react-flow__handle.source[data-handleid="first"]')).toBeVisible()
  await connect(page, downstream, 'main', page.getByTestId('exit-node-exit'), 'main')
  await upstream.click()
  await inspector.getByLabel('Label', { exact: true }).fill('second')
  await expect(downstream.locator('.react-flow__handle.source[data-handleid="second"]')).toBeVisible()
  await expect(downstream.locator('.react-flow__handle.source[data-handleid="first"]')).toHaveCount(0)
  await inspector.getByRole('button', { name: '关闭', exact: true }).first().click()
  const save = page.locator('header').getByRole('button', { name: '保存', exact: true })
  if (await save.isEnabled()) await save.click()
  await expect(page.locator('header').getByText(/修订号 \d+ · 已保存/)).toBeVisible()
  await page.reload()
  await expect(downstream.locator('.react-flow__handle.source[data-handleid="second"]')).toBeVisible()
  const savedResponse = await page.request.get(`/api/v1/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`, { headers })
  const saved = await savedResponse.json() as { revision: number }
  const version = await publishFixtureResponse(page, token, workflow.id, saved.revision)
  expect(version.ok(), await version.text()).toBeTruthy()
  await page.screenshot({ path: testInfo.outputPath('dynamic-plugin-chain.png'), fullPage: true })
})
