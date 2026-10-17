import { Handle, NodeResizer, Position, useUpdateNodeInternals, useViewport, type NodeProps } from '@xyflow/react'
import { ChevronDown, ChevronRight, Flag, Play, Plus, Repeat2, Trash2 } from 'lucide-react'
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
import { useTranslation } from 'react-i18next'

import { cn } from '../../../shared/lib/cn'
import type { CanvasNode } from '../model/types'
import { PortHandle } from '../nodes/manifest-node'
import { useEditorStore, useEditorStoreApi } from '../store/editor-store'
import { useCanvasRenderStore } from '../store/canvas-render-store'
import { LOOP_CONTAINER_DEFAULT_HEIGHT, LOOP_CONTAINER_DEFAULT_WIDTH, LOOP_CONTAINER_MIN_HEIGHT, LOOP_CONTAINER_MIN_WIDTH } from '../utils/layout'

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
  if (group.collapsed) return <div className="studio-group-proxy relative flex h-16 w-60 items-center gap-3 border border-primary/50 bg-surface px-3 shadow-sm" data-testid={`studio-group-${group.groupId}`}>
    <Handle className="!size-3 !border-2 !border-background !bg-muted-foreground" id="group-in" isConnectable={false} position={Position.Left} type="target" />
    <button aria-label={t('studio.group.expand')} className="nodrag grid size-8 shrink-0 place-items-center rounded-md bg-primary/10 text-primary hover:bg-primary/15" onClick={group.onToggle} type="button"><ChevronRight className="size-4" /></button>
    <div className="min-w-0 flex-1"><strong className="block truncate text-xs">{group.label}</strong><span className="text-[9px] text-muted-foreground">{t('studio.group.members', { count: group.memberCount })}</span></div>
    {!readOnly && <button aria-label={t('studio.group.delete')} className="nodrag grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-danger" onClick={group.onRemove} type="button"><Trash2 className="size-3.5" /></button>}
    <Handle className="!size-3 !border-2 !border-background !bg-primary" id="group-out" isConnectable={false} position={Position.Right} type="source" />
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
  const occupiedSignature = useCanvasRenderStore((state) => data.editorKind === 'loop-container' ? state.occupiedHandlesByNodeId.get(data.loopId) ?? '' : '')
  const container = data.editorKind === 'loop-container' ? data : undefined
  if (!container) return null
  const occupied = new Set(occupiedSignature ? occupiedSignature.split('\u0001') : [])
  return <section className={cn('studio-node studio-node-zoom-full studio-loop-container relative h-full w-full', selected && 'is-selected')} data-testid={`studio-loop-${container.loopId}`}>
    <svg aria-hidden className="pointer-events-none absolute inset-0 z-[1] size-full overflow-visible" data-testid={`studio-loop-boundary-links-${container.loopId}`}>
      {container.boundaryLinks.map((link) => <path className={link.kind === 'error' ? 'studio-loop-boundary-edge-error' : 'studio-loop-boundary-edge-main'} d={`M ${link.source.x} ${link.source.y} C ${link.source.x + 40} ${link.source.y}, ${link.target.x - 40} ${link.target.y}, ${link.target.x} ${link.target.y}`} data-loop-boundary-edge={link.kind} fill="none" key={link.id} vectorEffect="non-scaling-stroke" />)}
    </svg>
    {selected && !readOnly && <LoopResizeHandles loopId={container.loopId} />}
    <PortHandle id="main" kind="main" label={t('studio.boundary.input')} placement={{ position: Position.Left, axis: 18 }} type="target" />
    <PortHandle addLabel={t('studio.ports.addAfter', { label: t('studio.boundary.output') })} id="main" kind="main" label={t('studio.boundary.output')} onHover={onSourceHover ? (active) => onSourceHover(container.loopId, 'main', active) : undefined} onQuickAdd={onQuickAdd && !occupied.has('main') ? () => onQuickAdd(container.loopId, 'main') : undefined} placement={{ position: Position.Right, axis: 18 }} type="source" />
    <PortHandle addLabel={t('studio.ports.addAfter', { label: t('studio.boundary.error') })} id="error" kind="error" label={t('studio.boundary.error')} onHover={onSourceHover ? (active) => onSourceHover(container.loopId, 'error', active) : undefined} onQuickAdd={onQuickAdd && !occupied.has('error') ? () => onQuickAdd(container.loopId, 'error') : undefined} placement={{ position: Position.Right, axis: 32 }} type="source" />
    <header className="drag-handle absolute left-0 top-0 flex h-10 w-full items-center gap-2 px-3">
      <span className="grid size-6 shrink-0 place-items-center rounded-md bg-[#06b6d4] text-white" data-testid={`studio-loop-icon-${container.loopId}`}><Repeat2 className="size-3.5" /></span>
      <strong className="min-w-0 flex-1 truncate text-[13px] font-semibold">{container.label}</strong>
      <span className="shrink-0 rounded-full border border-cyan-300/70 bg-cyan-50/80 px-2 py-0.5 text-[10px] font-semibold leading-none text-cyan-700" data-testid={`studio-loop-parallel-${container.loopId}`}>{t('studio.container.parallelChip', { count: container.parallelism })}</span>
    </header>
    <span className="pointer-events-none absolute bottom-2 left-3 text-[10px] text-muted-foreground">{t('studio.container.builtinVars')}</span>
    {container.childCount === 0 && <div className="pointer-events-none absolute inset-x-6 top-28 rounded-lg border border-dashed border-cyan-300 bg-surface/70 px-4 py-6 text-center text-[11px] text-muted-foreground">{t('studio.container.emptyHint')}</div>}
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
    gesture.current = { pointerId: event.pointerId, corner, clientX: event.clientX, clientY: event.clientY, x: node.position.x, y: node.position.y, width: node.width ?? node.measured?.width ?? LOOP_CONTAINER_DEFAULT_WIDTH, height: node.height ?? node.measured?.height ?? LOOP_CONTAINER_DEFAULT_HEIGHT }
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
  return <div className="studio-loop-chip" data-testid={`studio-chip-${chip.loopId}`}>
    <Play className="size-3.5 fill-current" />
    {t('studio.container.iterationStart')}
    {onQuickAdd && <button aria-label={t('studio.container.addFirstNode')} className="nodrag ml-auto grid size-6 place-items-center rounded-full border border-cyan-300 bg-surface text-cyan-700" onClick={(event) => { event.stopPropagation(); onQuickAdd(`${chip.loopId}::iteration-start`, 'main') }} type="button"><Plus className="size-3.5" /></button>}
    <Handle className="studio-handle studio-port-output !z-30 !grid !size-4 !place-items-center !border-0 !bg-transparent" id="main" position={Position.Right} style={{ top: 20 }} title={t('studio.container.iterationStart')} type="source"><span className="studio-handle-mark block" /></Handle>
  </div>
}

export function IterationEndNode({ id, data }: NodeProps<CanvasNode>) {
  const { t } = useTranslation()
  const updateNodeInternals = useUpdateNodeInternals()
  const chip = data.editorKind === 'iteration-end' ? data : undefined
  useEffect(() => { if (chip) updateNodeInternals(id) }, [chip, id, updateNodeInternals])
  if (!chip) return null
  return <div className="studio-loop-chip studio-loop-end-chip" data-testid={`studio-end-chip-${chip.loopId}`}>
    <Handle className="studio-handle studio-port-input !z-30 !grid !size-4 !place-items-center !border-0 !bg-transparent" id="main" position={Position.Left} style={{ top: 20 }} title={t('studio.boundary.output')} type="target"><span className="studio-handle-mark block" /></Handle>
    <Handle className="studio-handle studio-port-error !z-30 !grid !size-4 !place-items-center !border-0 !bg-transparent" id="error" position={Position.Left} style={{ top: 44 }} title={t('studio.boundary.error')} type="target"><span className="studio-handle-mark block" /></Handle>
    <Flag className="size-3.5" />
    <span className="min-w-0"><strong className="block truncate">{t('studio.container.iterationEnd')}</strong><span className="mt-1 flex gap-2 text-[9px] font-medium text-muted-foreground"><span>{t('studio.boundary.output')}</span><span className="text-danger">{t('studio.boundary.error')}</span></span></span>
  </div>
}
