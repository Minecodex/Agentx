import type { LucideIcon } from 'lucide-react'

export function EntityCell({ icon: Icon, name, detail }: { icon: LucideIcon; name: string; detail?: string }) {
  return (
    <div className="flex min-w-0 items-center gap-3">
      <span className="grid size-9 shrink-0 place-items-center rounded-lg bg-primary/10 text-primary"><Icon className="size-4" /></span>
      <span className="min-w-0 flex-1"><strong className="block truncate text-xs text-foreground" title={name}>{name}</strong>{detail && <span className="mt-1 block truncate text-[10px] text-muted-foreground" title={detail}>{detail}</span>}</span>
    </div>
  )
}
