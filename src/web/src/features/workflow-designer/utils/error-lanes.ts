import type { StudioEdge } from '../model/types'

/** Shared targets remain in the main flow; only exclusively exceptional paths move down. */
export function errorOnlyNodes(ids: string[], edges: StudioEdge[], isError: (edge: StudioEdge) => boolean = edge => edge.sourceHandle === 'error' || edge.data?.sourcePortKind === 'error') {
  const incoming = new Set(edges.map(edge => edge.target))
  const outgoing = new Map<string, string[]>()
  for (const edge of edges) if (!isError(edge)) outgoing.set(edge.source, [...(outgoing.get(edge.source) ?? []), edge.target])
  const walk = (seeds: string[]) => {
    const seen = new Set(seeds), queue = [...seeds]
    for (let i = 0; i < queue.length; i++) for (const target of outgoing.get(queue[i]) ?? []) if (!seen.has(target)) { seen.add(target); queue.push(target) }
    return seen
  }
  const main = walk(ids.filter(id => !incoming.has(id)))
  return new Set([...walk(edges.filter(isError).map(edge => edge.target))].filter(id => !main.has(id)))
}
