import type { NodeManifest, StudioEdge, StudioNode } from '../model/types'
import { studioNodeGeometry } from '../nodes/canvas-node-geometry'
import { manifestForNode } from './manifest-lookup'

/** Port kinds are derived from current geometry after load, for rendering and ELK. */
export function canvasEdgePortKinds(edges: StudioEdge[], nodes: StudioNode[], manifests = new Map<string, NodeManifest>()) {
  const ports = new Map(nodes.map(node => [node.id, studioNodeGeometry(node, manifestForNode(manifests, node)).ports]))
  return edges.map(edge => {
    const sourcePortKind = ports.get(edge.source)?.find(port => port.direction === 'output' && port.id === (edge.sourceHandle ?? 'main'))?.kind
    const targetPortKind = ports.get(edge.target)?.find(port => port.direction === 'input' && port.id === (edge.targetHandle ?? 'main'))?.kind
    if ((!sourcePortKind || edge.data?.sourcePortKind === sourcePortKind) && (!targetPortKind || edge.data?.targetPortKind === targetPortKind)) return edge
    return { ...edge, data: { edgeKind: 'execution' as const, ...edge.data, ...(sourcePortKind ? { sourcePortKind } : {}), ...(targetPortKind ? { targetPortKind } : {}) } }
  })
}
