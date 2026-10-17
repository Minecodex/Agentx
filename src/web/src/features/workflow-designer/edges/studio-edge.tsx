import { BaseEdge, Position, getBezierPath, getSmoothStepPath, useStore, useViewport, type EdgeProps } from '@xyflow/react'
import { Plus, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'

import type { StudioEdge } from '../model/types'
import { useEditorStore } from '../store/editor-store'

const EDGE_PADDING_BOTTOM = 130
const EDGE_PADDING_X = 40
const EDGE_BORDER_RADIUS = 16
const HANDLE_SIZE = 20

export function StudioEdgeComponent(props: EdgeProps<StudioEdge>) {
  const { t } = useTranslation()
  const [hovered, setHovered] = useState(false)
  const readOnly = useEditorStore((state) => state.readOnly)
  const removeEdge = useEditorStore((state) => state.removeEdge)
  const requestEdgeInsert = useEditorStore((state) => state.requestEdgeInsert)
  const renderer = useStore((state) => state.domNode?.querySelector('.react-flow__renderer'))
  const viewport = useViewport()
  const error = props.data?.sourcePortKind === 'error'
  const runtimeStatus = props.data?.runtimeStatus
  const { path, labelX, labelY } = getStudioEdgePath(props)
  const toolbarVisible = hovered || props.selected
  return <>
    <BaseEdge id={props.id} interactionWidth={40} markerEnd={props.markerEnd} path={path} style={{ stroke: error || runtimeStatus === 'failed' ? 'var(--ui-danger)' : runtimeStatus === 'running' ? 'var(--ui-warning)' : runtimeStatus === 'succeeded' ? 'var(--ui-success)' : props.selected ? 'var(--ui-primary)' : 'var(--ui-border-strong)', strokeDasharray: error ? '7 5' : undefined, strokeWidth: 2 }} />
    <path d={path} fill="none" onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)} pointerEvents="stroke" stroke="transparent" strokeWidth={40} />
    {!readOnly && renderer && createPortal(<div className="nodrag nopan studio-edge-toolbar absolute z-20 flex -translate-x-1/2 -translate-y-1/2 items-center rounded-md border border-border bg-surface p-0.5 shadow-sm" data-edge-id={props.id} onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)} style={{ left: labelX * viewport.zoom + viewport.x, opacity: toolbarVisible ? 1 : 0, pointerEvents: toolbarVisible ? 'auto' : 'none', top: labelY * viewport.zoom + viewport.y }}><button aria-label={t('studio.edges.insert')} className="grid size-6 place-items-center rounded hover:bg-muted" onClick={() => requestEdgeInsert(props.id)} title={t('studio.edges.insert')} type="button"><Plus className="size-3.5" /></button><button aria-label={t('studio.edges.delete')} className="grid size-6 place-items-center rounded text-danger hover:bg-danger/10" onClick={() => removeEdge(props.id)} title={t('studio.edges.delete')} type="button"><Trash2 className="size-3.5" /></button></div>, renderer)}
  </>
}

export function getStudioEdgePath(props: Pick<EdgeProps<StudioEdge>, 'sourceX' | 'sourceY' | 'sourcePosition' | 'targetX' | 'targetY' | 'targetPosition'>) {
  const { sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition } = props
  if (sourceX - HANDLE_SIZE <= targetX) {
    const [path, labelX, labelY] = getBezierPath(props)
    return { path, labelX, labelY }
  }
  const midX = (sourceX + targetX) / 2
  const bottomY = sourceY + EDGE_PADDING_BOTTOM
  const [first] = getSmoothStepPath({ sourceX, sourceY, targetX: midX, targetY: bottomY, sourcePosition, targetPosition: Position.Right, borderRadius: EDGE_BORDER_RADIUS, offset: EDGE_PADDING_X })
  const [second] = getSmoothStepPath({ sourceX: midX, sourceY: bottomY, targetX, targetY, sourcePosition: Position.Left, targetPosition, borderRadius: EDGE_BORDER_RADIUS, offset: EDGE_PADDING_X })
  return { path: `${first} ${second}`, labelX: midX, labelY: bottomY }
}
