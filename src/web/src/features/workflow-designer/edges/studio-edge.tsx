import { useSmartEdgePath } from '@tisoap/react-flow-smart-edge'
import { BaseEdge, useStore, useViewport, type EdgeProps } from '@xyflow/react'
import { AlertTriangle, Plus, Trash2 } from 'lucide-react'
import { useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'
import type { StudioEdge } from '../model/types'
import { useEditorStore } from '../store/editor-store'
import { orthogonalRoutePoints, routeFromPoints, routeIsClear, routeToolbarPoint, temporaryRoute, ROUTE_PADDING } from './canvas-route'
import { routeWaypoints } from './routing-world'
import { useRoutingWorld } from './routing-context'

export function StudioEdgeComponent(props: EdgeProps<StudioEdge>) {
  const { t } = useTranslation()
  const [hovered, setHovered] = useState(false)
  const readOnly = useEditorStore(state => state.readOnly)
  const removeEdge = useEditorStore(state => state.removeEdge)
  const requestEdgeInsert = useEditorStore(state => state.requestEdgeInsert)
  const renderer = useStore(state => state.domNode?.querySelector('.react-flow__renderer'))
  const viewport = useViewport()
  const world = useRoutingWorld()
  const mountedBatch = useRef(world.batches ?? 0)
  const domain = world.domainFor(props.source, props.target)
  const error = props.data?.sourcePortKind === 'error' || props.sourceHandleId === 'error'
  const source = { x: props.sourceX, y: props.sourceY }, target = { x: props.targetX, y: props.targetY }
  const waypoints = routeWaypoints(domain, props.id, source, target, error)
  const result = useSmartEdgePath({ id: props.id, source: props.source, target: props.target, sourceX: source.x, sourceY: source.y, targetX: target.x, targetY: target.y, sourcePosition: props.sourcePosition, targetPosition: props.targetPosition, preset: 'smoothstep', options: { nodePadding: ROUTE_PADDING, gridRatio: 8, borderRadius: 12, avoidAreas: domain.avoidAreas }, waypoints })
  const preview = temporaryRoute(source, target)
  const points = result.route?.kind === 'routed' ? orthogonalRoutePoints([source, ...result.route.points.map(([x, y]) => ({ x, y })), target]) : preview.points
  const clear = !result.isDragging && result.route?.kind === 'routed' && routeIsClear(points, domain.obstacles, domain.frame)
  const blocked = !world.dragging && world.complete && !clear && (result.route?.kind === 'routed' || (world.batches ?? 0) > mountedBatch.current)
  const route = clear ? routeFromPoints(points) : { ...preview, status: blocked ? 'blocked' as const : 'pending' as const }
  const runtimeStatus = props.data?.runtimeStatus
  const color = error || runtimeStatus === 'failed' ? 'var(--ui-danger)' : runtimeStatus === 'running' ? 'var(--ui-warning)' : runtimeStatus === 'succeeded' ? 'var(--ui-success)' : props.selected ? 'var(--ui-primary)' : 'var(--ui-border-strong)'
  const toolbarVisible = hovered || props.selected
  const controls = world.hasQuickAdd(props.source, props.sourceHandleId ?? 'main') ? [{ x: source.x + 12, y: source.y - 12, width: 24, height: 24 }] : []
  const toolbar = routeToolbarPoint(route.points, [...domain.obstacles, ...controls], viewport.zoom)
  return <>
    <g data-route-id={props.id} data-route-state={route.status} data-route-points={JSON.stringify(route.points)}>
      <BaseEdge id={props.id} interactionWidth={32} markerEnd={props.markerEnd} path={route.path} style={{ stroke: color, strokeDasharray: error ? '6 5' : route.status !== 'ready' ? '3 5' : undefined, strokeWidth: props.selected ? 2.5 : 1.8, opacity: route.status === 'ready' ? 1 : .45 }} />
      <path d={route.path} fill="none" onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)} pointerEvents="stroke" stroke="transparent" strokeWidth={32} />
    </g>
    {blocked && renderer && createPortal(<span className="studio-route-warning pointer-events-auto absolute z-20 -translate-x-1/2 -translate-y-1/2 rounded border border-warning/30 bg-surface p-1 text-warning shadow-sm" role="status" style={{ left: route.label.x * viewport.zoom + viewport.x, top: route.label.y * viewport.zoom + viewport.y }} title={t('studio.routing.blocked')}><AlertTriangle aria-label={t('studio.routing.blocked')} className="size-3.5" /></span>, renderer)}
    {!readOnly && !props.data?.derived && renderer && toolbar && createPortal(<div className="nodrag nopan studio-edge-toolbar absolute z-20 flex -translate-x-1/2 -translate-y-1/2 items-center rounded-md border border-border bg-surface p-0.5 shadow-sm" data-edge-id={props.id} onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)} style={{ left: toolbar.x * viewport.zoom + viewport.x, opacity: toolbarVisible ? 1 : 0, pointerEvents: toolbarVisible ? 'auto' : 'none', top: toolbar.y * viewport.zoom + viewport.y }}><button aria-label={t('studio.edges.insert')} className="grid size-6 place-items-center rounded hover:bg-muted" disabled={blocked} onClick={() => requestEdgeInsert(props.id)} title={t('studio.edges.insert')} type="button"><Plus className="size-3.5" /></button><button aria-label={t('studio.edges.delete')} className="grid size-6 place-items-center rounded text-danger hover:bg-danger/10" onClick={() => removeEdge(props.id)} title={t('studio.edges.delete')} type="button"><Trash2 className="size-3.5" /></button></div>, renderer)}
  </>
}
