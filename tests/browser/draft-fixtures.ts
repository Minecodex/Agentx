import { expect, type Page } from '@playwright/test'

const drafts = new Map<string, string>()
const versions = new Map<string, string>()
async function authorization(page: Page, token?: string) {
  return token ?? ((await (await page.request.post('/api/v1/auth/refresh')).json()) as {accessToken:string}).accessToken
}
export async function fixtureVersionId(page: Page, workflowId: string, token?: string) {
  if (versions.has(workflowId)) return versions.get(workflowId)!
  const response=await page.request.get(`/api/v1/workflows/${workflowId}/versions`,{headers:{Authorization:`Bearer ${await authorization(page,token)}`}})
  expect(response.ok(),await response.text()).toBeTruthy()
  const values=await response.json() as Array<{id:string;versionNumber:number}>
  const initial=values.find(value=>value.versionNumber===1)!
  expect(initial).toBeTruthy()
  versions.set(workflowId,initial.id)
  return initial.id
}
export async function fixtureDraftId(page: Page, workflowId: string, token?: string) {
  if (drafts.has(workflowId)) return drafts.get(workflowId)!
  const accessToken=await authorization(page,token)
  const version=await fixtureVersionId(page,workflowId,accessToken)
  const response=await page.request.post(`/api/v1/workflows/${workflowId}/versions/${version}/edit`,{headers:{Authorization:`Bearer ${accessToken}`}})
  expect(response.ok(),await response.text()).toBeTruthy()
  const context=await response.json() as {document:{id:string}}
  drafts.set(workflowId,context.document.id)
  return context.document.id
}
export function fixtureDraftIdKnown(workflowId: string) {
  const id=drafts.get(workflowId)
  if (!id) throw new Error(`Open the fixture version before asserting document events for ${workflowId}`)
  return id
}
export async function publishFixtureResponse(page: Page, token: string, workflow: string, revision: number) {
  const headers={Authorization:`Bearer ${token}`,'Idempotency-Key':crypto.randomUUID()}
  const draft=await (await page.request.get(`/api/v1/workflows/${workflow}/drafts/${await fixtureDraftId(page,workflow,token)}`,{headers})).json() as {versionId:string;publicationStatus:string}
  const environments=await (await page.request.get('/api/v1/environments',{headers})).json() as Array<{id:string;code:string}>
  const response=await page.request.post(`/api/v1/workflows/${workflow}/deployments`,{headers,data:{environmentId:environments.find(item=>item.code==='development')!.id,workflowVersionId:draft.versionId,contentSource:draft.publicationStatus==='published'?'draft':'version',expectedRevision:revision}})
  if (!response.ok()) return response
  const deployment=await response.json() as {workflowVersionId:string}
  return page.request.get(`/api/v1/workflows/${workflow}/versions/${deployment.workflowVersionId}`,{headers})
}
export async function publishFixtureVersion<T = {id:string;versionNumber:number}>(page: Page, token: string, workflow: string, revision: number):Promise<T> {
  const response=await publishFixtureResponse(page,token,workflow,revision)
  expect(response.ok(),await response.text()).toBeTruthy()
  return response.json() as Promise<T>
}
