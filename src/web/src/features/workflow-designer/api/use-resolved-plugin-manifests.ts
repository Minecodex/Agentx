import { useQuery } from '@tanstack/react-query'
import { useMemo } from 'react'

import { apiRequest, jsonBody } from '../../../shared/api/client'
import { serializeStudio } from '../model/serializer'
import type { NodeManifest, StudioDocument } from '../model/types'
import type { ResolvedNodeDefinition } from './studio-api'

export type PluginDefinitionState = {
  status: 'complete' | 'incomplete' | 'invalid'
  issues: Array<{ path: string; code: string; message: string }>
}

export function useResolvedPluginManifests(
  workflowId: string,
  draftId: string,
  document: StudioDocument,
  manifests: Map<string, NodeManifest>,
) {
  const { definition } = useMemo(() => serializeStudio(document), [document])
  const candidates = useMemo(() => document.nodes.flatMap((node) => {
    if (node.data.editorKind !== 'action' || node.data.disabled) return []
    const manifest = manifests.get(`${node.data.nodeType}@${node.data.typeVersion}`)
    if (!manifest?.plugin || manifest.capability !== 'plugin_nodejs' || manifest.plugin.packageId.startsWith('agentx/')) return []
    return [{ node, manifest }]
  }), [document.nodes, manifests])
  const contractInputs = {
    start: definition.start,
    nodes: definition.nodes.filter(node => node.type !== 'exit'),
    connections: definition.connections,
  }
  const query = useQuery({
    queryKey: ['resolved-workflow-plugin-definitions', workflowId, draftId, contractInputs,
      candidates.map(({ manifest }) => manifest.plugin?.bundleDigest)],
    enabled: Boolean(workflowId && draftId) && candidates.length > 0,
    queryFn: async ({ signal }) => {
      await new Promise<void>((resolve, reject) => {
        const timer = setTimeout(() => { signal.removeEventListener('abort', abort); resolve() }, 150)
        const abort = () => { clearTimeout(timer); reject(new DOMException('Aborted', 'AbortError')) }
        signal.addEventListener('abort', abort, { once: true })
        if (signal.aborted) abort()
      })
      return apiRequest<{ nodes: Record<string, ResolvedNodeDefinition> }>(`/workflows/${workflowId}/drafts/${draftId}/resolve-plugins`, {
        method: 'POST', body: jsonBody({ definition }), signal,
      })
    },
    staleTime: Infinity,
    retry: false,
  })
  return useMemo(() => {
    const resolved = new Map<string, NodeManifest>()
    const states = new Map<string, PluginDefinitionState>()
    candidates.forEach(({ node, manifest }) => {
      const value = query.data?.nodes[node.id]
      if (!value) return
      resolved.set(node.id, {
        ...manifest,
        inputPorts: value.inputPorts ?? manifest.inputPorts,
        outputPorts: value.outputPorts ?? manifest.outputPorts,
        outputSchema: value.outputSchema ?? manifest.outputSchema,
        outputPortSchemas: value.outputPortSchemas ?? manifest.outputPortSchemas,
      })
      states.set(node.id, { status: value.status, issues: value.issues ?? [] })
    })
    return { resolved, states }
  }, [candidates, query.data])
}
