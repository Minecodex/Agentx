import { expect } from '@playwright/test'
import type { components } from '../../../src/web/src/shared/api/generated'
import { test, allEntries, completed, count, fixtureCanvas, read, saved, send } from '../studio-builder-support'

type Ack = components['schemas']['StudioEditorAcknowledgementV1']

test('P8-E2E-11: reload before layout ACK replays the durable geometry once', async ({ page }, info) => {
  const { token, created } = await fixtureCanvas(page, 'P8 layout receipt recovery')
  let blocked = true
  let original: Ack | undefined
  const delivered: Ack[] = []
  let session = ''
  await page.route('**/studio-builder/sessions/*/acknowledgements', async route => {
    if (blocked) {
      original ??= route.request().postDataJSON() as Ack
      session = new URL(route.request().url()).pathname.split('/').at(-2)!
      await route.abort('connectionreset')
    } else {
      delivered.push(route.request().postDataJSON() as Ack)
      await route.continue()
    }
  })
  const runId = await send(page, 'P8FIX: whole with_loop label=LayoutRecovered')
  await expect.poll(() => original?.status, { timeout: 90_000 }).toBe('applied')
  // React Flow may measure newly mounted cards after the command is applied.
  // Preserve that saved canvas too, while replaying the exact earlier receipt.
  const beforeReload = await saved(page, token, created) as unknown as Ack['snapshot']['document']
  expect((await read<{ status: string }>(page, token, `/studio-builder/sessions/${session}/commands/${original!.commandId}/receipt`)).status).toBe('pending')
  await page.reload()
  await expect(page.getByTestId('studio-loop-p8-loop')).toBeAttached()
  blocked = false
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  await completed(page)
  const replayed = delivered.filter(ack => ack.commandId === original!.commandId)
  expect(replayed.length).toBeGreaterThan(0)
  for (const ack of replayed) expect(ack).toEqual(original)
  const result = await saved(page, token, created) as unknown as Ack['snapshot']['document']
  expect(result.definition).toEqual(original!.snapshot.document.definition)
  expect(result.editorDocument).toEqual(beforeReload.editorDocument)
  const entries = await allEntries<{ runId: string; payload: { type: string; commandId?: string; status?: string } }>(page, token, session)
  expect(entries.filter(entry => entry.runId === runId && entry.payload.type === 'command_acknowledged' && entry.payload.status === 'applied')).toHaveLength(1)
  expect(await count(page, token, created.id)).toBe(0)
  await info.attach('durable-layout-receipt.json', { body: JSON.stringify(original), contentType: 'application/json' })
})
