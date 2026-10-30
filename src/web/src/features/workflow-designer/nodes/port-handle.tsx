import { Handle, Position, useStore } from '@xyflow/react'
import { Plus } from 'lucide-react'
import { cn } from '../../../shared/lib/cn'
import type { PortKind } from '../model/types'

export type Placement = { position: Position; axis: number }

/** Handle axes are pixels. The 8px visual sits inside a 20px hit target. */
export function PortHandle({ id, kind, label, placement, type, onQuickAdd, onHover, addLabel, row, testId, connectable = true }: { id: string; kind: PortKind; label: string; placement: Placement; type: 'source' | 'target'; onQuickAdd?: () => void; onHover?: (active: boolean) => void; addLabel?: string; row?: boolean; testId?: string; connectable?: boolean }) {
  const left = placement.position === Position.Left
  const canvasConnectable = useStore(state => state.nodesConnectable)
  const enabled = connectable && canvasConnectable
  const colorClass = kind === 'error' ? 'studio-port-error' : type === 'target' ? 'studio-port-input' : 'studio-port-output'
  return <>
    <Handle aria-label={label} className={cn('studio-handle !absolute !z-30 !grid !size-2 !place-items-center !border-0 !bg-transparent', colorClass)} data-testid={testId} data-port-id={id} data-port-kind={kind} data-port-type={type} id={id} isConnectable={enabled} isConnectableStart={enabled} isConnectableEnd={enabled} onMouseEnter={() => onHover?.(true)} onMouseLeave={() => onHover?.(false)} position={placement.position} style={{ top: placement.axis }} title={label} type={type}><span className="studio-handle-mark block" /></Handle>
    {!row && <span className={cn('studio-port-label pointer-events-none absolute z-20 max-w-20 -translate-y-1/2 truncate text-[10px] leading-none', left ? 'left-3' : 'right-3', kind === 'error' ? 'text-danger' : 'text-muted-foreground')} data-port-id={id} data-port-type={type} style={{ top: placement.axis }}>{label}</span>}
    {onQuickAdd && <button aria-label={addLabel} className="studio-port-add nodrag nopan absolute left-full z-40 ml-3 grid size-6 -translate-y-1/2 place-items-center rounded-full border border-border bg-surface text-muted-foreground hover:border-primary hover:text-primary" onClick={event => { event.stopPropagation(); onQuickAdd() }} style={{ top: placement.axis }} title={addLabel} type="button"><Plus className="size-3.5" /></button>}
  </>
}
