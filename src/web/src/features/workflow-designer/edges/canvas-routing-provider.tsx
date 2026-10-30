import { SmartEdgeProvider } from '@tisoap/react-flow-smart-edge'
import type { Node } from '@xyflow/react'
import { useCallback, useMemo, useRef, useState, type ReactNode } from 'react'
import type { CanvasNode, StudioEdge } from '../model/types'
import { canvasGeometry } from '../nodes/canvas-node-geometry'
import { useCanvasRenderStore } from '../store/canvas-render-store'
import { ROUTE_PADDING } from './canvas-route'
import { RoutingContext } from './routing-context'
import { routingWorld } from './routing-world'

const OPTIONS = { preset: 'smoothstep' as const, nodePadding: ROUTE_PADDING, gridRatio: 8, borderRadius: 12, routeWhileDragging: false, debounceMs: 16, routeOnlyWhenBlocked: false, cacheSize: 2048 }

export function CanvasRoutingProvider({ nodes, edges, children }: { nodes: CanvasNode[]; edges: StudioEdge[]; children: ReactNode }) {
  const manifests = useCanvasRenderStore(state => state.manifests)
  const quickAddEnabled = useCanvasRenderStore(state => Boolean(state.onQuickAdd))
  const world = useMemo(() => routingWorld(nodes, edges, manifests, quickAddEnabled), [nodes, edges, manifests, quickAddEnabled])
  const [completed, setCompleted] = useState({ signature: '', batches: 0 })
  const onMetrics = useCallback(() => { if (!world.dragging) setCompleted(previous => ({ signature: world.signature, batches: previous.batches + 1 })) }, [world.dragging, world.signature])
  const settledNodes = useRef<Node[]>([])
  const routingNodes = useMemo<Node[]>(() => nodes.filter(node => node.data.editorKind !== 'group' || node.data.collapsed).map(node => ({ ...node, measured: { width: node.width ?? node.measured?.width ?? (Number(node.style?.width) || canvasGeometry(node.data).width), height: node.height ?? node.measured?.height ?? (Number(node.style?.height) || canvasGeometry(node.data).height) } })), [nodes])
  const obstacles = useMemo(() => {
    if (!world.dragging) { settledNodes.current = routingNodes; return routingNodes }
    const previous = new Map(settledNodes.current.map(node => [node.id, node]))
    return routingNodes.map(node => ({ ...(previous.get(node.id) ?? node), dragging: node.dragging, selected: node.selected }))
  }, [routingNodes, world.dragging])
  const value = useMemo(() => ({ ...world, complete: completed.signature === world.signature, batches: completed.batches }), [world, completed])
  return <RoutingContext.Provider value={value}><SmartEdgeProvider nodes={obstacles} options={OPTIONS} onMetrics={onMetrics}>{children}</SmartEdgeProvider></RoutingContext.Provider>
}
