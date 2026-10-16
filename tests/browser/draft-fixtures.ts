import { expect, type Page } from '@playwright/test'

const drafts = new Map<string, string>()

export async function fixtureDraftId(page: Page, workflowId: string, token?: string) {
  if (drafts.has(workflowId)) return drafts.get(workflowId)!
  const accessToken = token ?? ((await (await page.request.post('/api/v1/auth/refresh')).json()) as {accessToken:string}).accessToken
  const response = await page.request.get(`/api/v1/workflows/${workflowId}/drafts`, {headers:{Authorization:`Bearer ${accessToken}`}})
  expect(response.ok(), await response.text()).toBeTruthy()
  const values = await response.json() as Array<{id:string}>
  expect(values.length, 'Fixtures with multiple canvases must explicitly select a Draft').toBe(1)
  drafts.set(workflowId, values[0].id)
  return values[0].id
}

export function fixtureDraftIdKnown(workflowId: string) {
  const id = drafts.get(workflowId)
  if (!id) throw new Error(`Select the fixture Draft before registering an event assertion for ${workflowId}`)
  return id
}
