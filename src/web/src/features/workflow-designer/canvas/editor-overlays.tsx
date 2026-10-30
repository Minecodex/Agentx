import { NodeResizer, Position, useUpdateNodeInternals, useViewport, type NodeProps } from '@xyflow/react'
import { ChevronDown, ChevronRight, Flag, Play, Plus, Repeat2, Trash2 } from 'lucide-react'
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
import { useTranslation } from 'react-i18next'

import { cn } from '../../../shared/lib/cn'
import type { CanvasNode } from '../model/types'
import { PortHandle } from '../nodes/port-handle'
import { CardShell } from '../nodes/card-shell'
import { boundaryGeometry, groupGeometry, loopGeometry, studioNodeGeometry } from '../nodes/canvas-node-geometry'
import { nodeGroupColor } from '../nodes/node-appearance'
import { useEditorStore, useEditorStoreApi } from '../store/editor-store'
import { useCanvasRenderStore } from '../store/canvas-render-store'
import { LOOP_CONTAINER_MIN_HEIGHT, LOOP_CONTAINER_MIN_WIDTH } from '../utils/layout'

const NOTE_COLORS = ['yellow', 'blue', 'green', 'rose'] as const

export function AnnotationNode({ id, data, selected }: NodeProps<CanvasNode>) {
  const readOnly = useEditorStore((state) => state.readOnly)
  const { t } = useTranslation()
  const note = data.editorKind === 'annotation' ? data : undefined
  const [editing, setEditing] = useState(false)
  const [text, setText] = useState(note?.text ?? '')
  const updateNodeInternals = useUpdateNodeInternals()
  useEffect(() => setText(note?.text ?? ''), [note?.text])
  useEffect(() => updateNodeInternals(id), [id, note?.text, updateNodeInternals])
  if (!note) return null

  const commitText = () => {
    setEditing(false)
    if (!readOnly && text !== note.text) note.onChange({ text })
  }
  return <article className={cn('studio-note relative h-full w-full overflow-hidden border shadow-sm', `studio-note-${note.color ?? 'yellow'}`, selected && 'ring-2 ring-primary/40')} data-testid={`studio-note-${note.annotationId}`} onDoubleClick={(event) => { event.stopPropagation(); if (!readOnly) setEditing(true) }}>
    <NodeResizer color="var(--ui-primary)" isVisible={selected && !readOnly} minHeight={80} minWidth={150} onResize={(_, frame) => note.onResize(frame)} onResizeEnd={() => { note.onResizeEnd(); updateNodeInternals(id) }} onResizeStart={note.onResizeStart} />
    <header className="drag-handle flex h-8 items-center gap-1 border-b border-black/10 px-2">
      <span className="flex-1 text-[9px] font-semibold uppercase text-black/55">{t('studio.note.title')}</span>
      {!readOnly && <div className="nodrag flex items-center gap-1">{NOTE_COLORS.map((color) => <button aria-label={t('studio.note.color', { color })} className={cn('size-3 rounded-full border border-black/15', `studio-note-swatch-${color}`, note.color === color && 'ring-1 ring-black/40')} key={color} onClick={() => note.onChange({ color })} type="button" />)}<button aria-label={t('studio.note.delete')} className="ml-1 grid size-5 place-items-center rounded text-black/50 hover:bg-black/10 hover:text-black" onClick={note.onRemove} type="button"><Trash2 className="size-3" /></button></div>}
    </header>
    {editing && !readOnly ? <textarea autoFocus className="nodrag nowheel h-[calc(100%-2rem)] w-full resize-none bg-transparent p-3 text-xs leading-5 text-black/75 outline-none" onBlur={commitText} onChange={(event) => setText(event.target.value)} onKeyDown={(event) => { if (event.key === 'Escape') { setText(note.text); setEditing(false) }; if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') commitText() }} value={text} /> : <div className="h-[calc(100%-2rem)] whitespace-pre-wrap break-words p-3 text-xs leading-5 text-black/75">{note.text}</div>}
  </article>
}

export function GroupNode({ data }: NodeProps<CanvasNode>) {
  const readOnly = useEditorStore((state) => state.readOnly)
  const { t } = useTranslation()
  const group = data.editorKind === 'group' ? data : undefined
  if (!group) return null
  const geometry = groupGeometry()
  if (group.collapsed) return <div className="studio-node studio-node-zoom-full studio-group-proxy relative" style={{ width: geometry.width, height: geometry.height }} data-testid={`studio-group-${group.groupId}`}>
    <CardShell label={group.label} group={nodeGroupColor('if')} icon={<button aria-label={t('studio.group.expand')} className="nodrag grid size-6 place-items-center" onClick={group.onToggle} type="button"><ChevronRight className="size-4" /></button>} trailing={!readOnly && <button aria-label={t('studio.group.delete')} className="nodrag grid size-6 place-items-center text-muted-foreground hover:text-danger" onClick={group.onRemove} type="button"><Trash2 className="size-3.5" /></button>}>
      <div className="absolute inset-x-3 top-[68px] text-xs text-muted-foreground">{t('studio.group.members', { count: group.memberCount })}</div>
    </CardShell>
    {geometry.ports.map(port => <PortHandle key={`${port.direction}:${port.id}`} id={port.id} kind={port.kind} label={t(port.kind === 'error' ? 'studio.boundary.error' : 'studio.ports.flow')} placement={{ position: port.side === 'left' ? Position.Left : Position.Right, axis: port.y }} row={port.kind !== 'error'} type={port.direction === 'input' ? 'target' : 'source'} connectable={false} />)}
  </div>
  return <section className="studio-group-expanded relative h-full w-full border border-dashed border-primary/45 bg-primary/[0.025]" data-testid={`studio-group-${group.groupId}`}>
    <header className="drag-handle pointer-events-auto absolute left-0 top-0 flex h-8 max-w-full items-center gap-2 rounded-br-md border-b border-r border-primary/25 bg-surface/95 px-2 shadow-sm">
      <button aria-label={t('studio.group.collapse')} className="nodrag grid size-5 place-items-center rounded text-primary hover:bg-primary/10" onClick={group.onToggle} type="button"><ChevronDown className="size-3.5" /></button>
      <strong className="max-w-48 truncate text-[10px]">{group.label}</strong><span className="text-[9px] text-muted-foreground">{group.memberCount}</span>
      {!readOnly && <button aria-label={t('studio.group.delete')} className="nodrag grid size-5 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-danger" onClick={group.onRemove} type="button"><Trash2 className="size-3" /></button>}
    </header>
  </section>
}

export function LoopContainerNode({ data, selected }: NodeProps<CanvasNode>) {
  const readOnly = useEditorStore((state) => state.readOnly)
  const { t } = useTranslation()
  const onQuickAdd = useCanvasRenderStore((state) => state.onQuickAdd)
  const onSourceHover = useCanvasRenderStore((state) => state.onSourceHover)
  const runtimeStatus = useCanvasRenderStore(state => data.editorKind === 'loop-container' ? state.runtimeStatuses.get(data.loopId) : undefined)
  const occupiedSignature = useCanvasRenderStore((state) => data.editorKind === 'loop-container' ? state.occupiedHandlesByNodeId.get(data.loopId) ?? '' : '')
  const container = data.editorKind === 'loop-container' ? data : undefined
  if (!container) return null
  const occupied = new Set(occupiedSignature ? occupiedSignature.split('\u0001') : [])
  const geometry = loopGeometry()
  return <section className={cn('studio-node studio-node-zoom-full studio-loop-container relative h-full w-full', selected && 'is-selected')} data-testid={`studio-loop-${container.loopId}`}>
    <CardShell label={container.label} group={nodeGroupColor('loop_over_items')} icon={<Repeat2 className="size-3.5" data-testid={`studio-loop-icon-${container.loopId}`} />} selected={selected} runtimeStatus={runtimeStatus} className="studio-loop-shell" trailing={<span className="studio-loop-parallel shrink-0 rounded-full border px-2 py-0.5 text-[10px] font-semibold" data-testid={`studio-loop-parallel-${container.loopId}`}>{t('studio.container.parallelChip', { count: container.parallelism })}</span>}>
      <span className="pointer-events-none absolute bottom-2 left-3 text-[10px] text-muted-foreground">{t('studio.container.builtinVars')}</span>
      {container.childCount === 0 && <div className="studio-loop-empty pointer-events-none absolute inset-x-6 top-48 rounded-lg border border-dashed bg-surface/70 px-4 py-6 text-center text-xs text-muted-foreground">{t('studio.container.emptyHint')}</div>}
    </CardShell>
    {selected && !readOnly && <LoopResizeHandles loopId={container.loopId} />}
    {geometry.ports.map(port => {
      const output = port.direction === 'output'
      const label = t(port.kind === 'error' ? 'studio.boundary.error' : output ? 'studio.boundary.output' : 'studio.boundary.input')
      return <PortHandle key={`${port.direction}:${port.id}`} id={port.id} kind={port.kind} label={label} addLabel={t('studio.ports.addAfter', { label })} onHover={output && onSourceHover ? active => onSourceHover(container.loopId, port.id, active) : undefined} onQuickAdd={output && onQuickAdd && !occupied.has(port.id) ? () => onQuickAdd(container.loopId, port.id) : undefined} placement={{ position: port.side === 'left' ? Position.Left : Position.Right, axis: port.y }} row={port.kind !== 'error'} type={output ? 'source' : 'target'} />
    })}
  </section>
}

type LoopResizeCorner = 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right'
type LoopResizeGesture = { pointerId: number; corner: LoopResizeCorner; clientX: number; clientY: number; x: number; y: number; width: number; height: number }
const LOOP_RESIZE_CORNERS: LoopResizeCorner[] = ['top-left', 'top-right', 'bottom-left', 'bottom-right']

function LoopResizeHandles({ loopId }: { loopId: string }) {
  const storeApi = useEditorStoreApi()
  const { t } = useTranslation()
  const { zoom } = useViewport()
  const gesture = useRef<LoopResizeGesture>()
  const start = (corner: LoopResizeCorner, event: ReactPointerEvent<HTMLButtonElement>) => {
    const node = storeApi.getState().nodes.find((candidate) => candidate.id === loopId)
    if (!node) return
    event.preventDefault()
    event.stopPropagation()
    event.currentTarget.setPointerCapture?.(event.pointerId)
    storeApi.getState().beginEdit({ nodeIds: [loopId] })
    const geometry = studioNodeGeometry(node)
    gesture.current = { pointerId: event.pointerId, corner, clientX: event.clientX, clientY: event.clientY, x: node.position.x, y: node.position.y, width: geometry.width, height: geometry.height }
  }
  const move = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const active = gesture.current
    if (!active || active.pointerId !== event.pointerId) return
    event.preventDefault()
    event.stopPropagation()
    const dx = (event.clientX - active.clientX) / zoom
    const dy = (event.clientY - active.clientY) / zoom
    const width = Math.max(LOOP_CONTAINER_MIN_WIDTH, active.width + (active.corner.endsWith('right') ? dx : -dx))
    const height = Math.max(LOOP_CONTAINER_MIN_HEIGHT, active.height + (active.corner.startsWith('bottom') ? dy : -dy))
    storeApi.getState().updateLoopFrame(loopId, {
      x: active.corner.endsWith('left') ? active.x - (width - active.width) : active.x,
      y: active.corner.startsWith('top') ? active.y - (height - active.height) : active.y,
      width,
      height,
    })
  }
  const end = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (gesture.current?.pointerId !== event.pointerId) return
    event.preventDefault()
    event.stopPropagation()
    gesture.current = undefined
    storeApi.getState().commitEdit()
  }
  return <>{LOOP_RESIZE_CORNERS.map((corner) => <button aria-label={`${t('studio.container.resize')} ${corner}`} className={`studio-loop-resize-handle nodrag nopan ${corner.replace('-', ' ')}`} data-resize-corner={corner} key={corner} onPointerCancel={end} onPointerDown={(event) => start(corner, event)} onPointerMove={move} onPointerUp={end} type="button" />)}</>
}

export function IterationChipNode({ id, data }: NodeProps<CanvasNode>) {
  const { t } = useTranslation()
  const onQuickAdd = useCanvasRenderStore((state) => state.onQuickAdd)
  const updateNodeInternals = useUpdateNodeInternals()
  const chip = data.editorKind === 'iteration-chip' ? data : undefined
  useEffect(() => { if (chip) updateNodeInternals(id) }, [chip, id, updateNodeInternals])
  if (!chip) return null
  const geometry = boundaryGeometry('start')
  return <div className="studio-node studio-loop-chip relative" style={{ width: geometry.width, height: geometry.height }} data-testid={`studio-chip-${chip.loopId}`}>
    <CardShell group={nodeGroupColor('loop_over_items')} icon={<Play className="size-3.5" />} label={t('studio.container.iterationStart')} trailing={onQuickAdd && <button aria-label={t('studio.container.addFirstNode')} className="nodrag grid size-6 place-items-center rounded-full border border-border text-primary" onClick={event => { event.stopPropagation(); onQuickAdd(`${chip.loopId}::iteration-start`, 'main') }} type="button"><Plus className="size-3.5" /></button>}>
      <span className="absolute inset-x-3 top-16 truncate text-[11px] text-muted-foreground">{t('studio.container.iterationEntrySummary')}</span>
    </CardShell>
    <PortHandle id="main" kind="main" label={t('studio.container.iterationStart')} placement={{ position: Position.Right, axis: geometry.ports[0].y }} row type="source" />
  </div>
}

export function IterationEndNode({ id, data }: NodeProps<CanvasNode>) {
  const { t } = useTranslation()
  const updateNodeInternals = useUpdateNodeInternals()
  const chip = data.editorKind === 'iteration-end' ? data : undefined
  useEffect(() => { if (chip) updateNodeInternals(id) }, [chip, id, updateNodeInternals])
  if (!chip) return null
  const geometry = boundaryGeometry('exit')
  return <div className="studio-node studio-loop-chip studio-loop-end-chip relative" style={{ width: geometry.width, height: geometry.height }} data-testid={`studio-end-chip-${chip.loopId}`}>
    <CardShell group={nodeGroupColor('loop_over_items')} icon={<Flag className="size-3.5" />} label={t('studio.container.iterationEnd')}>
      <span className="absolute inset-x-3 top-16 truncate text-[11px] text-muted-foreground">{t('studio.container.iterationExitSummary')}</span>
    </CardShell>
    {geometry.ports.map(port => <PortHandle key={port.id} id={port.id} kind={port.kind} label={t(port.kind === 'error' ? 'studio.boundary.error' : 'studio.boundary.output')} placement={{ position: Position.Left, axis: port.y }} row={port.kind !== 'error'} type="target" />)}
  </div>
}
