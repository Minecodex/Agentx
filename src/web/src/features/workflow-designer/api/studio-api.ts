import type { components } from '../../../shared/api/generated'
import { apiRequest, jsonBody } from '../../../shared/api/client'
import type { NodeManifest } from '../model/types'

export type NodeDefinitionSummary = components['schemas']['NodeDefinitionSummary']
export type ExecutionEvent = components['schemas']['ExecutionEventResponse']
export type ExecutionCommand = components['schemas']['ExecutionCommandResponse']
export type DebugOverlay = components['schemas']['DebugOverlayResponse']

export async function loadNodeCatalog() {
  const page = await apiRequest<{ items: NodeDefinitionSummary[] }>('/node-definitions?pageSize=100')
  const details = await Promise.all(page.items.map((item) => apiRequest<components['schemas']['NodeDefinitionDetail']>(`/node-definitions/${encodeURIComponent(item.nodeType)}/versions/${item.version}`)))
  return details.map((detail) => ({ ...detail, manifest: detail.manifest as NodeManifest }))
}

export async function loadNodeDefinition(nodeType: string, version: number) {
  const detail = await apiRequest<components['schemas']['NodeDefinitionDetail']>(`/node-definitions/${encodeURIComponent(nodeType)}/versions/${version}`)
  return { ...detail, manifest: detail.manifest as NodeManifest }
}

export type ResolvedNodeDefinition = {
  status: 'complete' | 'incomplete' | 'invalid'
  inputPorts?: NodeManifest['inputPorts']
  outputPorts?: NodeManifest['outputPorts']
  outputSchema?: unknown
  outputPortSchemas?: NodeManifest['outputPortSchemas']
  issues?: Array<{ path: string; code: string; message: string }>
}

export const resolveNodeDefinition = (
  nodeType: string,
  version: number,
  configuration: Record<string, unknown>,
  upstreamContracts: Record<string, unknown>,
  signal?: AbortSignal,
) => apiRequest<ResolvedNodeDefinition>(`/node-definitions/${encodeURIComponent(nodeType)}/versions/${version}/resolve`, {
  method: 'POST',
  body: jsonBody({ configuration, upstreamContracts }),
  signal,
})

export const saveDraft = (workflowId: string, draftId: string, expectedRevision: number, definition: unknown, editorDocument: unknown) => apiRequest<components['schemas']['DraftResponse']>(`/workflows/${workflowId}/drafts/${draftId}`, { method: 'PUT', headers: { 'Idempotency-Key': crypto.randomUUID() }, body: jsonBody({ expectedRevision, definition, editorDocument }) })
export const validateDraft = (workflowId: string, draftId: string, definition: unknown, editorDocument: unknown) => apiRequest<components['schemas']['ValidateDraftResponse']>(`/workflows/${workflowId}/drafts/${draftId}/validate`, { method: 'POST', body: jsonBody({ definition, editorDocument }) })
export const startDebugExecution = (workflowId: string, draftId: string, request: components['schemas']['DebugExecutionRequest']) => apiRequest<ExecutionCommand>(`/workflows/${workflowId}/drafts/${draftId}/debug-executions`, { method: 'POST', body: jsonBody(request) })
export const loadExecutionEvents = (executionId: string, after = 0) => apiRequest<components['schemas']['ExecutionEventListResponse']>(`/executions/${executionId}/events?after=${after}&limit=200`)
export const cancelExecution = (executionId: string) => apiRequest<void>(`/executions/${executionId}/cancel`, { method: 'POST' })
export const saveOverlay = (workflowId: string, draftId: string, nodeId: string, kind: string, payload: unknown) => apiRequest<DebugOverlay>(`/workflows/${workflowId}/drafts/${draftId}/debug-overlays/${encodeURIComponent(nodeId)}`, { method: 'PUT', body: jsonBody({ kind, payload }) })
export const deleteOverlay = (workflowId: string, draftId: string, nodeId: string) => apiRequest<void>(`/workflows/${workflowId}/drafts/${draftId}/debug-overlays/${encodeURIComponent(nodeId)}`, { method: 'DELETE' })
