import { useDesignOnly } from '../model/design-effects'
import { useQueries } from '@tanstack/react-query'

import { apiRequest } from '../../../shared/api/client'
import type { NodeManifest, ResourceOption, ResourceReference } from '../model/types'

type ProviderResponse = { items: Array<{ value: string; label: string; description?: string | null; manifest?: NodeManifest }> }
const EMPTY_PARAMETERS: Record<string, unknown> = {}
const EMPTY_OPTIONS: ResourceOption[] = []
const selectProviderOptions = (response: ProviderResponse): ResourceOption[] => response.items.map((item) => ({ value: item.value, label: item.label, detail: item.description ?? undefined, manifest: item.manifest }))

export function useProviderOptions(manifest?: NodeManifest, parameters?: Record<string, unknown>, resourceReferences: ResourceReference[] = []) {
  const designOnly = useDesignOnly()
  const currentParameters = parameters ?? EMPTY_PARAMETERS
  const fields = Object.entries(manifest?.uiSchema.fields ?? {}).filter((entry) => entry[1].control === 'provider_options' && entry[1].provider)
  const queries = useQueries({
    queries: fields.map(([, ui]) => ({
      queryKey: ['node-provider-options', manifest?.nodeType, manifest?.version, ui.provider, currentParameters, resourceReferences],
      queryFn: ({ signal }: { signal: AbortSignal }) => {
        const query = new URLSearchParams({ parameters: JSON.stringify(currentParameters) })
        query.set('resourceReferences', JSON.stringify(resourceReferences))
        return apiRequest<ProviderResponse>(`/node-definitions/${encodeURIComponent(manifest!.nodeType)}/versions/${manifest!.version}/providers/${encodeURIComponent(ui.provider!)}?${query}`, { signal })
      },
      enabled: (!designOnly || (manifest?.capability === 'builtin' && (
        (manifest.nodeType === 'sub_workflow' && ui.provider === 'workflow_versions')
        || (manifest.nodeType === 'approval' && ui.provider === 'users')
      ))) && Boolean(manifest && ui.provider),
      staleTime: 30_000,
      retry: false,
      select: selectProviderOptions,
    })),
  })
  return Object.fromEntries(fields.map(([field], index) => [field, queries[index]?.data ?? EMPTY_OPTIONS])) as Record<string, ResourceOption[]>
}
