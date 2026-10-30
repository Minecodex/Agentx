import type { StudioEdge } from '../model/types'

export function proxyEdges(edges: StudioEdge[], collapsedByMember: Map<string, string>): StudioEdge[] {
  return edges.flatMap((edge) => {
    const source = collapsedByMember.get(edge.source)
    const target = collapsedByMember.get(edge.target)
    if (source && source === target) return []
    const error = edge.data?.sourcePortKind === 'error' || edge.sourceHandle === 'error'
    return [{ ...edge, data: error ? { edgeKind: 'execution', ...edge.data, sourcePortKind: 'error' } : edge.data, source: source ?? edge.source, target: target ?? edge.target, sourceHandle: source ? error ? 'group-error-out' : 'group-out' : edge.sourceHandle, targetHandle: target ? (edge.targetHandle === 'error' || edge.data?.targetPortKind === 'error') ? 'group-error-in' : 'group-in' : edge.targetHandle }]
  })
}
