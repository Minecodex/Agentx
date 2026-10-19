import { expect } from '@playwright/test'
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { completed, create, login, read, saved, send, setModel, test, type Created } from '../studio-builder-support'
import { useRuntimePortForward } from './playground-helpers'

const directory = process.env.AGENTX_INTERACTION_AI_DIR!

test('AI authors an Agent, progress reply, raw input wait and two Agent branches', async ({ page }, info) => {
  test.setTimeout(900_000)
  await useRuntimePortForward(page)
  const token = await login(page)
  const phase = process.env.AGENTX_INTERACTION_AI_PHASE ?? 'create'
  if (phase !== 'review') await setModel(page, process.env.AGENTX_E2E_STUDIO_LIVE_ALIAS!)
  let created: Created
  if (phase === 'create') {
    created = await create(page, `AI 语音确认 · Hook 分流闭环 ${process.env.AGENTX_E2E_RUN_ID}`)
    await writeFile(join(directory, 'created.json'), JSON.stringify(created, null, 2))
  } else {
    created = JSON.parse(await readFile(join(directory, 'created.json'), 'utf8')) as Created
    await page.goto(`/workflows/${created.id}/versions/${created.createdVersionId}/editor`)
    await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  }
  if (phase !== 'review') {
    await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
    const prompt = phase === 'create'
      ? await readFile(join(directory, 'prompt.txt'), 'utf8')
      : await readFile(join(directory, 'repair.txt'), 'utf8')
    for (let attempt = 0; attempt < 3; attempt++) {
      await send(page, prompt)
      try {
        await completed(page, false, 720_000)
        break
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        await writeFile(join(directory, `AI-${phase}-attempt-${attempt + 1}.txt`), message)
        if (attempt === 2 || !message.includes('outcome_unknown')) throw error
      }
    }
    const draft = await saved(page, token, created)
    await writeFile(join(directory, 'ai-draft.json'), JSON.stringify(draft, null, 2))
    await info.attach('AI-created-workflow', { body: JSON.stringify(draft), contentType: 'application/json' })
    await page.getByTestId('studio-builder-panel').getByRole('button', { name: '关闭', exact: true }).click()
  }
  await page.setViewportSize({ width: 1920, height: 1080 })
  await page.getByRole('button', { name: '适应画布', exact: true }).click()
  await expect(page.locator('.react-flow__node[data-id="__start__"]')).toBeInViewport({ ratio: 1 })
  await expect(page.locator('.react-flow__node').last()).toBeInViewport({ ratio: 1 })
  await page.screenshot({ path: join(directory, `workflow-${phase}.png`), fullPage: true })
  await page.reload()
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  const reopened = await read<{ definition: { nodes: Array<{ id: string; key: string; type: string; typeVersion: number }> } }>(
    page, token, phase === 'review'
      ? `/workflows/${created.id}/versions/${created.createdVersionId}`
      : `/workflows/${created.id}/drafts/${created.documentId}`,
  )
  expect(reopened.definition.nodes.filter(node => node.type === 'agent')).toHaveLength(3)
  expect(reopened.definition.nodes.filter(node => node.type === 'reply_message')).toHaveLength(1)
  expect(reopened.definition.nodes.filter(node => node.type === 'await_input')).toHaveLength(1)
  expect(reopened.definition.nodes.filter(node => node.type === 'if')).toHaveLength(1)
  expect(reopened.definition.nodes.filter(node => node.type === 'agent').every(node => node.typeVersion === 2)).toBe(true)
  if (phase === 'review') {
    const results = JSON.parse(await readFile(join(directory, 'api-results.json'), 'utf8')) as Array<{
      accepted: { executionId: string }; expectedAgentId: string; excludedAgentId: string; asrReply: string
    }>
    for (const [index, result] of results.entries()) {
      const details = JSON.parse(await readFile(join(directory, `execution-${index + 1}-details.json`), 'utf8')) as {
        agentRuns: Array<{ status: string; modelCallCount: number }>
        inputWaits: Array<{ content: unknown; status: string }>
      }
      expect(details.agentRuns).toHaveLength(2)
      expect(details.agentRuns.every(run => run.status === 'succeeded' && run.modelCallCount > 0)).toBe(true)
      expect(details.inputWaits).toHaveLength(1)
      expect(details.inputWaits[0]).toMatchObject({ status: 'received', content: result.asrReply })
      const nodes = await read<{ items: Array<{ nodeId: string; status: string }> }>(
        page, token, `/executions/${result.accepted.executionId}/nodes`,
      )
      const expected = reopened.definition.nodes.find(node => node.id === result.expectedAgentId)!
      const excluded = reopened.definition.nodes.find(node => node.id === result.excludedAgentId)!
      expect(expected).toBeTruthy()
      expect(excluded).toBeTruthy()
      expect(nodes.items.some(node => node.nodeId === expected.id && node.status === 'succeeded')).toBe(true)
      expect(nodes.items.some(node => node.nodeId === excluded.id && node.status === 'succeeded')).toBe(false)
      await writeFile(join(directory, `execution-branch-${index + 1}-nodes.json`), JSON.stringify(nodes, null, 2))
      await page.goto(`/executions/${result.accepted.executionId}`)
      await expect(page.getByText('成功', { exact: true }).first()).toBeVisible({ timeout: 60_000 })
      await page.screenshot({ path: join(directory, `execution-branch-${index + 1}.png`), fullPage: true })
    }
  }
})
