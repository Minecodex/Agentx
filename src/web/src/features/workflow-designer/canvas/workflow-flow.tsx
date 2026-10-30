import { canvasEdgePortKinds } from '../utils/edge-port-kinds'
import { computeGroupView, type GroupView } from '../utils/group-geometry'
import { Background, BackgroundVariant, Controls, ConnectionLineType, MarkerType, MiniMap, ReactFlow, type AriaLabelConfig, type Connection, type NodeChange, type ReactFlowInstance } from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import { forwardRef, useCallback, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState, type DragEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { useShallow } from 'zustand/react/shallow'

import type { CanvasNode, ConnectionInteractionState, EditorDocument, NodeManifest, StudioEdge, StudioNode } from '../model/types'
import { StudioEdgeComponent } from '../edges/studio-edge'
import { CanvasRoutingProvider } from '../edges/canvas-routing-provider'
import { loopGeometryForContent, studioNodeGeometry } from '../nodes/canvas-node-geometry'
import { STUDIO_BOUNDARY_HEIGHT, STUDIO_BOUNDARY_WIDTH } from '../nodes/node-appearance'
import { canvasZoomTier, useCanvasRenderStoreApi } from '../store/canvas-render-store'
import { useEditorStore, useEditorStoreApi } from '../store/editor-store'
import { localizedNodeLabel } from '../model/manifest-localization'
import { inspectConnection, isIterationEndId, isIterationChipId, iterationChipId, iterationEndId, normalizeIterationChip } from '../utils/connections'
import { createGraphIndex, IncrementalGraphIndex, occupiedHandlesByNodeId, portKey } from '../utils/graph-index'
import { proxyEdges } from '../utils/group-edges'
import { manifestForNode } from '../utils/manifest-lookup'
import { LOOP_CHIP_POSITION, LOOP_END_CHIP_HEIGHT, loopEndChipPosition } from '../utils/layout'
import { CANVAS_NODE_TYPES } from './canvas-node-packages'

export const LARGE_GRAPH_RENDER_THRESHOLD = 150
export const LARGE_GRAPH_MINIMAP_THRESHOLD = 300
// Keep fitted cards above the MiniMap and controls, including their quick-add targets.
const CANVAS_FIT_VIEW_OPTIONS = { minZoom: 0.1, maxZoom: 1, padding: { top: '48px', right: '48px', bottom: '180px', left: '48px' } } as const

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
  const [draggedViewIds, setDraggedViewIds] = useState<ReadonlySet<string>>(new Set())
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
  const [viewGroupStates, setViewGroupStates] = useState<Record<string, boolean>>({})
  const visibleGroups = useMemo(() => readOnly ? groups.map(group => group.id in viewGroupStates ? { ...group, collapsed: viewGroupStates[group.id] } : group) : groups, [groups, readOnly, viewGroupStates])
  const annotationActions = useMemo(() => ({
    updateAnnotation: editor.updateAnnotation,
    removeAnnotation: editor.removeAnnotation,
    beginEdit: editor.beginEdit,
    commitEdit: editor.commitEdit,
    updateAnnotationFrame: editor.updateAnnotationFrame,
  }), [editor.beginEdit, editor.commitEdit, editor.removeAnnotation, editor.updateAnnotation, editor.updateAnnotationFrame])
  const groupActions = useMemo(() => ({ toggleGroup: (id: string) => {
    if (readOnly) setViewGroupStates(current => ({ ...current, [id]: !(current[id] ?? groups.find(group => group.id === id)?.collapsed ?? false) }))
    else editor.toggleGroup(id)
  }, removeGroup: editor.removeGroup }), [editor.removeGroup, editor.toggleGroup, groups, readOnly])
  const groupViewIndex = useRef(new GroupViewIndex())
  const groupViews = useMemo(() => groupViewIndex.current.sync(nodes, visibleGroups, manifests), [visibleGroups, manifests, nodes])
  const collapsedByMember = useMemo(() => {
    const result = new Map<string, string>()
    for (const group of visibleGroups) if (group.collapsed) for (const nodeId of group.nodeIds) result.set(nodeId, `group:${group.id}`)
    for (const node of storeApi.getState().nodes) if (node.data.editorKind === 'action' && node.data.parentId && result.has(node.data.parentId)) result.set(node.id, result.get(node.data.parentId)!)
    return result
  }, [visibleGroups, editor.graphRevision, storeApi])
  const containers = useMemo(() => readOnly ? readonlyContainerViews(indexContainers(nodes), collapsedByMember, groupViews, manifests) : indexContainers(nodes), [nodes, readOnly, collapsedByMember, groupViews, manifests])
  const visibleContainers = useMemo(() => new Map([...containers].filter(([id]) => !collapsedByMember.has(id))), [containers, collapsedByMember])
  const renderedNodeCache = useRef(new Map<string, { source: StudioNode; manifest?: NodeManifest; frame: string; value: CanvasNode }>())
  const flowNodes = useMemo<CanvasNode[]>(() => [
    ...materializeNodes(nodes.filter((node) => !collapsedByMember.has(node.id)), edges, manifests, containers, renderedNodeCache.current, i18n.language),
    ...boundaryNodes(boundaryLayouts),
    ...annotations.map((annotation) => annotationNode(annotation, annotationActions)),
    ...groupViews.filter(view => !view.parentId || !collapsedByMember.has(view.parentId)).map(view => groupNode(view, groupActions)),
    ...iterationChipNodes(visibleContainers),
    ...iterationEndNodes(visibleContainers),
  ].map(node => readOnly ? { ...node, draggable: false, connectable: false, deletable: false } : draggedViewIds.has(node.id) ? { ...node, dragging: true } : node), [annotationActions, annotations, boundaryLayouts, collapsedByMember, containers, visibleContainers, edges, groupActions, groupViews, i18n.language, manifests, nodes, readOnly, draggedViewIds])
  const semanticContainers = useMemo(() => indexContainers(storeApi.getState().nodes), [storeApi, editor.graphRevision])
  const canvasEdges = useMemo(() => {
    const visible = new Map([...semanticContainers].filter(([id]) => !collapsedByMember.has(id)))
    return proxyEdges(canvasEdgePortKinds([...edges, ...loopBoundaryEdges(visible, edges, manifests)], storeApi.getState().nodes, manifests), collapsedByMember)
  }, [collapsedByMember, edges, manifests, semanticContainers, storeApi])
  const renderedEdgeCache = useRef(new Map<string, { source: StudioEdge; sourceStatus?: string; targetStatus?: string; value: StudioEdge }>())
  const flowEdges = useMemo(() => materializeRuntimeEdges(canvasEdges, runtimeStatuses, renderedEdgeCache.current), [canvasEdges, runtimeStatuses])
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
    onQuickAdd: readOnly ? undefined : onQuickAdd,
    onSourceHover: readOnly ? undefined : onSourceHover,
  }), [canvasRenderStore, editor.graphRevision, graphIndex, manifests, occupiedHandles, onQuickAdd, onSourceHover, readOnly, runtimeStatuses, viewport.zoom])
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
  const onNodeDragStart = useCallback((_: unknown, node: CanvasNode, dragged?: CanvasNode[]) => {
    const ids = new Set((dragged ?? [node]).map(item => item.id))
    const draggedGroup = groupViewMap.get(node.id)
    for (const id of draggedGroup?.group.nodeIds ?? []) ids.add(id)
    for (const candidate of nodes) if (candidate.data.editorKind === 'action' && candidate.data.parentId && ids.has(candidate.data.parentId)) ids.add(candidate.id)
    for (const id of [...ids]) if (containers.has(id)) { ids.add(iterationChipId(id)); ids.add(iterationEndId(id)) }
    setDraggedViewIds(ids)
    if (node.id === '__start__') return editor.beginEdit({ boundary: 'start' })
    if (node.id.startsWith('annotation:')) return editor.beginEdit({ annotationIds: [node.id.slice('annotation:'.length)] })
    const group = groupViewMap.get(node.id)
    if (group) return editor.beginEdit({ nodeIds: group.group.nodeIds })
    const nodeIds = nodes.filter((candidate) => candidate.selected || candidate.id === node.id).map((candidate) => candidate.id)
    editor.beginEdit({ nodeIds })
  }, [editor, groupViewMap, nodes, containers])
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
  }, [editor, storeApi])
  const onNodeDragStop = useCallback((_: unknown, node: CanvasNode, dragged?: CanvasNode[]) => {
    releaseEscapedChildren(dragged ?? [node])
    setDraggedViewIds(new Set())
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
    <CanvasRoutingProvider nodes={flowNodes} edges={canvasEdges}><ReactFlow<CanvasNode, StudioEdge> zIndexMode="manual" minZoom={0.1} ariaLabelConfig={ariaLabelConfig} connectionLineType={ConnectionLineType.SmoothStep} connectionRadius={60} defaultViewport={viewport} edges={flowEdges} edgeTypes={EDGE_TYPES} edgesReconnectable={!readOnly} deleteKeyCode={readOnly ? null : "Backspace"} fitViewOptions={CANVAS_FIT_VIEW_OPTIONS} nodes={flowNodes} nodesConnectable={!readOnly} nodesDraggable={!readOnly} nodeTypes={CANVAS_NODE_TYPES} onConnect={onConnect} onConnectEnd={() => { setConnectionState({ status: 'cancelled' }); window.requestAnimationFrame(() => setConnectionState({ status: 'idle' })) }} onConnectStart={(_, params) => setConnectionState({ status: 'connecting', nodeId: params.nodeId ?? '', handleId: params.handleId ?? undefined })} onEdgesChange={readOnly ? undefined : onEdgesChange} onInit={(value) => { instance.current = value }} onMoveEnd={(_, next) => { if (readOnly) canvasRenderStore.setState({zoomTier:canvasZoomTier(next.zoom)}); else setViewport(next) }} onNodeClick={(_, node) => { if (node.data.editorKind === 'boundary') { select(undefined); onBoundaryOpen?.(node.data.boundary) } else { select(node.id); if (node.data.editorKind === 'action' || node.data.editorKind === 'exit' || node.data.editorKind === 'loop-container') onNodeOpen?.(node.id) } }} onNodeDragStart={readOnly ? undefined : onNodeDragStart} onNodeDragStop={readOnly ? undefined : onNodeDragStop} onNodesChange={readOnly ? undefined : onNodesChange} onPaneClick={() => { select(undefined); onPaneClear?.() }} onReconnect={readOnly ? undefined : onReconnect} onReconnectEnd={() => editor.clearEdgeReconnectRequest()} isValidConnection={valid} multiSelectionKeyCode="Shift" onlyRenderVisibleElements={nodes.length >= LARGE_GRAPH_RENDER_THRESHOLD} proOptions={{ hideAttribution: true }} selectionOnDrag={!readOnly}>
      <Background color="var(--ui-canvas-dot)" gap={22} size={1} variant={BackgroundVariant.Dots} />
      <Controls fitViewOptions={CANVAS_FIT_VIEW_OPTIONS} aria-label={t('studio.canvasA11y.controls')} className="!border-border !bg-surface !shadow-md" showInteractive={!readOnly} />
      {nodes.length < LARGE_GRAPH_MINIMAP_THRESHOLD && <MiniMap ariaLabel={t('studio.canvasA11y.minimap')} className="!border !border-border !bg-surface" maskColor="color-mix(in srgb, var(--ui-background) 70%, transparent)" pannable zoomable />}
    </ReactFlow></CanvasRoutingProvider>
  </div>
})

function boundaryNodes(layouts: EditorDocument['boundaryLayouts']): CanvasNode[] {
  const start = layouts.find((layout) => layout.boundary === 'start') ?? { x: 40, y: 220 }
  return [
    { id: '__start__', type: 'boundary', zIndex: 4, width: STUDIO_BOUNDARY_WIDTH, height: STUDIO_BOUNDARY_HEIGHT, measured: { width: STUDIO_BOUNDARY_WIDTH, height: STUDIO_BOUNDARY_HEIGHT }, position: { x: start.x, y: start.y }, draggable: true, deletable: false, data: { editorKind: 'boundary', boundary: 'start', label: '' } },
  ]
}

export type LoopContainerView = { loop: StudioNode; children: StudioNode[]; childIds: Set<string> }

const loopContainerWidth = (loop: StudioNode) => studioNodeGeometry(loop).width
const loopContainerHeight = (loop: StudioNode) => studioNodeGeometry(loop).height

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

/** View-only expansion may enlarge a saved compact loop without changing its document. */
export function readonlyContainerViews(containers: Map<string, LoopContainerView>, collapsed: Map<string, string>, groups: GroupView[], manifests: Map<string, NodeManifest>) {
  return new Map([...containers].map(([id, container]) => {
    const content = container.children.filter(node => !collapsed.has(node.id)).map(node => { const geometry = nodeMetrics(node, manifests); return { ...node.position, width: geometry.width, height: geometry.height }; });
    for (const group of groups) if (group.group.collapsed && group.parentId === id) content.push({ ...group.position, width: group.width, height: group.height });
    const geometry = loopGeometryForContent(container.loop, content);
    return [id, { ...container, loop: { ...container.loop, width: geometry.width, height: geometry.height } }] as const;
  }));
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
    zIndex: 4,
    initialWidth: STUDIO_BOUNDARY_WIDTH,
    initialHeight: STUDIO_BOUNDARY_HEIGHT,
    width: STUDIO_BOUNDARY_WIDTH, height: STUDIO_BOUNDARY_HEIGHT, style: { width: STUDIO_BOUNDARY_WIDTH, height: STUDIO_BOUNDARY_HEIGHT, zIndex: 4 },
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
    zIndex: 4,
    initialWidth: STUDIO_BOUNDARY_WIDTH,
    initialHeight: LOOP_END_CHIP_HEIGHT,
    width: STUDIO_BOUNDARY_WIDTH, height: LOOP_END_CHIP_HEIGHT, style: { width: STUDIO_BOUNDARY_WIDTH, height: LOOP_END_CHIP_HEIGHT, zIndex: 4 },
    data: { editorKind: 'iteration-end' as const, loopId: loop.id },
  }))
}


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
      const geometry = nodeMetrics(node, manifests)
      const signature = `${node.position.x}:${node.position.y}:${geometry.width}:${geometry.height}:${node.data.editorKind === 'action' ? `${node.data.nodeType}@${node.data.typeVersion}` : node.data.editorKind}`
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


function nodeMetrics(node: StudioNode, manifests: Map<string, NodeManifest>) {
  return studioNodeGeometry(node, manifestForNode(manifests, node))
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
    const frame = `${language}|${container ? `c${Math.round(loopContainerWidth(container.loop))}x${Math.round(loopContainerHeight(container.loop))}:${childFrame}:${bodyEdges}` : ''}|${parent ? `p${Math.round(loopContainerWidth(parent.loop))}x${Math.round(loopContainerHeight(parent.loop))}` : ''}`
    const cached = cache.get(node.id)
    if (cached?.source === node && cached.manifest === manifest && cached.frame === frame) return cached.value
    const value = projectCanvasNode(node, manifest, container, parent, language)
    cache.set(node.id, { source: node, manifest, frame, value })
    return value
  })
}

function projectCanvasNode(node: StudioNode, manifest: NodeManifest | undefined, container: LoopContainerView | undefined, parent: LoopContainerView | undefined, language: string): CanvasNode {
  if (container) {
    const width = loopContainerWidth(container.loop)
    const height = loopContainerHeight(container.loop)
    return {
      id: node.id,
      type: 'loop-container' as const,
      position: node.position,
      selected: node.selected,
      measured: { width, height },
      dragging: node.dragging,
      width,
      height,
      zIndex: 0,
      style: { width, height, zIndex: 0 },
      initialWidth: width,
      initialHeight: height,
      dragHandle: '.drag-handle',
      data: { editorKind: 'loop-container' as const, loopId: node.id, label: manifest ? localizedNodeLabel(manifest, node.data.label, language) : node.data.label, parallelism: containerParallelism(node), childCount: container.children.length },
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

export function loopBoundaryEdges(containers: Map<string, LoopContainerView>, edges: StudioEdge[], manifests: Map<string, NodeManifest>): StudioEdge[] {
  const result: StudioEdge[] = []
  const link = (id: string, source: string, sourceHandle: string, target: string, targetHandle: string, error = false) => result.push({ id, source, sourceHandle, target, targetHandle, type: 'studio', selectable: false, deletable: false, reconnectable: false, data: { edgeKind: 'execution', sourcePortKind: error ? 'error' : 'main', derived: true } })
  for (const [loopId, container] of containers) {
    for (const child of container.children) {
      const geometry = nodeMetrics(child, manifests)
      const incoming = edges.some(edge => edge.target === child.id && container.childIds.has(edge.source))
      const outgoing = edges.some(edge => edge.source === child.id && container.childIds.has(edge.target))
      if (!incoming) {
        const port = geometry.ports.find(port => port.direction === 'input' && port.kind === 'main')
        if (port) link(`loop-entry:${loopId}:${child.id}`, iterationChipId(loopId), 'main', child.id, port.id)
      }
      if (!outgoing) for (const port of geometry.ports.filter(port => port.direction === 'output')) link(`loop-exit:${loopId}:${child.id}:${port.id}`, child.id, port.id, iterationEndId(loopId), port.kind === 'error' ? 'error' : 'main', port.kind === 'error')
    }
  }
  return result
}

function withInitialNodeMetrics(node: StudioNode, manifest?: NodeManifest): StudioNode {
  const geometry = studioNodeGeometry(node, manifest)
  return { ...node, zIndex: 3, width: geometry.width, height: geometry.height, measured: { width: geometry.width, height: geometry.height }, initialWidth: geometry.width, initialHeight: geometry.height, style: { ...node.style, width: geometry.width, height: geometry.height, zIndex: 3 } }
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
    const color = edge.data?.sourcePortKind === 'error' || runtimeStatus === 'failed' ? 'var(--ui-danger)' : runtimeStatus === 'running' ? 'var(--ui-warning)' : runtimeStatus === 'succeeded' ? 'var(--ui-success)' : edge.selected ? 'var(--ui-primary)' : 'var(--ui-border-strong)'
    const value: StudioEdge = { ...edge, zIndex: 1, data: edge.data ? { ...edge.data, runtimeStatus } : edge.data, markerEnd: { type: MarkerType.ArrowClosed, color } }
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
    zIndex: 2,
    position: { x: annotation.x, y: annotation.y },
    style: { width: annotation.width ?? 240, height: annotation.height ?? 160, zIndex: 2 },
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
    zIndex: view.group.collapsed ? 3 : 0,
    position: view.position,
    parentId: view.parentId,
    width: view.width,
    height: view.height,
    measured: { width: view.width, height: view.height },
    style: { width: view.width, height: view.height, padding: 0, border: 0, borderRadius: 0, background: 'transparent', boxShadow: 'none', textAlign: 'left', zIndex: view.group.collapsed ? 3 : 0, pointerEvents: view.group.collapsed ? 'auto' : 'none' },
    dragHandle: '.drag-handle',
    selectable: false,
    connectable: false,
    data: { editorKind: 'group', groupId: view.group.id, label: view.group.label, collapsed: Boolean(view.group.collapsed), color: view.group.color, memberCount: view.group.nodeIds.length, onToggle: () => editor.toggleGroup(view.group.id), onRemove: () => editor.removeGroup(view.group.id) },
  }
}

function LegendDot({ className, label }: { className: string; label: string }) { return <span className="flex items-center gap-1.5"><span className={`size-2.5 rounded-full border-2 border-background shadow-sm ${className}`} />{label}</span> }
