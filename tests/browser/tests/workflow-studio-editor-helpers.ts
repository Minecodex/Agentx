import { expect, type Locator, type Page } from '@playwright/test'

export function sandboxImageDigest(): string {
  const image = process.env.AGENTX_E2E_SANDBOX_IMAGE
  if (!image?.includes('@sha256:')) throw new Error('The pytest harness must provide the verified Sandbox image digest')
  return image
}

export async function showEdgeToolbar(edge: Locator, toolbar: Locator) {
  await edge.locator('path[stroke="transparent"]').hover()
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
  const editor = scope.getByRole('textbox', { name: 'Editor content' })
  // Source controls lazy-load Monaco; its loading textarea is transient.
  // The output-example control is a permanent plain textarea.
  if (await scope.getAttribute('data-testid') === 'parameter-source') {
    await expect(editor).toBeVisible({ timeout: 30_000 })
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
  } else {
    const textarea = scope.locator('textarea:not([readonly])')
    await textarea.fill(value)
    await expect(textarea).toHaveValue(value)
  }
  // Blur the current focus to commit; Escape would close the entire inspector.
  await page.evaluate(() => {
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur()
  })
  await page.waitForTimeout(100)
}
