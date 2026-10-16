import { useQueries } from '@tanstack/react-query'

import { apiRequest } from '../../../shared/api/client'
import type { ResourceOption, ResourceType } from '../model/types'

export type ResourceOptionOperation = NonNullable<ResourceOption['operation']>
export type ResourceOptionRequest = { resourceType: ResourceType; operation: ResourceOptionOperation }

type ResourceOptionPage = { items: Array<{ id: string; name: string; detail: string; status: string; resourceVersionId?: string | null; accessState: ResourceOption['accessState']; pendingRequestId?: string | null; requirements?: ResourceOption['requirements']; metadata?: ResourceOption['metadata'] }>; page: number; pageSize: number; total: number }

export function useResourceOptions(workflowId: string, draftId: string, requests: ResourceOptionRequest[]) {
  const normalized = deduplicateRequests(requests)
  const queries = useQueries({ queries: normalized.map(({ resourceType, operation }) => ({
    queryKey: ['studio-resource-options', workflowId, draftId, resourceType, operation],
    queryFn: () => loadResourceOptions(workflowId, draftId, { resourceType, operation }),
    enabled: Boolean(workflowId && draftId),
    staleTime: 10_000,
    refetchInterval: (query: { state: { data?: ResourceOption[] } }) => query.state.data?.some((item: ResourceOption) => item.accessState === 'pending') ? 5_000 : false,
    refetchOnWindowFocus: 'always' as const,
    retry: false,
  })) })
  const options = normalized.reduce<Partial<Record<ResourceType, ResourceOption[]>>>((result, request, index) => {
    result[request.resourceType] = [...(result[request.resourceType] ?? []), ...(queries[index].data ?? [])]
    return result
  }, {})
  return { options, loading: queries.some((query) => query.isLoading), errors: queries.filter((query) => query.error).map((query) => query.error) }
}

export async function loadResourceOptions(workflowId: string, draftId: string, { resourceType, operation }: ResourceOptionRequest) {
  const items: ResourceOption[] = []
  let page = 1
  let hasMore = true
  while (hasMore) {
    const response = await apiRequest<ResourceOptionPage>(`/workflows/${workflowId}/resource-options?draftId=${draftId}&resourceType=${resourceType}&operation=${operation}&page=${page}&pageSize=100`)
    items.push(...response.items.map((item) => ({ resourceType, operation, value: item.id, label: item.name, detail: item.detail, status: item.status, versionId: item.resourceVersionId, accessState: item.accessState, pendingRequestId: item.pendingRequestId, requirements: item.requirements, metadata: item.metadata })))
    hasMore = items.length < response.total && response.items.length > 0
    if (hasMore) page += 1
  }
  return items
}

function deduplicateRequests(requests: ResourceOptionRequest[]) {
  return [...new Map(requests.map((request) => [`${request.resourceType}:${request.operation}`, request])).values()]
}
