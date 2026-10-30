import { Check, LoaderCircle, X } from 'lucide-react'
import type { ReactNode } from 'react'
import { cn } from '../../../shared/lib/cn'

export function CardShell({ label, icon, group, selected, runtimeStatus, trailing, children, className }: { label: string; icon: ReactNode; group: string; selected?: boolean; runtimeStatus?: string; trailing?: ReactNode; children?: ReactNode; className?: string }) {
  const stateClass = selected ? 'studio-node-selected' : ['running', 'succeeded', 'failed'].includes(runtimeStatus ?? '') ? `studio-node-${runtimeStatus}` : undefined
  return <div className={cn('studio-card relative size-full overflow-hidden border border-border bg-surface text-foreground', stateClass, className)}>
    <header className="studio-card-head drag-handle relative flex shrink-0 items-center gap-2 px-3">
      <span className="studio-card-icon grid size-6 shrink-0 place-items-center rounded-md text-white" style={{ backgroundColor: group }}>{icon}</span>
      <strong className="studio-card-title min-w-0 flex-1 truncate font-semibold leading-none" title={label}>{label}</strong>
      {trailing}
      {runtimeStatus === 'succeeded' ? <Check aria-hidden className="studio-card-status size-3.5 shrink-0 text-success" /> : runtimeStatus === 'failed' ? <X aria-hidden className="studio-card-status size-3.5 shrink-0 text-danger" /> : runtimeStatus === 'running' ? <LoaderCircle aria-hidden className="studio-card-status size-3.5 shrink-0 animate-spin text-warning" /> : undefined}
    </header>
    {children}
  </div>
}
