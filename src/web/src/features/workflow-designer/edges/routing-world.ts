import type { CanvasNode, NodeManifest, StudioEdge } from '../model/types'
import { canvasGeometry } from '../nodes/canvas-node-geometry'
import { errorOnlyNodes } from '../utils/error-lanes'
import { ROUTE_CHANNEL_GAP, ROUTE_PADDING, type CanvasPoint, type CanvasRect } from './canvas-route'

export type RoutingDomain = { obstacles: CanvasRect[]; bounds: CanvasRect; frame?: CanvasRect; avoidAreas: Array<CanvasRect>; lanes: Map<string, number>; channels: Map<string, { side: 'source' | 'target'; index: number }> }
export type RoutingWorld = { domainFor: (source: string, target: string) => RoutingDomain; nodes: Map<string, CanvasNode>; hasQuickAdd: (nodeId: string, handleId: string) => boolean; signature: string; dragging: boolean; complete?: boolean; batches?: number }

export function routingWorld(nodes: CanvasNode[], edges: StudioEdge[], manifests = new Map<string, NodeManifest>(), quickAddEnabled = true): RoutingWorld {
  const byId = new Map(nodes.map(node => [node.id, node]))
  const occupied = new Set(edges.filter(edge => !edge.data?.derived).map(edge => `${edge.source}\u0000${edge.sourceHandle ?? 'main'}`))
  const hasQuickAdd = (nodeId: string, handleId: string) => {
    const data = byId.get(nodeId)?.data
    if (!quickAddEnabled || !data || (data.editorKind !== 'action' && data.editorKind !== 'loop-container') || (data.editorKind === 'action' && data.disabled)) return false
    const manifest = data.editorKind === 'action' ? manifests.get(`node:${nodeId}`) ?? manifests.get(`${data.nodeType}@${data.typeVersion}`) : undefined
    return !occupied.has(`${nodeId}\u0000${handleId}`) || Boolean(manifest?.outputPorts.find(port => port.name === handleId.split(':')[0])?.variadic)
  }
  const position = (node: CanvasNode): CanvasPoint => {
    const parent = node.parentId ? byId.get(node.parentId) : undefined
    const offset = parent ? position(parent) : { x: 0, y: 0 }
    return { x: node.position.x + offset.x, y: node.position.y + offset.y }
  }
  const geometries = new Map(nodes.map(node => [node.id, canvasGeometry(node.data, manifests.get(`node:${node.id}`) ?? (node.data.editorKind === 'action' ? manifests.get(`${node.data.nodeType}@${node.data.typeVersion}`) : undefined))]))
  const boxes = new Map(nodes.map(node => { const geometry = geometries.get(node.id)!; return [node.id, { id: node.id, ...position(node), width: node.width ?? node.measured?.width ?? (Number(node.style?.width) || geometry.width), height: node.height ?? node.measured?.height ?? (Number(node.style?.height) || geometry.height) }] }))
  const signature = JSON.stringify([nodes.map(node => [boxes.get(node.id), node.parentId, Boolean(node.dragging), geometries.get(node.id)!.ports.map(port => [port.id, port.direction, port.x, port.y])]), edges.map(edge => [edge.id, edge.source, edge.sourceHandle, edge.target, edge.targetHandle, edge.data?.sourcePortKind, edge.data?.order])])
  const domains = new Map<string, RoutingDomain>()
  const domain = (parentId = '') => {
    const cached = domains.get(parentId)
    if (cached) return cached
    const members = nodes.filter(node => (node.parentId ?? '') === parentId && (node.data.editorKind !== 'group' || node.data.collapsed))
    const ids = new Set(members.map(node => node.id))
    const relevantEdges = edges.filter(edge => ids.has(edge.source) && ids.has(edge.target))
    const errorIds = errorOnlyNodes([...ids], relevantEdges)
    const obstacles = members.map(node => boxes.get(node.id)!)
    const normal = obstacles.filter(box => !errorIds.has(box.id!))
    const x = Math.min(0, ...obstacles.map(box => box.x)), y = Math.min(0, ...obstacles.map(box => box.y))
    const bounds = { x, y, width: Math.max(x + 1, ...obstacles.map(box => box.x + box.width)) - x, height: Math.max(y + 1, ...normal.map(box => box.y + box.height)) - y }
    const frame = parentId ? boxes.get(parentId) : undefined
    if (frame) obstacles.push({ id: `${parentId}:header`, x: frame.x, y: frame.y, width: frame.width, height: 48 })
    const isError = (edge: StudioEdge) => edge.sourceHandle === 'error' || edge.data?.sourcePortKind === 'error'
    const normalPairs = new Set(relevantEdges.filter(edge => !isError(edge)).map(edge => `${edge.source}\u0000${edge.target}`))
    const errorInputs = new Map<string, number>()
    for (const edge of relevantEdges.filter(isError)) errorInputs.set(edge.target, (errorInputs.get(edge.target) ?? 0) + 1)
    // A branch's own End, or a dedicated error End, needs only local obstacle
    // avoidance. Sending these links below the entire canvas creates long loops.
    const localError = (edge: StudioEdge) => byId.get(edge.target)?.data.editorKind === 'exit' && (normalPairs.has(`${edge.source}\u0000${edge.target}`) || (errorIds.has(edge.target) && errorInputs.get(edge.target) === 1))
    const lanes = new Map(relevantEdges.filter(edge => (isError(edge) && !localError(edge)) || boxes.get(edge.source)!.x + boxes.get(edge.source)!.width >= boxes.get(edge.target)!.x).sort((a, b) => boxes.get(a.source)!.x - boxes.get(b.source)!.x || (a.data?.order ?? 0) - (b.data?.order ?? 0) || a.id.localeCompare(b.id)).map((edge, index) => [edge.id, index]))
    const channels = new Map<string, { side: 'source' | 'target'; index: number }>()
    const forward = relevantEdges.filter(edge => !lanes.has(edge.id))
    for (const side of ['target', 'source'] as const) {
      const bundles = new Map<string, StudioEdge[]>()
      for (const edge of forward) bundles.set(edge[side], [...(bundles.get(edge[side]) ?? []), edge])
      for (const bundle of bundles.values()) if (bundle.length > 1 && (side === 'source' || new Set(bundle.map(edge => edge.targetHandle)).size > 1)) {
        const rank = (edge: StudioEdge) => geometries.get(edge[side])?.ports.find(port => port.direction === (side === 'source' ? 'output' : 'input') && port.id === edge[`${side}Handle`])?.y ?? 24
        bundle.sort((a, b) => rank(a) - rank(b) || (a.data?.order ?? 0) - (b.data?.order ?? 0) || a.id.localeCompare(b.id)).forEach((edge, index) => channels.set(edge.id, { side, index }))
      }
    }
    const avoidAreas: CanvasRect[] = []
    if (frame) {
      // Solid outside walls keep every internal route inside its loop frame.
      const margin = 64
      const right = Math.max(...[...boxes.values()].map(box => box.x + box.width), frame.x + frame.width) + margin
      const bottom = Math.max(...[...boxes.values()].map(box => box.y + box.height), frame.y + frame.height) + margin
      const left = Math.min(...[...boxes.values()].map(box => box.x), frame.x) - margin
      const top = Math.min(...[...boxes.values()].map(box => box.y), frame.y) - margin
      avoidAreas.push({ x: frame.x - ROUTE_PADDING, y: frame.y - ROUTE_PADDING, width: frame.width + ROUTE_PADDING * 2, height: 48 + ROUTE_PADDING })
      avoidAreas.push({ x: left, y: top, width: frame.x - left - ROUTE_PADDING, height: bottom - top }, { x: frame.x + frame.width + ROUTE_PADDING, y: top, width: right - frame.x - frame.width - ROUTE_PADDING, height: bottom - top }, { x: left, y: top, width: right - left, height: frame.y - top - ROUTE_PADDING }, { x: left, y: frame.y + frame.height + ROUTE_PADDING, width: right - left, height: bottom - frame.y - frame.height - ROUTE_PADDING })
    }
    const value = { obstacles, bounds, frame, avoidAreas: avoidAreas.filter(box => box.width > 0 && box.height > 0), lanes, channels }
    domains.set(parentId, value)
    return value
  }
  return { nodes: byId, signature, hasQuickAdd, dragging: nodes.some(node => node.dragging), domainFor: (source, target) => domain(byId.get(source)?.parentId === byId.get(target)?.parentId ? byId.get(source)?.parentId : '') }
}

export function routeWaypoints(domain: RoutingDomain, edgeId: string, source: CanvasPoint, target: CanvasPoint, error: boolean): CanvasPoint[] {
  if ((!error || !domain.lanes.has(edgeId)) && source.x <= target.x) {
    const channel = domain.channels.get(edgeId)
    if (!channel || source.y === target.y) return []
    const x = channel.side === 'source' ? source.x + 24 + channel.index * ROUTE_CHANNEL_GAP : target.x - 24 - channel.index * ROUTE_CHANNEL_GAP
    // Guides allocate lanes; the library still performs every obstacle search.
    // Omit a guide inside an obstacle so it cannot prevent an otherwise valid path.
    return [{ x, y: source.y }, { x, y: target.y }].filter(point => domain.obstacles.every(box => point.x <= box.x - ROUTE_PADDING || point.x >= box.x + box.width + ROUTE_PADDING || point.y <= box.y - ROUTE_PADDING || point.y >= box.y + box.height + ROUTE_PADDING))
  }
  const index = domain.lanes.get(edgeId) ?? 0
  const bottom = domain.bounds.y + domain.bounds.height + 36 + index * ROUTE_CHANNEL_GAP
  const lane = bottom
  const exitX = source.x + 24 + (index % 3) * ROUTE_CHANNEL_GAP
  const entryX = target.x - 24 - (index % 3) * ROUTE_CHANNEL_GAP
  return [{ x: exitX, y: source.y }, { x: exitX, y: lane }, { x: entryX, y: lane }, { x: entryX, y: target.y }]
}
