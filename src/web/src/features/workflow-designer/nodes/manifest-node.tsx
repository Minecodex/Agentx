import { Handle, Position, useUpdateNodeInternals, type NodeProps } from '@xyflow/react'
import { AlertTriangle, Check, LoaderCircle, Plus, X, Zap } from 'lucide-react'
import { memo, useEffect, useMemo, useState, type CSSProperties, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import type { TFunction } from 'i18next'

import { cn } from '../../../shared/lib/cn'
import { localizedNodeLabel, localizeManifest } from '../model/manifest-localization'
import type { ActionNodeData, CanvasNodeRole, NodeManifest, PortKind, InputBinding, StudioNode } from '../model/types'
import { useCanvasRenderStore } from '../store/canvas-render-store'
import type { NodeBindingSummary } from '../utils/graph-index'
import { STUDIO_BRANCH_ROW_HEIGHT, STUDIO_CARD_HEAD_HEIGHT, STUDIO_CARD_ROW_HEIGHT, STUDIO_INPUT_ROW_HEIGHT, canvasNodeFamily, canvasNodeMetrics, canvasNodeRole, nodeGroupColor } from './node-appearance'
import { NodeIcon } from './node-icon'
import { PluginCanvas } from './plugin-canvas'

export type { NodeBindingSummary } from '../utils/graph-index'

export const ManifestNode = memo(function ManifestNode({ id, data, selected }: NodeProps<StudioNode>) {
  const { t, i18n } = useTranslation()
  const manifest = useCanvasRenderStore((state) => state.manifests.get(`node:${id}`)
    ?? (data.editorKind === 'action' ? state.manifests.get(`${data.nodeType}@${data.typeVersion}`) : undefined))
  const runtimeStatus = useCanvasRenderStore((state) => state.runtimeStatuses.get(id))
  const bindingSummaries = useCanvasRenderStore((state) => state.bindingSummaries.get(id) ?? EMPTY_BINDINGS)
  const occupiedHandleSignature = useCanvasRenderStore((state) => state.occupiedHandlesByNodeId.get(id) ?? '')
  const zoomTier = useCanvasRenderStore((state) => state.zoomTier)
  const [hovered, setHovered] = useState(false)
  const onQuickAdd = useCanvasRenderStore((state) => state.onQuickAdd)
  const onSourceHover = useCanvasRenderStore((state) => state.onSourceHover)
  const updateNodeInternals = useUpdateNodeInternals()
  const inputPorts = manifest ? visibleInputPorts(manifest, data.editorKind === 'action' ? data.parameters : {}) : []
  const portSignature = useMemo(() => manifest ? JSON.stringify([
    inputPorts.map((port) => [port.name, port.kind]),
    manifest.outputPorts.map((port) => [port.name, port.kind]),
  ]) : '', [inputPorts, manifest])
  const occupiedHandles = useMemo(() => new Set(occupiedHandleSignature ? occupiedHandleSignature.split('\u0001') : []), [occupiedHandleSignature])
  const branchRows = data.editorKind === 'action' && manifest ? buildBranchRows(manifest, data.parameters, t) : undefined
  const inputRowPorts = inputPorts.length > 1 ? inputPorts : undefined
  const rowHandleSignature = `${branchRows?.map((row) => row.handleId).join('\u0001') ?? ''}#${inputRowPorts?.map((port) => port.name).join('\u0001') ?? ''}`
  useEffect(() => scheduleNodeInternalsUpdate(id, updateNodeInternals), [id, portSignature, rowHandleSignature, updateNodeInternals])
  if (data.editorKind !== 'action') return null

  const role = canvasNodeRole(manifest)
  const family = canvasNodeFamily(role)
  const card = manifest ? buildCard(data, manifest, bindingSummaries, t) : undefined
  const summaryRowCount = branchRows?.length ? 0 : card?.rows.length ?? 0
  const metrics = canvasNodeMetrics(role, {
    bodyRows: summaryRowCount,
    branchRows: branchRows?.length ?? 0,
    inputRows: inputRowPorts?.length ?? 0,
    attachments: card?.attachments,
  })
  const summaryRowsHeight = summaryRowCount * STUDIO_CARD_ROW_HEIGHT
  const rowHandleTop = (centerOffset: number) => `${Math.round(((STUDIO_CARD_HEAD_HEIGHT + summaryRowsHeight + centerOffset) / metrics.height) * 10000) / 100}%`
  const branchPortNames = new Set(branchRows?.map((row) => row.portName))
  const localized = manifest ? localizeManifest(manifest, i18n.language) : undefined
  const label = manifest ? localizedNodeLabel(manifest, data.label, i18n.language) : data.label || data.nodeType
  const statusClass = selected ? 'studio-node-selected' : runtimeStatus === 'running' || runtimeStatus === 'succeeded' || runtimeStatus === 'failed' ? `studio-node-${runtimeStatus}` : undefined
  const groupTint = nodeGroupColor(manifest?.nodeType ?? data.nodeType)
  const displayTier = selected || hovered ? 'full' : zoomTier
  return <div className={cn('studio-node relative shrink-0', `studio-node-${family}`, `studio-node-role-${role}`, `studio-node-zoom-${displayTier}`)} data-role={role} data-testid={`studio-node-${id}`} onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)} style={{ width: metrics.width, height: metrics.height }}>
    {!inputRowPorts && inputPorts.map((port, index) => <PortHandle id={port.name} key={`in-${port.name}`} kind={port.kind} label={localized?.inputPortLabel(port.name) ?? port.name} placement={portPlacement('input', port.kind, port.name, index, inputPorts)} type="target" />)}
    <div className={cn('studio-card relative flex size-full flex-col overflow-hidden rounded-2xl border border-border bg-surface text-foreground shadow-sm transition-shadow', statusClass)} title={bindingSummaries.map((binding) => `${binding.role}: ${binding.label}`).join('\n') || label}>
      <div className="studio-card-head relative flex h-11 shrink-0 items-center gap-2 px-3">
        <span className="studio-card-icon grid size-6 shrink-0 place-items-center rounded-md text-white" style={{ backgroundColor: groupTint }}><NodeIcon className="size-3.5" iconKey={manifest?.iconKey || roleIcon(role)} /></span>
        <strong className="studio-card-title min-w-0 flex-1 truncate text-[13px] font-semibold leading-none">{label}</strong>
        {runtimeStatus === 'succeeded' ? <Check aria-hidden className="studio-card-status size-3.5 shrink-0 text-success" /> : runtimeStatus === 'failed' ? <X aria-hidden className="studio-card-status size-3.5 shrink-0 text-danger" /> : runtimeStatus === 'running' ? <LoaderCircle aria-hidden className="studio-card-status size-3.5 shrink-0 animate-spin text-warning" /> : undefined}
      </div>
      {manifest?.plugin && <PluginCanvas manifest={manifest} parameters={data.parameters} />}
      {card && card.rows.length > 0 && <div className="studio-card-body px-3">{card.rows}</div>}
      {branchRows && branchRows.length > 0 && <div className="studio-card-rows">{branchRows.map((row) => (
        <div className="studio-branch-row" key={row.handleId}>
          <span className="studio-branch-text flex min-w-0 flex-1 items-center gap-1.5">
            {row.tag && <span className={cn('studio-branch-tag shrink-0', `studio-branch-tag-${row.tone}`)}>{row.tag}</span>}
            {row.label && <span className="min-w-0 flex-1 truncate">{row.label}</span>}
            {row.badge && <span className="studio-branch-badge shrink-0">{row.badge}</span>}
            {row.sub && <span className="studio-branch-sub shrink-0">{row.sub}</span>}
          </span>
        </div>
      ))}</div>}
      {inputRowPorts && <div className="studio-card-rows">{inputRowPorts.map((port) => (
        <div className="studio-branch-row studio-branch-row-input" key={`in-row-${port.name}`}>
          <span className="studio-branch-text truncate">{localized?.inputPortLabel(port.name) ?? port.name}</span>
        </div>
      ))}</div>}
      {card?.attachments && <AttachmentBadges bindings={bindingSummaries} translate={t} />}
    </div>
    {role === 'trigger' && <span className="studio-card-trigger absolute -left-2.5 top-[22px] grid size-5 -translate-y-1/2 place-items-center rounded-full bg-warning text-background"><Zap className="size-3 fill-current" /></span>}
    {branchRows?.map((row, index) => (
      <RowHandle addLabel={t('studio.ports.addAfter', { label: row.label || row.tag })} id={row.handleId} key={`row-out-${row.handleId}`} onHover={onSourceHover ? (active) => onSourceHover(id, row.handleId, active) : undefined} onQuickAdd={!data.disabled && onQuickAdd && (!occupiedHandles.has(row.handleId) || manifest?.outputPorts.find((port) => port.name === row.portName)?.variadic) ? () => onQuickAdd(id, row.handleId, row.portName) : undefined} style={{ top: rowHandleTop(index * STUDIO_BRANCH_ROW_HEIGHT + STUDIO_BRANCH_ROW_HEIGHT / 2) }} title={[row.tag, row.label].filter(Boolean).join(' ')} type="source" />
    ))}
    {inputRowPorts?.map((port, index) => (
      <RowHandle id={port.name} key={`row-in-${port.name}`} style={{ top: rowHandleTop(index * STUDIO_INPUT_ROW_HEIGHT + STUDIO_INPUT_ROW_HEIGHT / 2) }} title={localized?.inputPortLabel(port.name) ?? port.name} type="target" />
    ))}
    {manifest?.outputPorts.map((port, index) => branchPortNames.has(port.name) ? null : <PortHandle addLabel={t('studio.ports.addAfter', { label: localized?.outputPortLabel(port.name) ?? port.name })} id={port.name} key={`out-${port.name}`} kind={port.kind} label={localized?.outputPortLabel(port.name) ?? port.name} onHover={onSourceHover ? (active) => onSourceHover(id, port.name, active) : undefined} onQuickAdd={!data.disabled && onQuickAdd && (!occupiedHandles.has(port.name) || port.variadic) ? () => onQuickAdd(id, port.name) : undefined} placement={portPlacement('output', port.kind, port.name, index, manifest.outputPorts)} type="source" />)}
    {data.disabled && <AlertTriangle className="absolute -right-2 -top-2 z-30 size-5 rounded-full bg-surface p-0.5 text-warning" />}
  </div>
}, (previous, next) => previous.id === next.id && previous.data === next.data && previous.selected === next.selected)

const EMPTY_BINDINGS: NodeBindingSummary[] = []

function visibleInputPorts(manifest: NodeManifest, parameters: Record<string, unknown>) {
  if (manifest.nodeType !== 'merge') return manifest.inputPorts
  const names = parameters.mode === 'combine_by_position' || parameters.mode === 'combine_by_key' ? new Set(['left', 'right']) : new Set(['main'])
  return manifest.inputPorts.filter((port) => names.has(port.name))
}
let pendingInternals = new Set<string>()
let internalsUpdateScheduled = false

function scheduleNodeInternalsUpdate(id: string, update: (ids: string[]) => void) {
  pendingInternals.add(id)
  if (internalsUpdateScheduled) return
  internalsUpdateScheduled = true
  queueMicrotask(() => {
    const ids = [...pendingInternals]
    pendingInternals = new Set()
    internalsUpdateScheduled = false
    update(ids)
  })
}

type CardContent = { rows: ReactNode[]; attachments: boolean }

const RUNNER_LABELS: Record<string, string> = { python: 'Python', javascript: 'JavaScript', shell: 'Shell' }
const MERGE_MODES = { append: 'append', combine_by_position: 'combine_by_position', combine_by_key: 'combine_by_key' } as const

function cardRow(content: ReactNode, key: number) {
  return <div className="studio-card-row flex h-[18px] items-center gap-1.5 text-[11px] leading-none text-muted-foreground" key={key}>{content}</div>
}

/** Summarizes node configuration into 0-3 compact rows for the card body. */
function buildCard(data: ActionNodeData, manifest: NodeManifest, bindings: NodeBindingSummary[], t: TFunction): CardContent {
  const parameters = data.parameters
  const rows: ReactNode[] = []
  const push = (content: ReactNode) => { if (rows.length < 3) rows.push(cardRow(content, rows.length)) }
  switch (manifest.nodeType) {
    case 'declarative_http': {
      const method = typeof parameters.method === 'string' ? parameters.method : 'GET'
      const url = dynamicText(parameters.url)
      push(<>
        <span className="shrink-0 rounded border border-[#d9d6fe] bg-[#f4f3ff] px-1 py-0.5 text-[9px] font-bold leading-none text-[#6941c6]">{method}</span>
        {url && <span className="min-w-0 flex-1 truncate">{url}</span>}
      </>)
      break
    }
    case 'code': {
      const runner = typeof parameters.runner === 'string' ? RUNNER_LABELS[parameters.runner] ?? parameters.runner : undefined
      const outputs = parameters.outputExample && typeof parameters.outputExample === 'object' && !Array.isArray(parameters.outputExample)
        ? Object.keys(parameters.outputExample as Record<string, unknown>)
        : []
      push(<>
        {runner && <span className="shrink-0 font-medium">{runner}</span>}
        {outputs.length > 0 && <span className="min-w-0 flex-1 truncate">{t('studio.card.outputs')}: {outputs.slice(0, 3).join(', ')}</span>}
      </>)
      break
    }
    case 'model':
    case 'agent': {
      const model = bindingModelName(bindings) ?? inspectorModelName(data)
      if (model) push(<span className="min-w-0 flex-1 truncate">{model}</span>)
      if (manifest.nodeType === 'model') {
        const responseMode = parameters.responseMode === 'json_schema' ? 'json_schema' : 'text'
        push(<span className="truncate">{t('studio.card.modelMode', { mode: t(`studio.card.modelModes.${responseMode}`) })}</span>)
      }
      if (manifest.nodeType === 'agent' && typeof parameters.maxIterations === 'number') push(<span className="truncate">{t('studio.card.maxIterations', { count: parameters.maxIterations })}</span>)
      break
    }
    case 'set': {
      const values = parameters.values as { kind?: unknown; fields?: unknown } | undefined
      const count = values?.kind === 'object' && values.fields && typeof values.fields === 'object'
        ? Object.keys(values.fields).length
        : 0
      push(<span className="truncate">{t('studio.card.setFields', { count })}</span>)
      break
    }
    case 'merge': {
      const mode = typeof parameters.mode === 'string' && parameters.mode in MERGE_MODES ? MERGE_MODES[parameters.mode as keyof typeof MERGE_MODES] : 'append'
      push(<span className="truncate">{t('studio.card.mergeMode', { mode: t(`studio.card.mergeModes.${mode}`) })}</span>)
      break
    }
    case 'list': {
      const filterCount = Array.isArray((parameters.filter as { conditions?: unknown[] } | undefined)?.conditions)
        ? (parameters.filter as { conditions: unknown[] }).conditions.length
        : 0
      const sortCount = Array.isArray(parameters.sort) ? parameters.sort.length : 0
      push(<span className="truncate">{t('studio.card.listRules', { filters: filterCount, sorts: sortCount })}</span>)
      if (typeof parameters.takeN === 'number') push(<span className="truncate">{t('studio.card.takeN', { count: parameters.takeN })}</span>)
      break
    }
    case 'sub_workflow': {
      const versionId = typeof parameters.workflowVersionId === 'string' ? parameters.workflowVersionId : undefined
      if (versionId) push(<span className="truncate">{t('studio.card.version', { value: versionId.slice(0, 8) })}</span>)
      break
    }
    case 'loop_over_items': {
      const errorMode = typeof parameters.errorMode === 'string' ? parameters.errorMode : 'terminate'
      const parallelism = typeof parameters.parallelism === 'number' ? parameters.parallelism : 10
      const LOOP_ERROR_MODE_LABELS = { terminate: t('studio.container.errorMode.terminate'), continue: t('studio.container.errorMode.continue'), remove: t('studio.container.errorMode.remove') } as const
      const errorModeLabel = LOOP_ERROR_MODE_LABELS[errorMode as keyof typeof LOOP_ERROR_MODE_LABELS] ?? LOOP_ERROR_MODE_LABELS.terminate
      push(<span className="truncate">{errorModeLabel}</span>)
      push(<span className="truncate">{t('studio.container.parallelChip', { count: parallelism })}</span>)
      break
    }
    default:
      break
  }
  return { rows, attachments: manifest.nodeType === 'agent' && bindings.length > 0 }
}

function bindingModelName(bindings: NodeBindingSummary[]) {
  return bindings.find((binding) => binding.resourceType === 'model')?.label
}

function inspectorModelName(data: ActionNodeData) {
  return data.resourceReferences.find((reference) => reference.resourceType === 'model')?.resourceId
}

function dynamicText(value: unknown) {
  if (typeof value === 'string') return value
  if (!value || typeof value !== 'object' || !('kind' in value)) return undefined
  const binding = value as InputBinding
  if (binding.kind === 'literal') return binding.value == null ? undefined : String(binding.value)
  if (binding.kind === 'template') return binding.segments.map((segment) => segment.kind === 'text' ? segment.text : '{{…}}').join('')
  return undefined
}

type BranchRowTone = 'logic' | 'blue' | 'muted'
type BranchRowSpec = { handleId: string; portName: string; tag: string; tone: BranchRowTone; label: string; sub?: string; badge?: string }

const DEFAULT_APPROVAL_BUTTON_IDS = ['approved', 'rejected'] as const
const MS_PER_HOUR = 3_600_000

/** Height projection of the branch-row rendering, shared with auto-layout and initial metrics. */
export function canvasBranchPorts(manifest: NodeManifest | undefined, parameters: Record<string, unknown> | undefined): { bodyRows?: number; branchRows?: number; inputRows?: number } {
  if (!manifest) return {}
  const branchRows = branchRowCount(manifest, parameters)
  const visible = visibleInputPorts(manifest, parameters ?? {})
  const inputRows = visible.length > 1 ? visible.length : 0
  if (!branchRows && !inputRows) return {}
  return branchRows ? { bodyRows: 0, branchRows, ...(inputRows ? { inputRows } : {}) } : { inputRows }
}

function branchRowCount(manifest: NodeManifest, parameters: Record<string, unknown> | undefined) {
  if (manifest.nodeType === 'if' && hasOutputPort(manifest, 'case')) {
    return parseIfCases(parameters ?? {}).length + (hasOutputPort(manifest, 'else') ? 1 : 0) + (hasOutputPort(manifest, 'error') ? 1 : 0)
  }
  if (manifest.nodeType === 'approval' && hasOutputPort(manifest, 'decision')) {
    const buttons = parseApprovalButtons(parameters ?? {})
    return (buttons?.length ?? DEFAULT_APPROVAL_BUTTON_IDS.length) + (hasOutputPort(manifest, 'timed_out') ? 1 : 0) + (hasOutputPort(manifest, 'error') ? 1 : 0)
  }
  return 0
}

function buildBranchRows(manifest: NodeManifest, parameters: Record<string, unknown>, t: TFunction): BranchRowSpec[] {
  const rows: BranchRowSpec[] = []
  if (manifest.nodeType === 'if' && hasOutputPort(manifest, 'case')) {
    parseIfCases(parameters).forEach((item, index) => {
      const label = item.labels.length > 0
        ? item.labels.map((label, position) => label ?? t('studio.card.conditionN', { index: position + 1 })).join(', ')
        : t('studio.card.conditionN', { index: 1 })
      rows.push({
        handleId: `case:${item.id}`,
        portName: 'case',
        tag: index === 0 ? t('studio.card.ifTag') : t('studio.card.elifTag'),
        tone: 'logic',
        label,
        badge: item.labels.length > 1 ? t(item.logicalOp === 'or' ? 'studio.card.or' : 'studio.card.and') : undefined,
      })
    })
    if (hasOutputPort(manifest, 'else')) rows.push({ handleId: 'else', portName: 'else', tag: t('studio.card.elseTag'), tone: 'muted', label: '', sub: t('studio.card.elseFallback') })
  }
  if (manifest.nodeType === 'approval' && hasOutputPort(manifest, 'decision')) {
    const buttons = parseApprovalButtons(parameters) ?? DEFAULT_APPROVAL_BUTTON_IDS.map((id): { id: string; label?: string } => ({ id }))
    for (const button of buttons) rows.push({
      handleId: `decision:${button.id}`,
      portName: 'decision',
      tag: t('studio.card.buttonTag'),
      tone: 'blue',
      label: button.label ?? defaultApprovalLabel(button.id, t),
    })
    if (hasOutputPort(manifest, 'timed_out')) rows.push({ handleId: 'timed_out', portName: 'timed_out', tag: t('studio.card.timeoutRow'), tone: 'muted', label: '', sub: timeoutHoursText(parameters.timeoutMs, t) })
  }
  if (rows.length > 0 && hasOutputPort(manifest, 'error')) rows.push({ handleId: 'error', portName: 'error', tag: t('studio.boundary.error'), tone: 'muted', label: '' })
  return rows
}

function hasOutputPort(manifest: NodeManifest, name: string) {
  return manifest.outputPorts.some((port) => port.name === name)
}

type IfCase = { id: string; logicalOp: 'and' | 'or'; labels: Array<string | undefined> }

/** Parses the `cases` parameter of if nodes: [{id, name, conditions: [{condition, label}], logicalOp}]. */
function parseIfCases(parameters: Record<string, unknown>): IfCase[] {
  const raw = parameters.cases
  if (!Array.isArray(raw)) return []
  const cases: IfCase[] = []
  for (const entry of raw) {
    if (!entry || typeof entry !== 'object' || typeof (entry as { id?: unknown }).id !== 'string' || !(entry as { id: string }).id) continue
    const conditions = Array.isArray((entry as { conditions?: unknown }).conditions) ? (entry as { conditions: unknown[] }).conditions : []
    cases.push({
      id: (entry as { id: string }).id,
      logicalOp: (entry as { logicalOp?: unknown }).logicalOp === 'or' ? 'or' : 'and',
      labels: conditions.map((condition) => {
        const label = condition && typeof condition === 'object' ? (condition as { label?: unknown }).label : undefined
        return typeof label === 'string' && label.trim() ? label : undefined
      }),
    })
  }
  return cases
}

/** Parses the `buttons` parameter of approval nodes: [{id, label}]; undefined means "use the approved/rejected default". */
function parseApprovalButtons(parameters: Record<string, unknown>): Array<{ id: string; label?: string }> | undefined {
  const raw = parameters.buttons
  if (!Array.isArray(raw)) return undefined
  const buttons: Array<{ id: string; label?: string }> = []
  for (const entry of raw) {
    if (!entry || typeof entry !== 'object' || typeof (entry as { id?: unknown }).id !== 'string' || !(entry as { id: string }).id) continue
    const label = (entry as { label?: unknown }).label
    buttons.push({ id: (entry as { id: string }).id, label: typeof label === 'string' && label.trim() ? label : undefined })
  }
  return buttons
}

function defaultApprovalLabel(id: string, t: TFunction) {
  return id === 'approved' ? t('studio.card.approveDefault') : id === 'rejected' ? t('studio.card.rejectDefault') : id
}

function timeoutHoursText(timeoutMs: unknown, t: TFunction) {
  if (typeof timeoutMs !== 'number' || !Number.isFinite(timeoutMs) || timeoutMs <= 0) return undefined
  const hours = timeoutMs / MS_PER_HOUR
  return t('studio.card.timeoutHours', { hours: Number.isInteger(hours) ? hours : Math.round(hours * 10) / 10 })
}

function AttachmentBadges({ bindings, translate }: { bindings: NodeBindingSummary[]; translate: TFunction }) {
  const counts = new Map<string, number>()
  for (const binding of bindings) counts.set(binding.resourceType, (counts.get(binding.resourceType) ?? 0) + 1)
  return <div className="studio-card-att flex h-6 shrink-0 flex-wrap items-center gap-1 px-3">
    {[...counts.entries()].map(([resourceType, count]) => <span className="studio-card-att-chip inline-flex h-5 max-w-full items-center gap-1 rounded border border-border bg-muted/60 px-1.5 text-[10px] leading-none text-muted-foreground" key={resourceType}>
      <NodeIcon className="size-3 shrink-0" iconKey={attachmentIcon(resourceType)} />
      <span className="min-w-0 truncate">{translate(`resourceGrants.resourceTypes.${resourceType}`)}{count > 1 ? ` ×${count}` : ''}</span>
    </span>)}
    <span className="studio-card-att-add inline-flex h-5 items-center rounded border border-dashed border-border-strong px-1.5 text-[10px] leading-none text-muted-foreground">{translate('studio.card.addAttachment')}</span>
  </div>
}

export type Placement = { position: Position; axis: number }

/** Branch/input row handle: same 3px bar visual as PortHandle, anchored at the row's vertical center. */
function RowHandle({ id, onHover, onQuickAdd, addLabel, style, title, type }: { id: string; onHover?: (active: boolean) => void; onQuickAdd?: () => void; addLabel?: string; style: CSSProperties; title: string; type: 'source' | 'target' }) {
  return <>
    <Handle className={cn('studio-handle !absolute !z-30 !grid !size-4 !place-items-center !border-0 !bg-transparent', type === 'target' ? 'studio-port-input' : 'studio-port-output')} id={id} onMouseEnter={() => onHover?.(true)} onMouseLeave={() => onHover?.(false)} position={type === 'target' ? Position.Left : Position.Right} style={style} title={title} type={type}><span className="studio-handle-mark block" /></Handle>
    {onQuickAdd && <button aria-label={addLabel} className="studio-port-add nodrag absolute left-full z-40 ml-7 grid size-6 -translate-y-1/2 place-items-center rounded-full border border-border bg-surface text-muted-foreground hover:border-primary hover:text-primary" onClick={(event) => { event.stopPropagation(); onQuickAdd() }} style={style} title={addLabel} type="button"><Plus className="size-3.5" /></button>}
  </>
}

export function PortHandle({ id, kind, label, placement, type, onQuickAdd, onHover, addLabel }: { id: string; kind: PortKind; label: string; placement: Placement; type: 'source' | 'target'; onQuickAdd?: () => void; onHover?: (active: boolean) => void; addLabel?: string }) {
  const vertical = placement.position === Position.Left || placement.position === Position.Right
  const handleStyle = vertical ? { top: `${placement.axis}%` } : { left: `${placement.axis}%` }
  const labelClass = placement.position === Position.Left
    ? 'right-full mr-3 flex h-4 -translate-y-1/2 items-center leading-none'
    : placement.position === Position.Right
      ? 'left-full ml-3 flex h-4 -translate-y-1/2 items-center leading-none'
      : kind === 'error'
        ? 'top-full ml-2 flex h-4 -translate-y-1/2 items-center leading-none'
        : 'top-full mt-3 w-16 -translate-x-1/2 text-center leading-none'
  const labelStyle = vertical ? { top: `${placement.axis}%` } : { left: `${placement.axis}%` }
  const colorClass = kind === 'error' ? 'studio-port-error' : type === 'target' ? 'studio-port-input' : 'studio-port-output'
  return <>
    <Handle className={cn('studio-handle !absolute !z-30 !grid !size-4 !place-items-center !border-0 !bg-transparent', colorClass)} id={id} onMouseEnter={() => onHover?.(true)} onMouseLeave={() => onHover?.(false)} position={placement.position} style={handleStyle} title={label} type={type}><span className={cn('studio-handle-mark block')} /></Handle>
    <span className={cn('studio-port-label pointer-events-none absolute z-20 max-w-24 truncate text-[9px] font-medium text-muted-foreground', labelClass)} data-port-id={id} data-port-type={type} style={labelStyle}>{label}</span>
    {onQuickAdd && <button aria-label={addLabel} className={cn('studio-port-add nodrag absolute z-40 grid size-6 place-items-center rounded-full border border-border bg-surface text-muted-foreground hover:border-primary hover:text-primary', placement.position === Position.Bottom ? 'top-full mt-7 -translate-x-1/2' : 'left-full ml-7 -translate-y-1/2')} onClick={(event) => { event.stopPropagation(); onQuickAdd() }} style={labelStyle} title={addLabel} type="button"><Plus className="size-3.5" /></button>}
  </>
}

function portPlacement(direction: 'input' | 'output', kind: PortKind, name: string, index: number, ports: { name: string; kind: PortKind }[]): Placement {
  if (direction === 'input') return { position: Position.Left, axis: ((index + 1) / (ports.length + 1)) * 100 }
  const orderedPorts = [...ports].sort((left, right) => Number(left.kind === 'error' || left.name === 'error') - Number(right.kind === 'error' || right.name === 'error'))
  const orderedIndex = orderedPorts.findIndex((port) => port.name === name && port.kind === kind)
  return { position: Position.Right, axis: ((orderedIndex + 1) / (ports.length + 1)) * 100 }
}

function roleIcon(role: CanvasNodeRole) {
  return ({ trigger: 'mouse-pointer-click', branch: 'split', merge: 'git-merge', loop: 'repeat-2', agent: 'bot', code: 'code-2', suspend: 'clock-3', approval: 'badge-check', sub_workflow: 'git-merge' } as Partial<Record<CanvasNodeRole, string>>)[role] ?? 'box'
}

function attachmentIcon(resourceType: string) { return ({ model: 'brain-circuit', mcp_tool: 'wrench', memory: 'memory-stick', rag: 'database', skill: 'sparkles' } as Record<string, string>)[resourceType] ?? 'box' }
