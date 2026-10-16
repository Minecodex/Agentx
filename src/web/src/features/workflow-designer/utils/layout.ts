import type { NodeManifest, StudioDocument, StudioEdge, StudioNode } from '../model/types'
import { canvasBranchPorts } from '../nodes/manifest-node'
import { canvasNodeMetrics, canvasNodeRole } from '../nodes/node-appearance'

export const LARGE_GRAPH_LAYOUT_THRESHOLD = 250
export const LAYOUT_LAYER_GAP = 160
export const LAYOUT_NODE_GAP = 80

/** Loop container geometry (demo2 .container): resizer floor and inner padding around the body sub-DAG. */
export const LOOP_CONTAINER_MIN_WIDTH = 360
export const LOOP_CONTAINER_MIN_HEIGHT = 160
export const LOOP_CONTAINER_DEFAULT_WIDTH = 470
export const LOOP_CONTAINER_DEFAULT_HEIGHT = 300
export const LOOP_CONTAINER_PADDING = { top: 116, right: 24, bottom: 48, left: 24 }
/** Iteration start chip anchor inside the container, in parent-relative coordinates. */
export const LOOP_CHIP_POSITION = { x: 16, y: 56 }
/** Iteration end chip shares the header row and is anchored against the right edge. */
export const LOOP_END_CHIP_WIDTH = 140
export const LOOP_END_CHIP_HEIGHT = 64
export const loopEndChipPosition = (containerWidth: number) => ({ x: Math.max(LOOP_CHIP_POSITION.x + 180, containerWidth - LOOP_END_CHIP_WIDTH - 16), y: 56 })

export async function autoLayout(nodes: StudioNode[], edges: StudioEdge[], manifests?: Map<string, NodeManifest>) {
  if (nodes.length >= LARGE_GRAPH_LAYOUT_THRESHOLD) return largeGraphLayout(nodes, edges, manifests)
  const { roots, childrenByLoop } = partitionContainers(nodes)
  const layouted = new Map(nodes.map((node) => [node.id, { ...node }]))
  // Containers are sub-canvases: lay out the body first, size the frame from the content.
  for (const [loopId, children] of childrenByLoop) {
    const body = await layoutWithElk(children, innerEdges(edges, children), manifests, { x: LOOP_CONTAINER_PADDING.left, y: LOOP_CONTAINER_PADDING.top })
    for (const node of body) layouted.set(node.id, node)
    const loop = layouted.get(loopId)
    if (loop) layouted.set(loopId, { ...loop, width: frameWidth(body, manifests), height: frameHeight(body, manifests) })
  }
  const outerEdges = edges.filter((edge) => !isChild(childrenByLoop, edge.source) && !isChild(childrenByLoop, edge.target))
  const outerSizes = new Map([...childrenByLoop].map(([loopId, children]) => [loopId, frameSize(layouted, loopId, children, manifests)]))
  for (const node of await layoutWithElk(roots, outerEdges, manifests, { x: 0, y: 0 }, outerSizes)) {
    const sized = layouted.get(node.id)
    layouted.set(node.id, sized ? { ...sized, position: node.position } : node)
  }
  return nodes.map((node) => layouted.get(node.id) ?? node)
}

export function largeGraphLayout(nodes: StudioNode[], edges: StudioEdge[], manifests?: Map<string, NodeManifest>) {
  const { roots, childrenByLoop } = partitionContainers(nodes)
  const layouted = new Map(nodes.map((node) => [node.id, { ...node }]))
  for (const [loopId, children] of childrenByLoop) {
    bucketLayout(children, innerEdges(edges, children), manifests, { x: LOOP_CONTAINER_PADDING.left, y: LOOP_CONTAINER_PADDING.top })
    for (const node of children) layouted.set(node.id, node)
    const loop = layouted.get(loopId)
    if (loop) layouted.set(loopId, { ...loop, width: frameWidth(children, manifests), height: frameHeight(children, manifests) })
  }
  const outerEdges = edges.filter((edge) => !isChild(childrenByLoop, edge.source) && !isChild(childrenByLoop, edge.target))
  const outerSizes = new Map([...childrenByLoop].map(([loopId, children]) => [loopId, frameSize(layouted, loopId, children, manifests)]))
  bucketLayout(roots, outerEdges, manifests, { x: 0, y: 0 }, outerSizes)
  for (const node of roots) {
    const sized = layouted.get(node.id)
    layouted.set(node.id, sized ? { ...sized, position: node.position } : node)
  }
  return nodes.map((node) => layouted.get(node.id) ?? node)
}

type ContainerPartition = { roots: StudioNode[]; childrenByLoop: Map<string, StudioNode[]> }

function partitionContainers(nodes: StudioNode[]): ContainerPartition {
  const loops = new Set(nodes
    .filter((node) => node.data.editorKind === 'action' && node.data.nodeType === 'loop_over_items')
    .map((node) => node.id))
  const childrenByLoop = new Map<string, StudioNode[]>()
  const roots: StudioNode[] = []
  for (const source of nodes) {
    const node = { ...source, position: { ...source.position } }
    const parentId = node.data.editorKind === 'action' && node.data.parentId && loops.has(node.data.parentId) ? node.data.parentId : undefined
    if (parentId && parentId !== node.id) {
      const siblings = childrenByLoop.get(parentId)
      if (siblings) siblings.push(node)
      else childrenByLoop.set(parentId, [node])
    } else roots.push(node)
  }
  return { roots, childrenByLoop }
}

const isChild = (childrenByLoop: Map<string, StudioNode[]>, nodeId: string) => [...childrenByLoop.values()].some((children) => children.some((child) => child.id === nodeId))

function innerEdges(edges: StudioEdge[], children: StudioNode[]) {
  const ids = new Set(children.map((child) => child.id))
  return edges.filter((edge) => ids.has(edge.source) && ids.has(edge.target))
}

function nodeMetricsOf(node: StudioNode | undefined, manifests?: Map<string, NodeManifest>) {
  if (!node) return { width: 240, height: 120 }
  if (node.data.editorKind === 'exit') return { width: 240, height: 62 }
  const manifest = manifests?.get(`${node.data.nodeType}@${node.data.typeVersion}`)
  const metrics = canvasNodeMetrics(canvasNodeRole(manifest), {
    richHeight: node.height ?? node.measured?.height,
    ...canvasBranchPorts(manifest, node.data.parameters),
  })
  return { width: node.width ?? node.measured?.width ?? metrics.width, height: node.height ?? node.measured?.height ?? metrics.height }
}

async function layoutWithElk(nodes: StudioNode[], edges: StudioEdge[], manifests: Map<string, NodeManifest> | undefined, origin: { x: number; y: number }, sizes?: Map<string, { width: number; height: number }>) {
  const { default: ELK } = await import('elkjs/lib/elk.bundled.js')
  const elk = new ELK()
  const graph = await elk.layout({
    id: 'root',
    layoutOptions: { 'elk.algorithm': 'layered', 'elk.direction': 'RIGHT', 'elk.spacing.nodeNode': String(LAYOUT_NODE_GAP), 'elk.spacing.componentComponent': String(LAYOUT_LAYER_GAP), 'elk.layered.spacing.nodeNodeBetweenLayers': String(LAYOUT_LAYER_GAP) },
    children: nodes.map((node) => {
      const override = sizes?.get(node.id)
      const metrics = override ?? nodeMetricsOf(node, manifests)
      return { id: node.id, width: metrics.width, height: metrics.height }
    }),
    edges: edges.filter(edge => nodes.some(node => node.id === edge.source) && nodes.some(node => node.id === edge.target)).map((edge) => ({ id: edge.id, sources: [edge.source], targets: [edge.target] })),
  })
  const positions = new Map(graph.children?.map((node) => [node.id, { x: node.x ?? 0, y: node.y ?? 0 }]))
  return nodes.map((node) => ({ ...node, position: { x: (positions.get(node.id)?.x ?? node.position.x) + origin.x, y: (positions.get(node.id)?.y ?? node.position.y) + origin.y } }))
}

function bucketLayout(nodes: StudioNode[], edges: StudioEdge[], manifests: Map<string, NodeManifest> | undefined, origin: { x: number; y: number }, sizes?: Map<string, { width: number; height: number }>) {
  const byId = new Map(nodes.map((node) => [node.id, node]))
  const ids = new Set(nodes.map((node) => node.id))
  const outgoing = new Map<string, string[]>()
  const indegree = new Map(nodes.map((node) => [node.id, 0]))
  const levels = new Map(nodes.map((node) => [node.id, 0]))
  for (const edge of edges) {
    if (!ids.has(edge.source) || !ids.has(edge.target)) continue
    const targets = outgoing.get(edge.source)
    if (targets) targets.push(edge.target)
    else outgoing.set(edge.source, [edge.target])
    indegree.set(edge.target, (indegree.get(edge.target) ?? 0) + 1)
  }
  const queue = nodes.filter((node) => indegree.get(node.id) === 0).map((node) => node.id)
  for (let cursor = 0; cursor < queue.length; cursor++) {
    const source = queue[cursor]
    for (const target of outgoing.get(source) ?? []) {
      levels.set(target, Math.max(levels.get(target) ?? 0, (levels.get(source) ?? 0) + 1))
      const remaining = (indegree.get(target) ?? 1) - 1
      indegree.set(target, remaining)
      if (remaining === 0) queue.push(target)
    }
  }
  const buckets = new Map<number, string[]>()
  for (const node of nodes) {
    const level = levels.get(node.id) ?? 0
    const bucket = buckets.get(level)
    if (bucket) bucket.push(node.id)
    else buckets.set(level, [node.id])
  }
  let x = origin.x + 80
  for (const [, bucketIds] of [...buckets].sort(([left], [right]) => left - right)) {
    let y = origin.y + 60
    let width = 0
    for (const id of bucketIds) {
      const node = byId.get(id)!
      const metrics = sizes?.get(id) ?? nodeMetricsOf(node, manifests)
      node.position = { x, y }
      y += metrics.height + LAYOUT_NODE_GAP
      width = Math.max(width, metrics.width)
    }
    x += width + LAYOUT_LAYER_GAP
  }
}

const frameWidth = (body: StudioNode[], manifests?: Map<string, NodeManifest>) => Math.max(
  LOOP_CONTAINER_MIN_WIDTH,
  ...body.map((node) => node.position.x + nodeMetricsOf(node, manifests).width + LOOP_CONTAINER_PADDING.right),
)
const frameHeight = (body: StudioNode[], manifests?: Map<string, NodeManifest>) => Math.max(
  LOOP_CONTAINER_MIN_HEIGHT,
  ...body.map((node) => node.position.y + nodeMetricsOf(node, manifests).height + LOOP_CONTAINER_PADDING.bottom),
)

/** Container frame size after a body layout, reusing measured or persisted overrides. */
function frameSize(layouted: Map<string, StudioNode>, loopId: string, children: StudioNode[], manifests?: Map<string, NodeManifest>) {
  const loop = layouted.get(loopId)
  const body = children.map(node => layouted.get(node.id) ?? node)
  return { width: Math.max(loop?.width ?? 0, frameWidth(body, manifests)), height: Math.max(loop?.height ?? 0, frameHeight(body, manifests)) }
}

/** Start participates in the same layout as real nodes; it is never serialized as a business node. */
export async function layoutStudioDocument(document: StudioDocument, manifests?: Map<string, NodeManifest>): Promise<StudioDocument> {
  const position = document.boundaryLayouts.find(layout => layout.boundary === 'start') ?? { x: 40, y: 220 }
  const start: StudioNode = { id: '__start__', type: 'exit', position: { x: position.x, y: position.y }, width: 240, height: 62,
    data: { editorKind: 'exit', key: '__start__', label: '', protected: true, parameters: { outputs: {}, errorOutputs: {} } } }
  const layouted = await autoLayout([start, ...document.nodes], document.edges, manifests)
  const anchor = layouted.find(node => node.id === start.id)!.position
  const offset = { x: position.x - anchor.x, y: position.y - anchor.y }
  return { ...document,
    nodes: layouted.filter(node => node.id !== start.id).map(node => ({ ...node, position: node.data.editorKind === 'action' && node.data.parentId ? node.position : { x: node.position.x + offset.x, y: node.position.y + offset.y } })),
    boundaryLayouts: [{ boundary: 'start', x: position.x, y: position.y }],
  }
}
