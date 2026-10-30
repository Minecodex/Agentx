import { Position, useUpdateNodeInternals, type NodeProps } from '@xyflow/react'
import { AlertTriangle } from 'lucide-react'
import { memo, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { cn } from '../../../shared/lib/cn'
import { localizedNodeLabel, localizeManifest } from '../model/manifest-localization'
import type { CanvasNodeRole, StudioNode } from '../model/types'
import { useCanvasRenderStore } from '../store/canvas-render-store'
import type { NodeBindingSummary } from '../utils/graph-index'
import { actionNodeGeometry } from './canvas-node-geometry'
import { cardContent } from './card-content'
import { CardShell } from './card-shell'
import { canvasNodeFamily, canvasNodeRole, nodeGroupColor } from './node-appearance'
import { NodeIcon } from './node-icon'
import { PluginCanvas } from './plugin-canvas'
import { PortHandle } from './port-handle'

export type { NodeBindingSummary } from '../utils/graph-index'

export const ManifestNode = memo(function ManifestNode({ id, data, selected }: NodeProps<StudioNode>) {
  const { t, i18n } = useTranslation()
  const manifest = useCanvasRenderStore(state => state.manifests.get(`node:${id}`) ?? (data.editorKind === 'action' ? state.manifests.get(`${data.nodeType}@${data.typeVersion}`) : undefined))
  const runtimeStatus = useCanvasRenderStore(state => state.runtimeStatuses.get(id))
  const bindings = useCanvasRenderStore(state => state.bindingSummaries.get(id) ?? EMPTY_BINDINGS)
  const occupiedSignature = useCanvasRenderStore(state => state.occupiedHandlesByNodeId.get(id) ?? '')
  const zoomTier = useCanvasRenderStore(state => state.zoomTier)
  const onQuickAdd = useCanvasRenderStore(state => state.onQuickAdd)
  const onSourceHover = useCanvasRenderStore(state => state.onSourceHover)
  const [hovered, setHovered] = useState(false)
  const updateNodeInternals = useUpdateNodeInternals()
  const occupied = useMemo(() => new Set(occupiedSignature.split('\u0001')), [occupiedSignature])
  const content = data.editorKind === 'action' ? cardContent(data, manifest, bindings.length ? bindings : undefined, t) : undefined
  const geometry = data.editorKind === 'action' ? actionNodeGeometry(data, manifest, content) : undefined
  const signature = geometry ? JSON.stringify(geometry) : ''
  useEffect(() => scheduleNodeInternalsUpdate(id, updateNodeInternals), [id, signature, updateNodeInternals])
  if (data.editorKind !== 'action' || !content || !geometry) return null
  const role = canvasNodeRole(manifest)
  const localized = manifest ? localizeManifest(manifest, i18n.language) : undefined
  const label = manifest ? localizedNodeLabel(manifest, data.label, i18n.language) : data.label || data.nodeType
  const displayTier = selected || hovered ? 'full' : zoomTier
  const outputRows = geometry.ports.filter(port => port.direction === 'output' && port.row !== undefined)
  const inputRows = geometry.ports.filter(port => port.direction === 'input' && port.row !== undefined)
  return <div className={cn('studio-node relative shrink-0', `studio-node-${canvasNodeFamily(role)}`, `studio-node-role-${role}`, `studio-node-zoom-${displayTier}`)} data-role={role} data-testid={`studio-node-${id}`} onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)} style={{ width: geometry.width, height: geometry.height }}>
    <CardShell group={nodeGroupColor(manifest?.nodeType ?? data.nodeType)} icon={<NodeIcon className="size-3.5" iconKey={manifest?.iconKey || roleIcon(role)} />} label={label} runtimeStatus={runtimeStatus} selected={selected}>
      {geometry.rowCount > 0 && <div className="studio-card-rows absolute inset-x-0" style={{ top: geometry.contentTop }}>{Array.from({ length: geometry.rowCount }, (_, index) => {
        const input = inputRows.find(port => port.row === index)
        const output = outputRows.find(port => port.row === index)
        const branch = content.branches.find(row => row.handleId === output?.id)
        return <div className={cn('studio-branch-row', input && !output && 'studio-branch-row-input')} key={index}>
          {input && <span className="studio-branch-text min-w-0 flex-1 truncate" title={localized?.inputPortLabel(input.portName) ?? input.id}>{localized?.inputPortLabel(input.portName) ?? input.id}</span>}
          {branch ? <span className="studio-branch-text flex min-w-0 flex-1 items-center gap-1.5" title={[branch.tag, branch.label, branch.sub].filter(Boolean).join(' ')}><span className={cn('studio-branch-tag shrink-0', `studio-branch-tag-${branch.tone}`)}>{branch.tag}</span><span className="min-w-0 truncate">{branch.label || branch.sub}</span>{branch.badge && <span className="studio-branch-badge shrink-0">{branch.badge}</span>}</span> : output && <span className="studio-branch-text min-w-0 flex-1 truncate text-right" title={localized?.outputPortLabel(output.portName) ?? output.id}>{localized?.outputPortLabel(output.portName) ?? output.id}</span>}
        </div>
      })}</div>}
      <div className="studio-card-body absolute inset-x-0 px-3" style={{ top: geometry.summaryTop }}>
        {content.summaries.map((row, index) => <div className={cn('studio-card-row flex items-center gap-1.5 text-muted-foreground', row.mono && 'font-mono')} key={index} title={[row.badge, row.text].filter(Boolean).join(' ')}>{row.badge && <span className="studio-summary-badge shrink-0">{row.badge}</span>}<span className="min-w-0 truncate">{row.text}</span></div>)}
        {manifest?.plugin && <PluginCanvas manifest={manifest} parameters={data.parameters} />}
      </div>
      {content.attachments.length > 0 && <div className="studio-card-att absolute inset-x-3 grid grid-cols-2 gap-x-1" style={{ top: geometry.attachmentTop }}>
        {content.attachments.map(({ resourceType, count }) => <span title={bindings.filter(binding => binding.resourceType === resourceType).map(binding => binding.label).join(', ')} className="studio-card-att-chip inline-flex h-5 min-w-0 items-center gap-1 truncate rounded border border-border bg-muted/60 px-1.5 text-[10px] text-muted-foreground" key={resourceType}><NodeIcon className="size-3 shrink-0" iconKey={attachmentIcon(resourceType)} /><span className="truncate">{t(`resourceGrants.resourceTypes.${resourceType}`)}{count > 1 ? ` ×${count}` : ''}</span></span>)}
        <span className="studio-card-att-add inline-flex h-5 items-center rounded border border-dashed border-border-strong px-1.5 text-[10px] text-muted-foreground">{t('studio.card.addAttachment')}</span>
      </div>}
    </CardShell>
    {geometry.ports.map(port => {
      const type = port.direction === 'input' ? 'target' : 'source'
      const branch = content.branches.find(row => row.handleId === port.id)
      const portLabel = branch ? branch.label || branch.tag : (type === 'target' ? localized?.inputPortLabel(port.portName) : localized?.outputPortLabel(port.portName)) ?? port.portName
      const quickAdd = type === 'source' && !data.disabled && onQuickAdd && (!occupied.has(port.id) || manifest?.outputPorts.find(p => p.name === port.portName)?.variadic)
      return <PortHandle addLabel={t('studio.ports.addAfter', { label: portLabel })} id={port.id} key={`${type}:${port.id}`} kind={port.kind} label={portLabel} onHover={type === 'source' && onSourceHover ? active => onSourceHover(id, port.id, active) : undefined} onQuickAdd={quickAdd ? () => onQuickAdd(id, port.id, port.portName) : undefined} placement={{ position: port.side === 'left' ? Position.Left : Position.Right, axis: port.y }} row={port.row !== undefined || port.kind !== 'error'} type={type} />
    })}
    {data.disabled && <AlertTriangle className="absolute -right-2 -top-2 z-30 size-5 rounded-full bg-surface p-0.5 text-warning" />}
  </div>
}, (previous, next) => previous.id === next.id && previous.data === next.data && previous.selected === next.selected)

const EMPTY_BINDINGS: NodeBindingSummary[] = []
let pendingInternals = new Set<string>()
let internalsUpdateScheduled = false
function scheduleNodeInternalsUpdate(id: string, update: (ids: string[]) => void) {
  pendingInternals.add(id)
  if (internalsUpdateScheduled) return
  internalsUpdateScheduled = true
  queueMicrotask(() => { const ids = [...pendingInternals]; pendingInternals = new Set(); internalsUpdateScheduled = false; update(ids) })
}
function roleIcon(role: CanvasNodeRole) { return ({ trigger: 'mouse-pointer-click', branch: 'split', merge: 'git-merge', loop: 'repeat-2', agent: 'bot', code: 'code-2', suspend: 'clock-3', approval: 'badge-check', sub_workflow: 'git-merge' } as Partial<Record<CanvasNodeRole, string>>)[role] ?? 'box' }
function attachmentIcon(type: string) { return ({ model: 'brain-circuit', mcp_tool: 'wrench', memory: 'memory-stick', rag: 'database', skill: 'sparkles' } as Record<string, string>)[type] ?? 'box' }
