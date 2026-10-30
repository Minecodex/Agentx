import { expect, test, type Locator, type Page } from '@playwright/test'

export async function closeDetails(page: Page) {
  const details = page.getByTestId('node-details-view')
  if (await details.isVisible()) await details.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
}

export async function addCanvasNode(page: Page, type: string, configure?: (details: Locator) => Promise<void>) {
  const before = new Set(await page.locator('.react-flow__node').evaluateAll(nodes => nodes.map(node => node.getAttribute('data-id'))))
  if (await page.getByTestId('node-creator-trigger').isVisible()) await page.getByTestId('node-creator-trigger').click()
  await page.getByTestId('node-creator').getByRole('textbox', { name: /搜索节点|Search nodes/ }).fill(type)
  await page.getByTestId(`palette-action-${type}`).click()
  await expect.poll(async () => (await page.locator('.react-flow__node').evaluateAll(nodes => nodes.map(node => node.getAttribute('data-id')))).filter(id => !before.has(id)).length).toBeGreaterThan(0)
  const added = await page.locator('.react-flow__node').evaluateAll(nodes => nodes.map(node => ({ id: node.getAttribute('data-id')!, type: node.className })))
  const id = added.find(node => !before.has(node.id) && (type === 'loop_over_items' ? node.type.includes('loop-container') : node.type.includes('node-manifest')))!.id
  if (configure) await configure(page.getByTestId('node-details-view'))
  await closeDetails(page)
  return page.locator(`.react-flow__node[data-id="${id}"]`)
}

export async function waitForCanvasSettled(page: Page) {
  await page.locator('.react-flow__viewport').evaluate(async element => {
    let stable = 0, previous = ''
    while (stable < 12) {
      await new Promise(requestAnimationFrame)
      const state = `${element.getBoundingClientRect().x}:${element.getBoundingClientRect().y}:${getComputedStyle(element).transform}`
      stable = state === previous ? stable + 1 : 0
      previous = state
    }
  })
}

export async function moveCard(page: Page, card: Locator, left: number, top: number) {
  await waitForCanvasSettled(page)
  const box = await card.boundingBox()
  const head = await card.locator('.studio-card-head').boundingBox()
  if (!box || !head) throw new Error('Card is not measurable')
  const x = head.x + head.width / 2, y = head.y + head.height / 2
  await page.mouse.move(x, y)
  await page.mouse.down()
  // Cross React Flow's drag threshold before measuring the gesture delta.
  await page.mouse.move(x + 3, y + 3)
  const active = (await card.boundingBox())!
  await page.mouse.move(x + 3 + left - active.x, y + 3 + top - active.y, { steps: 15 })
  await page.mouse.up()
  await expect.poll(async () => Math.abs((await card.boundingBox())!.x - left) + Math.abs((await card.boundingBox())!.y - top)).toBeLessThan(3)
}

export async function assertRoutesAvoidCards(page: Page, exclude = '.react-flow__node-group:has(.studio-group-expanded)') {
  await expect.poll(() => page.locator('[data-route-state="pending"]').count(), { timeout: 30_000 }).toBe(0)
  await expect(page.locator('[data-route-state="blocked"]')).toHaveCount(0)
  const evidence = await page.locator('[data-route-id]').evaluateAll((routes, selector) => routes.map(route => {
    const curve = route.querySelector('path') as SVGPathElement
    const matrix = curve.getScreenCTM()!.inverse()
    const points = JSON.parse(route.getAttribute('data-route-points')!) as Array<{ x: number; y: number }>
    const boxes = [...document.querySelectorAll('.react-flow__node')].filter(node => !node.matches(selector)).map(node => {
      const rect = node.getBoundingClientRect()
      const a = new DOMPoint(rect.left, rect.top).matrixTransform(matrix), b = new DOMPoint(rect.right, rect.bottom).matrixTransform(matrix)
      return { id: node.getAttribute('data-id'), x: a.x, y: a.y, right: b.x, bottom: b.y }
    })
    const collisions: unknown[] = []
    for (let i = 1; i < points.length; i++) {
      const a = points[i - 1], b = points[i]
      if (Math.abs(a.x - b.x) > .1 && Math.abs(a.y - b.y) > .1) collisions.push({ diagonal: [a, b] })
      for (const box of boxes) {
        const horizontal = Math.abs(a.y - b.y) < .1 && a.y > box.y + .5 && a.y < box.bottom - .5 && Math.max(a.x, b.x) > box.x + .5 && Math.min(a.x, b.x) < box.right - .5
        const vertical = Math.abs(a.x - b.x) < .1 && a.x > box.x + .5 && a.x < box.right - .5 && Math.max(a.y, b.y) > box.y + .5 && Math.min(a.y, b.y) < box.bottom - .5
        if (horizontal || vertical) collisions.push({ node: box.id, segment: [a, b] })
      }
    }
    return { id: route.getAttribute('data-route-id'), points, boxes, collisions }
  }), exclude)
  for (const route of evidence) expect(route.collisions, JSON.stringify(route)).toEqual([])
  await test.info().attach('canvas-routes.json', { body: JSON.stringify(evidence, null, 2), contentType: 'application/json' })
  return evidence
}
