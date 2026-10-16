import { Background, BackgroundVariant, Controls, MarkerType, MiniMap, ReactFlow, type AriaLabelConfig, type Connection, type NodeChange, type ReactFlowInstance } from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import { forwardRef, useCallback, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState, type DragEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { useShallow } from 'zustand/react/shallow'

import type { CanvasNode, ConnectionInteractionState, EditorDocument, NodeManifest, StudioEdge, StudioNode } from '../model/types'
import { StudioEdgeComponent } from '../edges/studio-edge'
import { canvasBranchPorts } from '../nodes/manifest-node'
import { canvasNodeMetrics, canvasNodeRole } from '../nodes/node-appearance'
import { canvasZoomTier, useCanvasRenderStoreApi } from '../store/canvas-render-store'
import { useEditorStore, useEditorStoreApi } from '../store/editor-store'
import { localizedNodeLabel } from '../model/manifest-localization'
import { inspectConnection, isIterationEndId, isIterationChipId, iterationChipId, iterationEndId, normalizeIterationChip } from '../utils/connections'
import { createGraphIndex, IncrementalGraphIndex, occupiedHandlesByNodeId, portKey } from '../utils/graph-index'
import { proxyEdges } from '../utils/group-edges'
import { manifestForNode } from '../utils/manifest-lookup'
import { LOOP_CHIP_POSITION, LOOP_CONTAINER_DEFAULT_HEIGHT, LOOP_CONTAINER_DEFAULT_WIDTH, LOOP_END_CHIP_HEIGHT, loopEndChipPosition } from '../utils/layout'
import { CANVAS_NODE_TYPES } from './canvas-node-packages'

export const LARGE_GRAPH_RENDER_THRESHOLD = 150
export const LARGE_GRAPH_MINIMAP_THRESHOLD = 300

/** Slack a parented child can push past the container edge while dragging (extent, parent-relative). */
const LOOP_CHILD_EXTENT_MARGIN = 80
/** Dragging a child further than this outside its container detaches it from the loop. */
export const LOOP_CHILD_DETACH_OVERFLOW = 40

type FlowProps = {
  manifests: Map<string, NodeManifest>
  readOnly?: boolean
  runtimeStatuses: Map<string, string>
  onDropAction: (manifest: NodeManifest, position: { x: number; y: number }, parentId?: string) => void
  onNodeOpen?: (nodeId: string) => void
  onBoundaryOpen?: (boundary: 'start') => void
  onPaneClear?: () => void
  onQuickAdd?: (nodeId: string, handleId: string, manifestPortName?: string) => void
}

export type WorkflowFlowHandle = {
  getViewportCenter: () => { x: number; y: number } | undefined
  getViewportBounds: () => { x: number; y: number; width: number; height: number } | undefined
}

const EDGE_TYPES = { studio: StudioEdgeComponent }

export const WorkflowFlow = forwardRef<WorkflowFlowHandle, FlowProps>(function WorkflowFlow({ manifests, runtimeStatuses, readOnly = false, onDropAction, onNodeOpen, onBoundaryOpen, onPaneClear, onQuickAdd }, ref) {
  const storeApi = useEditorStoreApi()
  const { t, i18n } = useTranslation()
  const instance = useRef<ReactFlowInstance<CanvasNode, StudioEdge>>()
  const [connectionState, setConnectionState] = useState<ConnectionInteractionState>({ status: 'idle' })
  const editor = useEditorStore(useShallow((state) => ({
    nodes: state.nodes, edges: state.edges, annotations: state.annotations, groups: state.groups, viewport: state.viewport, boundaryLayouts: state.boundaryLayouts,
    graphRevision: state.graphRevision,
    onEdgesChange: state.onEdgesChange, connect: state.connect, select: state.select, setViewport: state.setViewport,
    reconnectEdge: state.reconnectEdge, edgeReconnectRequest: state.edgeReconnectRequest, clearEdgeReconnectRequest: state.clearEdgeReconnectRequest,
    beginEdit: state.beginEdit, commitEdit: state.commitEdit, onNodesChange: state.onNodesChange,
    updateAnnotationFrame: state.updateAnnotationFrame, removeAnnotation: state.removeAnnotation, updateAnnotation: state.updateAnnotation,
    moveGroup: state.moveGroup, removeGroup: state.removeGroup, toggleGroup: state.toggleGroup, updateBoundaryPosition: state.updateBoundaryPosition,
    detachFromContainer: state.detachFromContainer,
  })))
  const { nodes, edges, annotations, groups, viewport, boundaryLayouts, onEdgesChange, connect, select, setViewport, commitEdit } = editor
  const annotationActions = useMemo(() => ({
    updateAnnotation: editor.updateAnnotation,
    removeAnnotation: editor.removeAnnotation,
    beginEdit: editor.beginEdit,
    commitEdit: editor.commitEdit,
    updateAnnotationFrame: editor.updateAnnotationFrame,
  }), [editor.beginEdit, editor.commitEdit, editor.removeAnnotation, editor.updateAnnotation, editor.updateAnnotationFrame])
  const groupActions = useMemo(() => ({ toggleGroup: editor.toggleGroup, removeGroup: editor.removeGroup }), [editor.removeGroup, editor.toggleGroup])
  const groupViewIndex = useRef(new GroupViewIndex())
  const groupViews = useMemo(() => groupViewIndex.current.sync(nodes, groups, manifests), [groups, manifests, nodes])
  const collapsedByMember = useMemo(() => {
    const result = new Map<string, string>()
    for (const group of groups) if (group.collapsed) for (const nodeId of group.nodeIds) result.set(nodeId, `group:${group.id}`)
    return result
  }, [groups])
  const containers = useMemo(() => indexContainers(nodes), [nodes])
  const renderedNodeCache = useRef(new Map<string, { source: StudioNode; manifest?: NodeManifest; frame: string; value: CanvasNode }>())
  const flowNodes = useMemo<CanvasNode[]>(() => [
    ...materializeNodes(nodes.filter((node) => !collapsedByMember.has(node.id)), edges, manifests, containers, renderedNodeCache.current, i18n.language),
    ...boundaryNodes(boundaryLayouts),
    ...annotations.map((annotation) => annotationNode(annotation, annotationActions)),
    ...groupViews.map((view) => groupNode(view, groupActions)),
    ...iterationChipNodes(containers),
    ...iterationEndNodes(containers),
  ], [annotationActions, annotations, boundaryLayouts, collapsedByMember, containers, edges, groupActions, groupViews, i18n.language, manifests, nodes])
  const renderedEdgeCache = useRef(new Map<string, { source: StudioEdge; sourceStatus?: string; targetStatus?: string; value: StudioEdge }>())
  const flowEdges = useMemo(() => materializeRuntimeEdges(proxyEdges(edges, collapsedByMember), runtimeStatuses, renderedEdgeCache.current), [collapsedByMember, edges, runtimeStatuses])
  const groupViewMap = useMemo(() => new Map(groupViews.map((view) => [view.id, view])), [groupViews])
  const graphIndexController = useRef(new IncrementalGraphIndex())
  const graphIndex = useMemo(() => graphIndexController.current.sync(storeApi.getState().nodes, storeApi.getState().edges, manifests), [editor.graphRevision, manifests])
  const occupiedHandlesCache = useRef<{ graphRevision: number; value: Map<string, string> }>()
  if (occupiedHandlesCache.current?.graphRevision !== editor.graphRevision) occupiedHandlesCache.current = { graphRevision: editor.graphRevision, value: occupiedHandlesByNodeId(graphIndex) }
  const occupiedHandles = occupiedHandlesCache.current.value
  const onSourceHover = useCallback((nodeId: string, handleId: string, active: boolean) => setConnectionState((current) => {
    if (active && current.status === 'idle') return { status: 'source-hover', nodeId, handleId }
    if (!active && current.status === 'source-hover' && current.nodeId === nodeId && current.handleId === handleId) return { status: 'idle' }
    return current
  }), [])
  const canvasRenderStore = useCanvasRenderStoreApi()
  useLayoutEffect(() => canvasRenderStore.setState({
    manifests, runtimeStatuses, bindingSummaries: graphIndex.bindingSummaryByNodeId, occupiedHandlesByNodeId: occupiedHandles,
    zoomTier: canvasZoomTier(viewport.zoom),
    onQuickAdd,
    onSourceHover,
  }), [canvasRenderStore, editor.graphRevision, graphIndex, manifests, occupiedHandles, onQuickAdd, onSourceHover, runtimeStatuses, viewport.zoom])
  useImperativeHandle(ref, () => ({
    getViewportCenter: () => {
      const flow = instance.current
      const element = document.querySelector<HTMLElement>('[data-testid="workflow-canvas"]')
      if (!flow || !element) return undefined
      const bounds = element.getBoundingClientRect()
      return flow.screenToFlowPosition({ x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 })
    },
    getViewportBounds: () => {
      const flow = instance.current
      const element = document.querySelector<HTMLElement>('[data-testid="workflow-canvas"]')
      if (!flow || !element) return undefined
      const bounds = element.getBoundingClientRect()
      const topLeft = flow.screenToFlowPosition({ x: bounds.left, y: bounds.top })
      const bottomRight = flow.screenToFlowPosition({ x: bounds.right, y: bounds.bottom })
      return { x: topLeft.x, y: topLeft.y, width: bottomRight.x - topLeft.x, height: bottomRight.y - topLeft.y }
    },
  }), [])
  const valid = useCallback((connection: Connection | StudioEdge) => {
    const validation = inspectConnection(connection, graphIndex)
    setConnectionState(validation.status === 'invalid'
      ? { status: 'incompatible', reason: validation.reason }
      : validation.status === 'occupied' ? { status: 'occupied' } : { status: 'compatible' })
    return validation.status !== 'invalid'
  }, [graphIndex])
  const onConnect = useCallback((rawConnection: Connection) => {
    if (readOnly) return
    if (isIterationChipId(rawConnection.source) || isIterationEndId(rawConnection.target)) {
      if (inspectConnection(rawConnection, graphIndex).status === 'invalid') return
      setConnectionState({ status: 'committed' })
      return
    }
    const connection = normalizeIterationChip(rawConnection)
    const validation = inspectConnection(connection, graphIndex)
    if (validation.status === 'invalid') return
    const source = graphIndex.nodeById.get(connection.source ?? '')
    const sourceManifest = source ? manifestForNode(manifests, source) : undefined
    const sourcePortKind = sourceManifest?.outputPorts.find((port) => port.name === connection.sourceHandle)?.kind
    const order = graphIndex.edgesBySourcePort.get(portKey(connection.source ?? '', connection.sourceHandle))?.length ?? 0
    connect(connection, { edgeKind: 'execution', order, sourcePortKind }, validation.replaceEdge?.id)
    setConnectionState({ status: 'committed' })
  }, [connect, graphIndex, manifests, readOnly])
  const onNodesChange = useCallback((changes: NodeChange<CanvasNode>[]) => {
    const canonical: NodeChange<CanvasNode>[] = []
    for (const change of changes) {
      if (!('id' in change)) continue
      if (change.id === '__start__') {
        if (change.type === 'position' && change.position) editor.updateBoundaryPosition('start', change.position)
        continue
      }
      if (change.id.startsWith('annotation:')) {
        const annotationId = change.id.slice('annotation:'.length)
        if (change.type === 'position' && change.position) editor.updateAnnotationFrame(annotationId, change.position)
        if (change.type === 'remove') editor.removeAnnotation(annotationId)
        continue
      }
      const group = groupViewMap.get(change.id)
      if (group) {
        if (change.type === 'position' && change.position) editor.moveGroup(group.group.id, { x: change.position.x - group.position.x, y: change.position.y - group.position.y })
        if (change.type === 'remove') editor.removeGroup(group.group.id)
        continue
      }
      if (isIterationChipId(change.id) || isIterationEndId(change.id)) continue
      canonical.push(change)
    }
    if (canonical.length) editor.onNodesChange(canonical as NodeChange<StudioNode>[])
  }, [editor, groupViewMap])
  const onReconnect = useCallback((edge: StudioEdge, connection: Connection) => {
    const reconnectIndex = createGraphIndex(nodes, edges.filter((candidate) => candidate.id !== edge.id), manifests)
    if (inspectConnection(connection, reconnectIndex).status === 'invalid') return
    editor.reconnectEdge(edge.id, connection)
    editor.clearEdgeReconnectRequest()
  }, [edges, editor, manifests, nodes])
  const onNodeDragStart = useCallback((_: unknown, node: CanvasNode) => {
    if (node.id === '__start__') return editor.beginEdit({ boundary: 'start' })
    if (node.id.startsWith('annotation:')) return editor.beginEdit({ annotationIds: [node.id.slice('annotation:'.length)] })
    const group = groupViewMap.get(node.id)
    if (group) return editor.beginEdit({ nodeIds: group.group.nodeIds })
    const nodeIds = nodes.filter((candidate) => candidate.selected || candidate.id === node.id).map((candidate) => candidate.id)
    editor.beginEdit({ nodeIds })
  }, [editor, groupViewMap, nodes])
  const onDrop = (event: DragEvent) => {
    event.preventDefault()
    if (readOnly) return
    const raw = event.dataTransfer.getData('application/agentx-studio')
    if (!raw || !instance.current) return
    const value = JSON.parse(raw) as { kind: string; nodeType?: string; version?: number }
    const position = instance.current.screenToFlowPosition({ x: event.clientX, y: event.clientY })
    if (value.kind !== 'action') return
    const manifest = manifests.get(`${value.nodeType}@${value.version}`)
    if (!manifest) return
    // Dropping into a container bounding box claims membership; loops never nest.
    const store = storeApi.getState()
    const parentId = manifest.nodeType === 'loop_over_items' ? undefined : findDropContainer(store.nodes, position)
    const parent = parentId ? store.nodes.find((node) => node.id === parentId) : undefined
    onDropAction(manifest, parent ? { x: position.x - parent.position.x, y: position.y - parent.position.y } : position, parentId)
  }
  const releaseEscapedChildren = useCallback((dragged: CanvasNode[]) => {
    const store = storeApi.getState()
    for (const node of dragged) {
      if (!node.parentId) continue
      const container = store.nodes.find((candidate) => candidate.id === node.parentId)
      if (!container) continue
      const overflow = containerEscapeOverflow(
        { position: node.position, width: node.measured?.width ?? node.width ?? 240, height: node.measured?.height ?? node.height ?? 120 },
        container,
      )
      if (overflow > LOOP_CHILD_DETACH_OVERFLOW) editor.detachFromContainer(node.id)
    }
  }, [editor])
  const onNodeDragStop = useCallback((_: unknown, node: CanvasNode, dragged?: CanvasNode[]) => {
    releaseEscapedChildren(dragged ?? [node])
    commitEdit()
  }, [commitEdit, releaseEscapedChildren])
  const ariaLabelConfig: Partial<AriaLabelConfig> = {
    'node.a11yDescription.default': t('studio.canvasA11y.node'),
    'node.a11yDescription.keyboardDisabled': t('studio.canvasA11y.nodeKeyboard'),
    'node.a11yDescription.ariaLiveMessage': ({ direction, x, y }) => t('studio.canvasA11y.nodeMoved', { direction, x, y }),
    'edge.a11yDescription.default': t('studio.canvasA11y.edge'),
    'controls.ariaLabel': t('studio.canvasA11y.controls'),
    'controls.zoomIn.ariaLabel': t('studio.canvasA11y.zoomIn'),
    'controls.zoomOut.ariaLabel': t('studio.canvasA11y.zoomOut'),
    'controls.fitView.ariaLabel': t('studio.canvasA11y.fitView'),
    'controls.interactive.ariaLabel': t('studio.canvasA11y.interactive'),
    'minimap.ariaLabel': t('studio.canvasA11y.minimap'),
    'handle.ariaLabel': t('studio.canvasA11y.handle'),
  }
  return <div className="relative min-w-0 flex-1 bg-canvas" data-canonical-edge-count={edges.length} data-connection-state={connectionState.status} data-testid="workflow-canvas" onDragOver={(event) => { event.preventDefault(); event.dataTransfer.dropEffect = 'copy' }} onDrop={onDrop}>
    <div className="pointer-events-none absolute right-3 top-3 z-10 flex items-center gap-3 rounded-md border border-border bg-surface/90 px-2.5 py-1.5 text-[9px] text-muted-foreground shadow-sm backdrop-blur"><LegendDot className="bg-primary" label={t('studio.ports.flow')} /><LegendDot className="bg-danger" label={t('studio.ports.error')} /><LegendDot className="bg-warning" label={t('studio.ports.resource')} /></div>
    <ReactFlow<CanvasNode, StudioEdge> ariaLabelConfig={ariaLabelConfig} connectionRadius={60} defaultViewport={viewport} edges={flowEdges} edgeTypes={EDGE_TYPES} edgesReconnectable={!readOnly} fitViewOptions={{ maxZoom: 1, padding: 0.2 }} nodes={flowNodes} nodesConnectable={!readOnly} nodesDraggable={!readOnly} nodeTypes={CANVAS_NODE_TYPES} onConnect={onConnect} onConnectEnd={() => { setConnectionState({ status: 'cancelled' }); window.requestAnimationFrame(() => setConnectionState({ status: 'idle' })) }} onConnectStart={(_, params) => setConnectionState({ status: 'connecting', nodeId: params.nodeId ?? '', handleId: params.handleId ?? undefined })} onEdgesChange={readOnly ? undefined : onEdgesChange} onInit={(value) => { instance.current = value }} onMoveEnd={(_, next) => { if (readOnly) canvasRenderStore.setState({zoomTier:canvasZoomTier(next.zoom)}); else setViewport(next) }} onNodeClick={(_, node) => { if (node.data.editorKind === 'boundary') { select(undefined); onBoundaryOpen?.(node.data.boundary) } else { select(node.id); if (node.data.editorKind === 'action' || node.data.editorKind === 'exit' || node.data.editorKind === 'loop-container') onNodeOpen?.(node.id) } }} onNodeDragStart={readOnly ? undefined : onNodeDragStart} onNodeDragStop={readOnly ? undefined : onNodeDragStop} onNodesChange={readOnly ? undefined : onNodesChange} onPaneClick={() => { select(undefined); onPaneClear?.() }} onReconnect={readOnly ? undefined : onReconnect} onReconnectEnd={() => editor.clearEdgeReconnectRequest()} isValidConnection={valid} multiSelectionKeyCode="Shift" onlyRenderVisibleElements={nodes.length >= LARGE_GRAPH_RENDER_THRESHOLD} proOptions={{ hideAttribution: true }} selectionOnDrag={!readOnly}>
      <Background color="var(--ui-canvas-dot)" gap={22} size={1} variant={BackgroundVariant.Dots} />
      <Controls aria-label={t('studio.canvasA11y.controls')} className="!border-border !bg-surface !shadow-md" />
      {nodes.length < LARGE_GRAPH_MINIMAP_THRESHOLD && <MiniMap ariaLabel={t('studio.canvasA11y.minimap')} className="!border !border-border !bg-surface" maskColor="color-mix(in srgb, var(--ui-background) 70%, transparent)" pannable zoomable />}
    </ReactFlow>
  </div>
})

function boundaryNodes(layouts: EditorDocument['boundaryLayouts']): CanvasNode[] {
  const start = layouts.find((layout) => layout.boundary === 'start') ?? { x: 40, y: 220 }
  return [
    { id: '__start__', type: 'boundary', position: { x: start.x, y: start.y }, draggable: true, deletable: false, data: { editorKind: 'boundary', boundary: 'start', label: '' } },
  ]
}

export type LoopContainerView = { loop: StudioNode; children: StudioNode[]; childIds: Set<string> }

const loopContainerWidth = (loop: StudioNode) => loop.width ?? loop.measured?.width ?? LOOP_CONTAINER_DEFAULT_WIDTH
const loopContainerHeight = (loop: StudioNode) => loop.height ?? loop.measured?.height ?? LOOP_CONTAINER_DEFAULT_HEIGHT

/** A loop_over_items node becomes a container once it owns body children (parentId membership). */
export function indexContainers(nodes: StudioNode[]): Map<string, LoopContainerView> {
  const loops = new Map<string, LoopContainerView>()
  for (const node of nodes)
    if (node.data.editorKind === 'action' && node.data.nodeType === 'loop_over_items')
      loops.set(node.id, { loop: node, children: [], childIds: new Set() })
  for (const node of nodes) {
    const parentId = node.data.editorKind === 'action' ? node.data.parentId : undefined
    const container = parentId && parentId !== node.id ? loops.get(parentId) : undefined
    if (container) {
      container.children.push(node)
      container.childIds.add(node.id)
    }
  }
  return loops
}

/** Palette drops inside a container bounding box claim membership for the new node. */
export function findDropContainer(nodes: StudioNode[], point: { x: number; y: number }): string | undefined {
  for (const { loop } of indexContainers(nodes).values()) {
    const width = loopContainerWidth(loop)
    const height = loopContainerHeight(loop)
    if (point.x >= loop.position.x && point.x <= loop.position.x + width && point.y >= loop.position.y && point.y <= loop.position.y + height) return loop.id
  }
  return undefined
}

/** How far a child rect (parent-relative position) sticks out of its container; 0 when fully inside. */
export function containerEscapeOverflow(child: { position: { x: number; y: number }; width: number; height: number }, container: StudioNode): number {
  const left = container.position.x + child.position.x
  const top = container.position.y + child.position.y
  return Math.max(
    container.position.x - left,
    left + child.width - (container.position.x + loopContainerWidth(container)),
    container.position.y - top,
    top + child.height - (container.position.y + loopContainerHeight(container)),
    0,
  )
}

function iterationChipNodes(containers: Map<string, LoopContainerView>): CanvasNode[] {
  return [...containers.values()].map(({ loop }) => ({
    id: iterationChipId(loop.id),
    type: 'iteration-chip' as const,
    parentId: loop.id,
    position: LOOP_CHIP_POSITION,
    draggable: false,
    selectable: false,
    deletable: false,
    zIndex: 3,
    initialWidth: 140,
    initialHeight: 40,
    style: { width: 140, height: 40, zIndex: 4 },
    data: { editorKind: 'iteration-chip' as const, loopId: loop.id },
  }))
}

function containerParallelism(loop: StudioNode) {
  const value = loop.data.editorKind === 'action' ? loop.data.parameters.parallelism : undefined
  return typeof value === 'number' && Number.isFinite(value) ? value : 1
}

function iterationEndNodes(containers: Map<string, LoopContainerView>): CanvasNode[] {
  return [...containers.values()].map(({ loop }) => ({
    id: iterationEndId(loop.id),
    type: 'iteration-end' as const,
    parentId: loop.id,
    position: loopEndChipPosition(loopContainerWidth(loop)),
    draggable: false,
    selectable: false,
    deletable: false,
    zIndex: 3,
    initialWidth: 140,
    initialHeight: LOOP_END_CHIP_HEIGHT,
    style: { width: 140, height: LOOP_END_CHIP_HEIGHT, zIndex: 4 },
    data: { editorKind: 'iteration-end' as const, loopId: loop.id },
  }))
}

type GroupView = { id: string; group: EditorDocument['groups'][number]; position: { x: number; y: number }; width: number; height: number }

class GroupViewIndex {
  private readonly views = new Map<string, GroupView>()
  private nodeSignatures = new Map<string, string>()
  private memberships = new Map<string, string[]>()
  private groupSignatures = new Map<string, string>()
  private manifests?: Map<string, NodeManifest>

  sync(nodes: StudioNode[], groups: EditorDocument['groups'], manifests: Map<string, NodeManifest>) {
    const nodeMap = new Map(nodes.map((node) => [node.id, node]))
    const memberships = new Map<string, string[]>()
    const affected = new Set<string>()
    const groupIds = new Set(groups.map((group) => group.id))
    for (const group of groups) {
      const signature = JSON.stringify(group)
      if (this.groupSignatures.get(group.id) !== signature || this.manifests !== manifests) affected.add(group.id)
      this.groupSignatures.set(group.id, signature)
      for (const nodeId of group.nodeIds) memberships.set(nodeId, [...(memberships.get(nodeId) ?? []), group.id])
    }
    for (const id of this.groupSignatures.keys()) if (!groupIds.has(id)) { this.groupSignatures.delete(id); this.views.delete(id) }
    const nodeIds = new Set(nodes.map((node) => node.id))
    for (const node of nodes) {
      const signature = `${node.position.x}:${node.position.y}:${node.width}:${node.height}:${node.measured?.width}:${node.measured?.height}:${node.data.editorKind === 'action' ? `${node.data.nodeType}@${node.data.typeVersion}` : node.data.editorKind}`
      if (this.nodeSignatures.get(node.id) !== signature) for (const groupId of new Set([...(this.memberships.get(node.id) ?? []), ...(memberships.get(node.id) ?? [])])) affected.add(groupId)
      this.nodeSignatures.set(node.id, signature)
    }
    for (const id of this.nodeSignatures.keys()) if (!nodeIds.has(id)) { for (const groupId of this.memberships.get(id) ?? []) affected.add(groupId); this.nodeSignatures.delete(id) }
    for (const group of groups) if (affected.has(group.id)) {
      const view = computeGroupView(group, nodeMap, manifests)
      if (view) this.views.set(group.id, view)
      else this.views.delete(group.id)
    }
    this.memberships = memberships
    this.manifests = manifests
    return groups.flatMap((group) => { const view = this.views.get(group.id); return view ? [view] : [] })
  }
}

function computeGroupView(group: EditorDocument['groups'][number], nodeMap: Map<string, StudioNode>, manifests: Map<string, NodeManifest>): GroupView | undefined {
  const members = group.nodeIds.flatMap((id) => { const node = nodeMap.get(id); return node ? [node] : [] })
  if (!members.length) return undefined
  // Parented children carry container-relative positions: resolve before measuring bounds.
  const absolute = (node: StudioNode) => {
    const parentId = node.data.editorKind === 'action' ? node.data.parentId : undefined
    const parent = parentId ? nodeMap.get(parentId) : undefined
    return parent ? { x: parent.position.x + node.position.x, y: parent.position.y + node.position.y } : node.position
  }
  const bounds = members.map((node) => ({ position: absolute(node), metrics: nodeMetrics(node, manifests) }))
  const left = Math.min(...bounds.map(({ position }) => position.x))
  const top = Math.min(...bounds.map(({ position }) => position.y))
  const right = Math.max(...bounds.map(({ position, metrics }) => position.x + metrics.width))
  const bottom = Math.max(...bounds.map(({ position, metrics }) => position.y + metrics.height))
  const collapsedMetrics = canvasNodeMetrics('default', { kind: 'group' }, true)
  return { id: `group:${group.id}`, group, position: { x: left - 32, y: top - 48 }, width: group.collapsed ? collapsedMetrics.width : right - left + 64, height: group.collapsed ? collapsedMetrics.height : bottom - top + 80 }
}

function nodeMetrics(node: StudioNode, manifests: Map<string, NodeManifest>) {
  if (node.data.editorKind === 'exit') return { width: 240, height: 62 }
  if (node.data.editorKind !== 'action') return { width: 96, height: 96 }
  const manifest = manifestForNode(manifests, node)
  const metrics = canvasNodeMetrics(canvasNodeRole(manifest), { richHeight: node.height ?? node.measured?.height })
  return { width: node.width ?? node.measured?.width ?? metrics.width, height: (node.height ?? node.measured?.height ?? metrics.height) + (metrics.labelBelow ? 28 : 0) }
}

function materializeNodes(nodes: StudioNode[], edges: StudioEdge[], manifests: Map<string, NodeManifest>, containers: Map<string, LoopContainerView>, cache: Map<string, { source: StudioNode; manifest?: NodeManifest; frame: string; value: CanvasNode }>, language: string) {
  const ids = new Set(nodes.map((node) => node.id))
  for (const id of cache.keys()) if (!ids.has(id)) cache.delete(id)
  return nodes.map((node) => {
    const manifest = manifestForNode(manifests, node)
    const parentId = node.data.editorKind === 'action' ? node.data.parentId : undefined
    const container = containers.get(node.id)
    const parent = parentId && parentId !== node.id ? containers.get(parentId) : undefined
    // Membership or container size changes must refresh projected parent/extent wiring.
    const childFrame = container ? container.children.map((child) => `${child.id}:${child.position.x}:${child.position.y}:${child.width ?? child.measured?.width ?? ''}:${child.height ?? child.measured?.height ?? ''}`).join('|') : ''
    const bodyEdges = container ? edges.filter((edge) => container.childIds.has(edge.source) || container.childIds.has(edge.target)).map((edge) => `${edge.id}:${edge.source}:${edge.sourceHandle}:${edge.target}:${edge.targetHandle}`).join('|') : ''
    const frame = `${language}|${container ? `c${Math.round(loopContainerWidth(node))}x${Math.round(loopContainerHeight(node))}:${childFrame}:${bodyEdges}` : ''}|${parent ? `p${Math.round(loopContainerWidth(parent.loop))}x${Math.round(loopContainerHeight(parent.loop))}` : ''}`
    const cached = cache.get(node.id)
    if (cached?.source === node && cached.manifest === manifest && cached.frame === frame) return cached.value
    const value = projectCanvasNode(node, edges, manifests, manifest, container, parent, language)
    cache.set(node.id, { source: node, manifest, frame, value })
    return value
  })
}

function projectCanvasNode(node: StudioNode, edges: StudioEdge[], manifests: Map<string, NodeManifest>, manifest: NodeManifest | undefined, container: LoopContainerView | undefined, parent: LoopContainerView | undefined, language: string): CanvasNode {
  if (container) {
    const width = loopContainerWidth(node)
    const height = loopContainerHeight(node)
    return {
      id: node.id,
      type: 'loop-container' as const,
      position: node.position,
      selected: node.selected,
      measured: node.measured,
      dragging: node.dragging,
      width,
      height,
      style: { width, height, zIndex: 0 },
      initialWidth: width,
      initialHeight: height,
      dragHandle: '.drag-handle',
      data: { editorKind: 'loop-container' as const, loopId: node.id, label: manifest ? localizedNodeLabel(manifest, node.data.label, language) : node.data.label, parallelism: containerParallelism(node), childCount: container.children.length, boundaryLinks: loopBoundaryLinks(container, edges, manifests, width) },
    }
  }
  const base = withInitialNodeMetrics(node, manifest)
  if (!parent) return base
  const width = loopContainerWidth(parent.loop)
  const height = loopContainerHeight(parent.loop)
  return {
    ...base,
    parentId: parent.loop.id,
    extent: [[-LOOP_CHILD_EXTENT_MARGIN, -LOOP_CHILD_EXTENT_MARGIN], [width + LOOP_CHILD_EXTENT_MARGIN, height + LOOP_CHILD_EXTENT_MARGIN]] as [[number, number], [number, number]],
  }
}

export function loopBoundaryLinks(container: LoopContainerView, edges: StudioEdge[], manifests: Map<string, NodeManifest>, width: number) {
  const links: Array<{ id: string; kind: 'entry' | 'main' | 'error'; source: { x: number; y: number }; target: { x: number; y: number } }> = []
  const entries = container.children.filter((child) => !edges.some((edge) => container.childIds.has(edge.source) && edge.target === child.id))
  const sinks = container.children.filter((child) => !edges.some((edge) => edge.source === child.id && container.childIds.has(edge.target)))
  const start = { x: LOOP_CHIP_POSITION.x + 140, y: LOOP_CHIP_POSITION.y + 20 }
  const end = loopEndChipPosition(width)
  for (const child of entries) {
    const metrics = nodeMetrics(child, manifests)
    links.push({ id: `entry:${child.id}`, kind: 'entry', source: start, target: { x: child.position.x, y: child.position.y + metrics.height / 2 } })
  }
  for (const child of sinks) {
    const metrics = nodeMetrics(child, manifests)
    const manifest = manifestForNode(manifests, child)
    links.push({ id: `main:${child.id}`, kind: 'main', source: { x: child.position.x + metrics.width, y: child.position.y + metrics.height / 3 }, target: { x: end.x, y: end.y + 20 } })
    if (manifest?.outputPorts.some((port) => port.kind === 'error' || port.name === 'error')) links.push({ id: `error:${child.id}`, kind: 'error', source: { x: child.position.x + metrics.width, y: child.position.y + metrics.height * 2 / 3 }, target: { x: end.x, y: end.y + 44 } })
  }
  return links
}

function withInitialNodeMetrics(node: StudioNode, manifest?: NodeManifest): StudioNode {
  if (node.data.editorKind === 'exit') {
    return { ...node, initialWidth: 240, initialHeight: 62, style: { ...node.style, zIndex: 3 } }
  }
  if (node.data.editorKind !== 'action') return node
  const metrics = canvasNodeMetrics(canvasNodeRole(manifest), canvasBranchPorts(manifest, node.data.parameters))
  return { ...node, initialWidth: metrics.width, initialHeight: metrics.height, style: { ...node.style, zIndex: 3 } }
}

export function materializeRuntimeEdges(edges: StudioEdge[], runtimeStatuses: Map<string, string>, cache = new Map<string, { source: StudioEdge; sourceStatus?: string; targetStatus?: string; value: StudioEdge }>()) {
  const ids = new Set(edges.map((edge) => edge.id))
  for (const id of cache.keys()) if (!ids.has(id)) cache.delete(id)
  return edges.map((edge) => {
    const sourceStatus = runtimeStatuses.get(edge.source)
    const targetStatus = runtimeStatuses.get(edge.target)
    const cached = cache.get(edge.id)
    if (cached?.source === edge && cached.sourceStatus === sourceStatus && cached.targetStatus === targetStatus) return cached.value
    const runtimeStatus = edgeRuntimeStatus(sourceStatus, targetStatus)
    const color = edge.data?.sourcePortKind === 'error' || runtimeStatus === 'failed' ? 'var(--ui-danger)' : runtimeStatus === 'running' ? 'var(--ui-warning)' : runtimeStatus === 'succeeded' ? 'var(--ui-success)' : 'var(--ui-primary)'
    const value: StudioEdge = { ...edge, zIndex: 0, data: edge.data ? { ...edge.data, runtimeStatus } : edge.data, markerEnd: { type: MarkerType.ArrowClosed, color } }
    cache.set(edge.id, { source: edge, sourceStatus, targetStatus, value })
    return value
  })
}

function edgeRuntimeStatus(source?: string, target?: string) {
  if (source === 'failed' || target === 'failed') return 'failed'
  if (source === 'running' || target === 'running') return 'running'
  if (source === 'succeeded' && target === 'succeeded') return 'succeeded'
  return undefined
}

function annotationNode(annotation: EditorDocument['annotations'][number], editor: Pick<import("../store/editor-store").EditorState, 'updateAnnotation' | 'removeAnnotation' | 'beginEdit' | 'commitEdit' | 'updateAnnotationFrame'>): CanvasNode {
  return {
    id: `annotation:${annotation.id}`,
    type: 'annotation',
    position: { x: annotation.x, y: annotation.y },
    style: { width: annotation.width ?? 240, height: annotation.height ?? 160, zIndex: 1 },
    dragHandle: '.drag-handle',
    data: {
      editorKind: 'annotation', annotationId: annotation.id, text: annotation.text, color: annotation.color,
      onChange: (patch) => editor.updateAnnotation(annotation.id, patch),
      onRemove: () => editor.removeAnnotation(annotation.id),
      onResizeStart: () => editor.beginEdit({ annotationIds: [annotation.id] }),
      onResize: (frame) => editor.updateAnnotationFrame(annotation.id, frame),
      onResizeEnd: editor.commitEdit,
    },
  }
}

function groupNode(view: GroupView, editor: Pick<import("../store/editor-store").EditorState, 'toggleGroup' | 'removeGroup'>): CanvasNode {
  return {
    id: view.id,
    type: 'group',
    position: view.position,
    style: { width: view.width, height: view.height, zIndex: 2, pointerEvents: view.group.collapsed ? 'auto' : 'none' },
    dragHandle: '.drag-handle',
    selectable: false,
    connectable: false,
    data: { editorKind: 'group', groupId: view.group.id, label: view.group.label, collapsed: Boolean(view.group.collapsed), color: view.group.color, memberCount: view.group.nodeIds.length, onToggle: () => editor.toggleGroup(view.group.id), onRemove: () => editor.removeGroup(view.group.id) },
  }
}

function LegendDot({ className, label }: { className: string; label: string }) { return <span className="flex items-center gap-1.5"><span className={`size-2.5 rounded-full border-2 border-background shadow-sm ${className}`} />{label}</span> }
