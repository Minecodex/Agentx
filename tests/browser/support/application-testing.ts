import { expect, type Page } from '@playwright/test'

type Json = Record<string, unknown>
export async function testingRequest<T>(page: Page, token: string, path: string, method = 'GET', data?: unknown): Promise<T> {
  const response = await page.request.fetch(`/api/v1${path}`, { method, data, headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': crypto.randomUUID() } })
  expect(response.ok(), `${path}: ${response.status()} ${await response.text()}`).toBeTruthy()
  return response.status() === 204 ? undefined as T : response.json() as Promise<T>
}

export async function ensureScoringModel(page: Page, token: string) {
  const setting = await testingRequest<{ version: number; status: string }>(page, token, '/platform-ai/model-setting')
  if (setting.status === 'ready') return
  const endpoint = process.env.AGENTX_E2E_ECHO_BASE_URL
  if (!endpoint) throw new Error('A real scoring model or AGENTX_E2E_ECHO_BASE_URL is required')
  const me = await testingRequest<{ departmentId: string }>(page, token, '/auth/me')
  const model = await testingRequest<{ id: string }>(page, token, '/models/aliases', 'POST', { alias: `scoring-${Date.now()}`, connectionName: 'Evaluation fixture', endpoint: `${endpoint.replace(/\/$/, '')}/v1`, modelName: 'echo-model', providerType: 'openai_compatible', ownerDepartmentId: me.departmentId, price: { currency: 'USD', inputPerMillion: '1', outputPerMillion: '2' } })
  await testingRequest(page, token, '/platform-ai/model-setting', 'PUT', { expectedVersion: setting.version, modelAliasId: model.id })
}

export async function beginApplicationEvaluation(page: Page, token: string, options: { applicationId?: string; workflowId: string; versionId: string; name: string; input: Json; expectedOutput: unknown }) {
  await ensureScoringModel(page, token)
  const app = options.applicationId ? { id: options.applicationId } : await testingRequest<{ id: string }>(page, token, '/applications', 'POST', { name: options.name, slug: `evaluation-${crypto.randomUUID()}`, workflowId: options.workflowId, visibility: 'company' })
  const path = `/applications/${app.id}`
  const dataset = await testingRequest<{ id: string; name: string; revision: number }>(page, token, `${path}/datasets`, 'POST', { name: options.name, workflowVersionId: options.versionId })
  await testingRequest(page, token, `${path}/datasets/${dataset.id}/cases`, 'POST', { expectedRevision: dataset.revision, name: options.name, input: options.input, expectedOutput: options.expectedOutput })
  const profile = await testingRequest<{ versionId: string }>(page, token, `${path}/evaluation-profiles`, 'POST', { name: options.name, criteria: 'Evaluate correctness of actual output against the reference, accepting equivalent wording.', passThreshold: 0.8 })
  const batch = await testingRequest<{ id: string }>(page, token, `${path}/evaluations`, 'POST', { name: options.name, profileVersionId: profile.versionId, selections: [{ datasetId: dataset.id, expectedRevision: dataset.revision + 1, workflowVersionIds: [options.versionId] }] })
  return { ...batch, dataset, applicationId: app.id, reportPath: `${path}/evaluations/${batch.id}/report` }
}
