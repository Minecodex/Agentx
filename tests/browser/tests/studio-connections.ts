import { expect, type Locator, type Page } from '@playwright/test'

export async function connectStudioPorts(page: Page, source: Locator, sourceHandle: string, target: Locator, targetHandle: string) {
  const canvas = page.getByTestId('workflow-canvas')
  // ReactFlow can mount an already-created edge after its parent state has
  // committed. Use the canonical count, then verify the intended DOM pair.
  const edgeCount = Number(await canvas.getAttribute('data-canonical-edge-count'))
  const sourceId = await source.evaluate(element => element.closest('.react-flow__node')?.getAttribute('data-id'))
  const targetId = await target.evaluate(element => element.closest('.react-flow__node')?.getAttribute('data-id'))
  expect(sourceId).toBeTruthy()
  expect(targetId).toBeTruthy()
  const connection = page.locator(`.react-flow__edge[aria-label="Edge from ${sourceId} to ${targetId}"]`)
  const connectionCount = await connection.count()
  const from = source.locator(`.react-flow__handle.source[data-handleid="${sourceHandle}"]`)
  const to = target.locator(`.react-flow__handle.target[data-handleid="${targetHandle}"]`)
  await expect(from).toBeVisible()
  await expect(to).toBeVisible()
  for (let attempt = 0; attempt < 3 && await connection.count() === connectionCount; attempt += 1) {
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
  await expect(connection).toHaveCount(connectionCount + 1)
  await expect(canvas).toHaveAttribute('data-canonical-edge-count', String(edgeCount + 1))
}
