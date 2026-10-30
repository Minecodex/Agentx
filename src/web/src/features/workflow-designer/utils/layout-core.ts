import { ROUTE_CHANNEL_GAP } from '../edges/canvas-route'
import { computeGroupView } from './group-geometry'
import { proxyEdges } from './group-edges'
import { canvasEdgePortKinds } from './edge-port-kinds'
import type { ELK as ElkEngine, ElkNode } from 'elkjs/lib/elk-api'
import type { NodeManifest, StudioDocument, StudioEdge, StudioNode } from '../model/types'
import { groupGeometry, studioNodeGeometry, type CanvasNodeGeometry } from '../nodes/canvas-node-geometry'
import { manifestForNode } from './manifest-lookup'
import { errorOnlyNodes } from './error-lanes'
import { LAYOUT_LAYER_GAP, LAYOUT_NODE_GAP, LOOP_CONTAINER_MIN_HEIGHT, LOOP_CONTAINER_MIN_WIDTH, LOOP_CONTAINER_PADDING } from './layout-constants'

const portId = (nodeId: string, direction: string, handle: string) => `${nodeId}:${direction}:${handle}`

export function layoutNodeGeometry(node: StudioNode, manifests?: Map<string, NodeManifest>): CanvasNodeGeometry {
  return studioNodeGeometry(node, manifests ? manifestForNode(manifests, node) : undefined)
}

export async function autoLayout(nodes: StudioNode[], edges: StudioEdge[], manifests: Map<string, NodeManifest> | undefined, elk: ElkEngine, overrides = new Map<string, CanvasNodeGeometry>()): Promise<StudioNode[]> {
  const geometry = (node: StudioNode) => overrides.get(node.id) ?? layoutNodeGeometry(node, manifests)
  const loops = new Map<string, StudioNode[]>()
  for (const node of nodes) if (node.data.editorKind === 'action' && node.data.nodeType === 'loop_over_items') loops.set(node.id, [])
  const roots: StudioNode[] = []
  for (const node of nodes) {
    const parent = node.data.editorKind === 'action' && node.data.parentId ? loops.get(node.data.parentId) : undefined
    if (parent) parent.push(node)
    else roots.push(node)
  }
  const result = new Map<string, StudioNode>()
  for (const [loopId, children] of loops) {
    const ids = new Set(children.map(child => child.id))
    const body = await layoutDomain(children, edges.filter(edge => ids.has(edge.source) && ids.has(edge.target)), manifests, LOOP_CONTAINER_PADDING, overrides, elk)
    for (const child of body) result.set(child.id, child)
    const loop = roots.find(node => node.id === loopId)!
    const width = Math.max(LOOP_CONTAINER_MIN_WIDTH, ...body.map(node => node.position.x + geometry(node).width + LOOP_CONTAINER_PADDING.right))
    const errorPorts = body.reduce((count, node) => count + geometry(node).ports.filter(port => port.direction === 'output' && port.kind === 'error').length, 0)
    const height = Math.max(LOOP_CONTAINER_MIN_HEIGHT, ...body.map(node => node.position.y + geometry(node).height + Math.max(LOOP_CONTAINER_PADDING.bottom, 64 + errorPorts * ROUTE_CHANNEL_GAP)))
    result.set(loopId, { ...loop, width, height })
  }
  const rootIds = new Set(roots.map(node => node.id))
  const rootNodes = roots.map(node => result.get(node.id) ?? node)
  for (const node of await layoutDomain(rootNodes, edges.filter(edge => rootIds.has(edge.source) && rootIds.has(edge.target)), manifests, { left: 0, top: 0 }, overrides, elk)) result.set(node.id, node)
  return nodes.map(node => result.get(node.id) ?? node)
}

async function layoutDomain(nodes: StudioNode[], edges: StudioEdge[], manifests: Map<string, NodeManifest> | undefined, origin: { left: number; top: number }, overrides: Map<string, CanvasNodeGeometry>, elk: ElkEngine): Promise<StudioNode[]> {
  if (!nodes.length) return []
  const geometries = new Map(nodes.map(node => [node.id, overrides.get(node.id) ?? layoutNodeGeometry(node, manifests)]))
  const isError = (edge: StudioEdge) => geometries.get(edge.source)?.ports.some(port => port.id === edge.sourceHandle && port.direction === 'output' && port.kind === 'error') || edge.sourceHandle === 'error'
  const incoming = new Map<string, StudioEdge[]>()
  for (const edge of edges) incoming.set(edge.target, [...(incoming.get(edge.target) ?? []), edge])
  const localErrorExits = new Set(nodes.filter(node => node.data.editorKind === 'exit' && incoming.get(node.id)?.length === 1 && isError(incoming.get(node.id)![0])).map(node => node.id))
  // Feed ELK the same branch order as the card, independent of creation order.
  const outgoing = new Map<string, StudioEdge[]>()
  for (const edge of edges) outgoing.set(edge.source, [...(outgoing.get(edge.source) ?? []), edge])
  for (const values of outgoing.values()) values.sort((a, b) => {
    const rank = (edge: StudioEdge) => { const port = geometries.get(edge.source)?.ports.find(port => port.direction === 'output' && port.id === edge.sourceHandle); return port?.kind === 'error' && !localErrorExits.has(edge.target) ? Infinity : port?.y ?? 24 }
    return rank(a) - rank(b) || (a.data?.order ?? 0) - (b.data?.order ?? 0)
  })
  const targets = new Set(edges.map(edge => edge.target))
  const queue = nodes.filter(node => !targets.has(node.id)).map(node => node.id)
  const byId = new Map(nodes.map(node => [node.id, node]))
  const ordered = new Map<string, StudioNode>()
  for (let i = 0; i < queue.length; i++) { const id = queue[i]; if (ordered.has(id) || !byId.has(id)) continue; ordered.set(id, byId.get(id)!); for (const edge of outgoing.get(id) ?? []) queue.push(edge.target) }
  for (const node of nodes) if (!ordered.has(node.id)) ordered.set(node.id, node)
  const children: ElkNode[] = [...ordered.values()].map(node => {
    const geometry = geometries.get(node.id)!
    return { id: node.id, width: geometry.width, height: geometry.height, layoutOptions: { 'elk.portConstraints': 'FIXED_POS' }, ports: geometry.ports.map(port => ({ id: portId(node.id, port.direction, port.id), x: port.x, y: port.y, width: 0, height: 0, layoutOptions: { 'elk.port.side': port.side === 'left' ? 'WEST' : 'EAST' } })) }
  })
  const graph = await elk.layout({ id: 'canvas', layoutOptions: {
    'elk.algorithm': 'layered', 'elk.direction': 'RIGHT', 'elk.edgeRouting': 'ORTHOGONAL',
    'elk.padding': '[top=0,left=0,bottom=0,right=0]', 'elk.spacing.nodeNode': String(LAYOUT_NODE_GAP),
    'elk.spacing.componentComponent': String(LAYOUT_NODE_GAP), 'elk.layered.spacing.nodeNodeBetweenLayers': String(LAYOUT_LAYER_GAP),
    'elk.layered.spacing.edgeNodeBetweenLayers': '24', 'elk.layered.spacing.edgeEdgeBetweenLayers': '12',
    'elk.layered.considerModelOrder.strategy': 'NODES_AND_EDGES', 'elk.layered.crossingMinimization.forceNodeModelOrder': 'true',
    'elk.layered.nodePlacement.strategy': 'NETWORK_SIMPLEX', 'elk.layered.nodePlacement.favorStraightEdges': 'true',
  }, children, edges: edges.map(edge => {
    const source = geometries.get(edge.source)?.ports.find(port => port.direction === 'output' && port.id === (edge.sourceHandle ?? 'main'))
    const target = geometries.get(edge.target)?.ports.find(port => port.direction === 'input' && port.id === (edge.targetHandle ?? 'main'))
    return { id: edge.id, sources: [source ? portId(edge.source, 'output', source.id) : edge.source], targets: [target ? portId(edge.target, 'input', target.id) : edge.target], layoutOptions: { 'elk.layered.priority.straightness': source?.kind === 'error' ? '0' : '10' } }
  }) })
  const positions = new Map(graph.children?.map(node => [node.id, { x: node.x ?? 0, y: node.y ?? 0 }]))
  // Keep a dedicated terminal next to its source where ELK placed it. Only
  // multi-node/shared exceptional branches need the separate lower region.
  const errorOnly = new Set([...errorOnlyNodes(nodes.map(node => node.id), edges, isError)].filter(id => !localErrorExits.has(id)))
  const normalBottom = Math.max(0, ...nodes.filter(node => !errorOnly.has(node.id)).map(node => positions.get(node.id)!.y + geometries.get(node.id)!.height))
  const errorTop = Math.min(Infinity, ...nodes.filter(node => errorOnly.has(node.id)).map(node => positions.get(node.id)!.y))
  const errorOffset = Math.max(0, normalBottom + LAYOUT_NODE_GAP - errorTop)
  return nodes.map(node => ({ ...node, position: { x: positions.get(node.id)!.x + origin.left, y: positions.get(node.id)!.y + origin.top + (errorOnly.has(node.id) ? errorOffset : 0) } }))
}

export async function layoutStudioDocumentCore(document: StudioDocument, manifests: Map<string, NodeManifest> | undefined, elk: ElkEngine): Promise<StudioDocument> {
  const position = document.boundaryLayouts.find(layout => layout.boundary === 'start') ?? { x: 40, y: 220 }
  const start: StudioNode = { id: '__start__', type: 'exit', position: { x: position.x, y: position.y }, data: { editorKind: 'exit', key: '__start__', label: '', protected: true, parameters: { outputs: {}, errorOutputs: {} } } }
  const byId = new Map(document.nodes.map(node => [node.id, node]))
  const proxies = document.groups.filter(group => group.collapsed).flatMap(group => {
    const view = computeGroupView(group, byId, manifests)
    return view ? [view] : []
  })
  const membership = new Map(proxies.flatMap(view => view.group.nodeIds.map(id => [id, view.id] as const)))
  for (const node of document.nodes) if (node.data.editorKind === 'action' && node.data.parentId && membership.has(node.data.parentId)) membership.set(node.id, membership.get(node.data.parentId)!)
  const proxyNodes: StudioNode[] = proxies.map(view => ({
    id: view.id, type: 'manifest', position: view.position,
    data: { editorKind: 'action', nodeType: '__group__', typeVersion: 1, label: view.group.label, key: view.id, parentId: view.parentId, parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false },
  }))
  const visible = [start, ...document.nodes.filter(node => !membership.has(node.id)), ...proxyNodes]
  const overrides = new Map(proxies.map(view => [view.id, groupGeometry()]))
  const layouted = await autoLayout(visible, proxyEdges(canvasEdgePortKinds(document.edges, document.nodes, manifests), membership), manifests, elk, overrides)
  const anchor = layouted.find(node => node.id === start.id)!.position
  const placed = new Map(layouted.map(node => [node.id, { ...node, position: node.data.editorKind === 'action' && node.data.parentId ? node.position : { x: node.position.x + position.x - anchor.x, y: node.position.y + position.y - anchor.y } }]))
  const deltas = new Map(proxies.map(view => {
    const at = placed.get(view.id)!.position
    return [view.id, { x: at.x - view.position.x, y: at.y - view.position.y }]
  }))
  const directMembership = new Map(proxies.flatMap(view => view.group.nodeIds.map(id => [id, view.id] as const)))
  const nodes = document.nodes.map(node => {
    const group = directMembership.get(node.id)
    const parent = node.data.editorKind === 'action' ? node.data.parentId : undefined
    if (group && (!parent || membership.get(parent) !== group)) {
      const delta = deltas.get(group)!
      return { ...node, position: { x: node.position.x + delta.x, y: node.position.y + delta.y } }
    }
    return placed.get(node.id) ?? node
  })
  return { ...document, nodes, boundaryLayouts: [{ boundary: 'start', x: position.x, y: position.y }] }
}
