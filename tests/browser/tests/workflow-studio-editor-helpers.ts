import { expect, type Locator, type Page } from '@playwright/test'

export async function showEdgeToolbar(edge: Locator, toolbar: Locator) {
  const path = edge.locator('path[stroke="transparent"]').first()
  await expect(path).toBeAttached()
  await expect.poll(() => path.evaluate((element) => (element as SVGPathElement).getTotalLength())).toBeGreaterThan(0)
  const point = await path.evaluate((element) => {
    const curve = element as SVGPathElement
    const matrix = curve.getScreenCTM()
    if (!matrix) throw new Error('Edge has no screen transform')
    // The label toolbar covers the midpoint; hovering a quarter along the
    // actual stroke reveals it without accidentally clicking Delete.
    const owner = curve.closest('.react-flow__edge')!
    const fractions = [.25, .75, ...Array.from({ length: 19 }, (_, index) => (index + 1) / 20)]
    for (const fraction of fractions) {
      const local = curve.getPointAtLength(curve.getTotalLength() * fraction)
      const screen = new DOMPoint(local.x, local.y).matrixTransform(matrix)
      const hit = document.elementFromPoint(screen.x, screen.y)
      if (hit && owner.contains(hit)) return { x: screen.x, y: screen.y }
    }
    throw new Error('Edge has no unobstructed stroke in the viewport')
  })
  await edge.page().mouse.move(point.x, point.y)
  await expect(toolbar).toHaveCSS('opacity', '1')
}

export async function resizeLoop(page: Page, loopFrame: Locator, resizeHandle: Locator) {
  const viewport = page.viewportSize()
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await page.locator('.react-flow__controls-zoomout').click()
  }
  await expect.poll(async () => {
    const handle = await resizeHandle.boundingBox()
    return Boolean(handle && viewport && handle.x + handle.width / 2 < viewport.width - 8 && handle.y + handle.height / 2 < viewport.height - 8)
  }).toBeTruthy()
  const before = await loopFrame.boundingBox()
  expect(before).toBeTruthy()
  const zoom = await page.locator('.react-flow__viewport').evaluate((element) => new DOMMatrix(getComputedStyle(element).transform).a)
  const deltaX = 160 * zoom
  const deltaY = 100 * zoom
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const handle = await resizeHandle.boundingBox()
    if (!handle) throw new Error('Loop resize handle is not measurable')
    await resizeHandle.hover({ force: true })
    await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2)
    await page.mouse.down()
    await page.mouse.move(handle.x + handle.width / 2 + deltaX, handle.y + handle.height / 2 + deltaY, { steps: 12 })
    await page.mouse.up()
    if (((await loopFrame.boundingBox())?.width ?? 0) > before!.width + deltaX / 2) break
  }
  await expect.poll(async () => (await loopFrame.boundingBox())?.width ?? 0).toBeGreaterThan(before!.width + deltaX / 2)
}

export async function fillMonaco(page: Page, scope: Locator, value: string, paste = false) {
  await scope.scrollIntoViewIfNeeded()
  const monaco = scope.getByRole('textbox', { name: 'Editor content' })
  const richText = scope.locator('[contenteditable="true"]').first()
  const fallback = scope.locator('textarea:not([readonly]):not([aria-hidden="true"])').first()
  await expect.poll(async () => await monaco.isVisible().catch(() => false)
    || await fallback.isVisible().catch(() => false)
    || await richText.isVisible().catch(() => false), { timeout: 30_000 }).toBeTruthy()
  const usesMonaco = await monaco.isVisible().catch(() => false)
  const usesFallback = !usesMonaco && await fallback.isVisible().catch(() => false)
  const editor = usesMonaco ? monaco : usesFallback ? fallback : richText
  if (usesMonaco) {
    await editor.focus()
    await page.keyboard.press('Control+A')
    if (paste) {
      await editor.evaluate((element, source) => {
        const clipboardData = new DataTransfer()
        clipboardData.setData('text/plain', source)
        element.dispatchEvent(new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }))
      }, value)
    } else await page.keyboard.insertText(value)
    await page.keyboard.press('Control+Home')
    await expect.poll(async () => (await scope.locator('.view-lines:visible').textContent())?.replaceAll('\u00a0', ' ')).toContain(value.split('\n')[0])
  } else if (usesFallback) {
    await editor.fill(value)
    await expect(editor).toHaveValue(value)
  } else {
    await editor.fill(value)
    await expect(editor).toContainText(value.split('\n')[0])
  }
  await page.keyboard.press('Escape')
  await editor.blur()
  await page.waitForTimeout(100)
}
