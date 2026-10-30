import { expect, type Page } from '@playwright/test'
import { writeFile } from 'node:fs/promises'
import { create, login, publishVersion, read, saved, test, type Created } from '../studio-builder-support'
import { connectStudioPorts } from './studio-connections'
import { addCanvasNode, assertRoutesAvoidCards, closeDetails, moveCard, waitForCanvasSettled } from './workflow-routing-helpers'
import { showEdgeToolbar } from './workflow-studio-editor-helpers'

type Layout = { nodeId: string; x: number; y: number }
type Document = { revision: number; definition: unknown; editorDocument: { nodeLayouts: Layout[]; [key: string]: unknown } }
const documentOf = (page: Page, token: string, created: Created) => read<Document>(page, token, `/workflows/${created.id}/drafts/${created.documentId}`)
const undo = (page: Page) => page.getByRole('button', { name: /^(撤销|Undo)$/ }).click()
const redo = (page: Page) => page.getByRole('button', { name: /^(重做|Redo)$/ }).click()

test('Canvas error exits keep each branch local and preserve dedicated error-only Ends after refresh', async ({ page }, info) => {
  const token = await login(page)
  const created = await create(page, `Local error exits ${Date.now()}`)
  await showEdgeToolbar(page.locator('.react-flow__edge').first(), page.locator('.studio-edge-toolbar[data-edge-id="start-exit"]'))
  await page.locator('.studio-edge-toolbar[data-edge-id="start-exit"]').getByRole('button', { name: /删除连线|Delete connection/ }).click()
  const a = await addCanvasNode(page, 'set'), b = await addCanvasNode(page, 'set'), gate = await addCanvasNode(page, 'set')
  const endA = page.locator('.react-flow__node-exit').first()
  const addExit = async () => {
    const before = new Set(await page.locator('.react-flow__node-exit').evaluateAll(nodes => nodes.map(node => node.getAttribute('data-id'))))
    await page.getByTestId('node-creator-shell').getByRole('button', { name: '结束节点', exact: true }).click()
    await closeDetails(page)
    const id = await page.locator('.react-flow__node-exit').evaluateAll((nodes, ids) => nodes.map(node => node.getAttribute('data-id')!).find(id => !ids.includes(id)), [...before])
    return page.locator(`.react-flow__node-exit[data-id="${id}"]`)
  }
  const endB = await addExit(), errorEnd = await addExit()
  await page.getByRole('button', { name: '收起节点面板' }).click()
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/ }).click()
  await waitForCanvasSettled(page)
  const canvas = (await page.getByTestId('workflow-canvas').boundingBox())!
  await moveCard(page, page.getByTestId('workflow-start'), canvas.x + canvas.width / 2, canvas.y + 630)
  for (const [index, [source, target]] of [[a, endA], [b, endB], [gate, errorEnd]].entries()) {
    await moveCard(page, source, canvas.x + 100, canvas.y + 40 + index * 200)
    await moveCard(page, target, canvas.x + canvas.width - (await target.boundingBox())!.width - 100, canvas.y + 40 + index * 200)
    if (index < 2) await connectStudioPorts(page, source, 'main', target, 'main')
    await connectStudioPorts(page, source, 'error', target, 'error')
  }
  const checkLocal = async () => {
    await expect(page.locator('[data-route-id]')).toHaveCount(5)
    await assertRoutesAvoidCards(page)
    const routes = await page.locator('[data-route-id]').evaluateAll(elements => elements.map(element => ({ id: element.getAttribute('data-route-id'), points: JSON.parse(element.getAttribute('data-route-points')!) as Array<{ x: number; y: number }> })))
    expect(routes).toHaveLength(5)
    for (const route of routes) expect(Math.max(...route.points.map(point => point.y)) - Math.min(...route.points.map(point => point.y)), JSON.stringify(route)).toBeLessThan(8)
  }
  await checkLocal()
  const document = await saved(page, token, created)
  await writeFile(info.outputPath('branch-local-error-exits.json'), JSON.stringify(document, null, 2))
  await page.reload()
  await checkLocal()
  await page.screenshot({ path: info.outputPath('branch-local-error-exits.png') })
})

test('Canvas routing keeps card positions stable, avoids moved third-party obstacles and preserves group port kinds', async ({ page }, info) => {
  const token = await login(page)
  const created = await create(page, `Canvas routing ${Date.now()}`)
  const initial = page.locator('.react-flow__edge').first()
  await showEdgeToolbar(initial, page.locator('.studio-edge-toolbar[data-edge-id="start-exit"]'))
  await page.locator('.studio-edge-toolbar[data-edge-id="start-exit"]').getByRole('button', { name: /删除连线|Delete connection/ }).click()
  const a = await addCanvasNode(page, 'set'), b = await addCanvasNode(page, 'set'), third = await addCanvasNode(page, 'set')
  await page.getByRole('button', { name: '收起节点面板' }).click()
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/ }).click()
  await waitForCanvasSettled(page)
  const canvas = await page.getByTestId('workflow-canvas').boundingBox()
  if (!canvas) throw new Error('Canvas missing')
  const start = page.getByTestId('workflow-start'), exit = page.getByTestId('exit-node-exit')
  await moveCard(page, start, canvas.x + 40, canvas.y + 36)
  await moveCard(page, exit, canvas.x + canvas.width - (await exit.boundingBox())!.width - 40, canvas.y + 36)
  await moveCard(page, a, canvas.x + 40, canvas.y + 220)
  await moveCard(page, b, canvas.x + canvas.width - (await b.boundingBox())!.width - 40, canvas.y + 220)
  await moveCard(page, third, canvas.x + canvas.width / 2 - (await third.boundingBox())!.width / 2, canvas.y + 420)
  await connectStudioPorts(page, a, 'main', b, 'main')
  await assertRoutesAvoidCards(page)
  await saved(page, token, created)
  const before = await documentOf(page, token, created)
  const thirdId = await third.getAttribute('data-id')
  const initialPath = await page.locator('[data-route-id]').first().getAttribute('data-route-points')
  const thirdBox = (await third.boundingBox())!
  await moveCard(page, third, thirdBox.x, (await a.boundingBox())!.y)
  await assertRoutesAvoidCards(page)
  expect(await page.locator('[data-route-id]').first().getAttribute('data-route-points')).not.toBe(initialPath)
  await saved(page, token, created)
  const moved = await documentOf(page, token, created)
  expect(moved.editorDocument.nodeLayouts.filter(node => node.nodeId !== thirdId)).toEqual(before.editorDocument.nodeLayouts.filter(node => node.nodeId !== thirdId))
  await undo(page)
  await assertRoutesAvoidCards(page)
  await redo(page)
  await assertRoutesAvoidCards(page)
  await connectStudioPorts(page, a, 'error', exit, 'error')
  await assertRoutesAvoidCards(page)
  await a.locator('.studio-card-head').click()
  await page.keyboard.down('Shift')
  await third.locator('.studio-card-head').click()
  await page.keyboard.up('Shift')
  await closeDetails(page)
  await page.getByTestId('node-creator-shell').getByRole('button', { name: '分组', exact: true }).click()
  const group = page.locator('[data-testid^="studio-group-"]').first()
  await group.getByRole('button', { name: '折叠分组' }).click()
  await expect(group.locator('.react-flow__handle')).toHaveCount(4)
  await expect(group.locator('[data-handleid="group-error-out"]')).toHaveCSS('top', '52px')
  const offsets = await group.evaluate(element => {
    const box = element.getBoundingClientRect()
    const input = element.querySelector('[data-handleid="group-in"]')!.getBoundingClientRect()
    const output = element.querySelector('[data-handleid="group-out"]')!.getBoundingClientRect()
    return [Math.abs(input.x + input.width / 2 - box.left), Math.abs(output.x + output.width / 2 - box.right)]
  })
  expect(Math.max(...offsets)).toBeLessThan(1)
  await assertRoutesAvoidCards(page)
  await group.getByRole('button', { name: '展开分组' }).click()
  await assertRoutesAvoidCards(page)
  await page.getByRole('button', { name: /^(自动布局|Auto layout)$/ }).click()
  await saved(page, token, created)
  const arranged = await documentOf(page, token, created)
  await assertRoutesAvoidCards(page)
  await undo(page)
  await saved(page, token, created)
  const unarranged = await documentOf(page, token, created)
  expect(unarranged.editorDocument.nodeLayouts).not.toEqual(arranged.editorDocument.nodeLayouts)
  await redo(page)
  await saved(page, token, created)
  expect((await documentOf(page, token, created)).editorDocument.nodeLayouts).toEqual(arranged.editorDocument.nodeLayouts)
  await page.reload()
  await assertRoutesAvoidCards(page)
  await expect(page.getByRole('button', { name: '撤销', exact: true })).toBeDisabled()
  const refreshed = await documentOf(page, token, created)
  expect(refreshed.revision).toBe(arranged.revision + 2)
  expect(JSON.stringify(refreshed)).not.toMatch(/CanvasRoute|route-points|svgPathString|avoidAreas/)
  await writeFile(info.outputPath('canvas-document.json'), JSON.stringify(refreshed, null, 2))
  await page.screenshot({ path: info.outputPath('routing-after-refresh.png') })
})

test('Canvas cards share geometry across registered nodes in Chinese and English, light and dark', async ({ page }, info) => {
  const token = await login(page)
  const childName = 'Canvas matrix child'
  const child = await create(page, childName)
  await saved(page, token, child)
  await publishVersion(page, token, child, child.createdVersionId)
  const created = await create(page, `Canvas card matrix ${Date.now()}`)
  for (const type of ['agent', 'approval', 'await_input', 'code', 'declarative_http', 'if', 'knowledge_retrieval', 'list', 'loop_over_items', 'merge', 'model', 'reply_message', 'send_message', 'set', 'sub_workflow']) {
    await addCanvasNode(page, type, async details => {
      if (type === 'agent') {
        await details.locator('[data-field-path="parameters.sessionPolicy"]').getByRole('combobox').click()
        await page.getByRole('option', { name: /仅本次调用|Invocation only/ }).click()
      }
      if (type === 'sub_workflow') {
        await details.getByTestId('parameter-workflowVersionId').getByRole('combobox').click()
        await page.getByRole('option', { name: new RegExp(childName) }).click()
      }
    })
  }
  await page.getByRole('button', { name: '收起节点面板' }).click()
  await page.getByRole('button', { name: /^(自动布局|Auto layout)$/ }).click()
  await saved(page, token, created)
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/ }).click()
  const baseline = await page.locator('.react-flow__node').evaluateAll(nodes => nodes.map(node => ({ id: node.getAttribute('data-id'), width: (node as HTMLElement).style.width, height: (node as HTMLElement).style.height, ports: [...node.querySelectorAll('.react-flow__handle')].map(port => ({ id: port.getAttribute('data-handleid'), top: (port as HTMLElement).style.top })) })))
  for (const locale of ['zh-CN', 'en-US'] as const) for (const theme of ['light', 'dark'] as const) {
    const menu = () => page.locator('header').getByRole('button', { name: /更多.*操作|More.*actions/i })
    if (await page.locator('html').getAttribute('lang') !== locale) {
      await menu().click()
      await page.getByRole('menuitemradio', { name: locale === 'zh-CN' ? '简体中文' : 'English' }).click()
    }
    await menu().click()
    await page.getByRole('menuitemradio', { name: theme === 'light' ? /^(浅色|Light)$/ : /^(深色|Dark)$/ }).click()
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
    await assertRoutesAvoidCards(page)
    const covered = await page.locator('.react-flow__minimap').evaluate(map => {
      const mini = map.getBoundingClientRect()
      return [...document.querySelectorAll('.studio-card')].filter(card => {
        const box = card.getBoundingClientRect()
        return box.right > mini.left && box.left < mini.right && box.bottom > mini.top && box.top < mini.bottom
      }).map(card => card.closest('.react-flow__node')?.getAttribute('data-id'))
    })
    expect(covered, 'Fit view must reserve space for the MiniMap').toEqual([])
    const current = await page.locator('.react-flow__node').evaluateAll(nodes => nodes.map(node => ({ id: node.getAttribute('data-id'), width: (node as HTMLElement).style.width, height: (node as HTMLElement).style.height, ports: [...node.querySelectorAll('.react-flow__handle')].map(port => ({ id: port.getAttribute('data-handleid'), top: (port as HTMLElement).style.top })) })))
    expect(current).toEqual(baseline)
    await expect(page.getByTestId('workflow-canvas')).toHaveScreenshot(`canvas-cards-${locale}-${theme}.png`, { animations: 'disabled', maxDiffPixelRatio: .002 })
  }
  await writeFile(info.outputPath('card-geometries.json'), JSON.stringify(baseline, null, 2))
})
