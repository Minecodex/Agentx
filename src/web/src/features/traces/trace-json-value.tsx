import { ChevronRight } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { cn } from '../../shared/lib/cn'

/** JSON stays data: preserve strings and field order, and mount nested values on expansion. */
export function TraceJsonValue({ value, className }: { value: unknown; className?: string }) {
  return <div className={cn('max-h-96 overflow-auto rounded-lg border border-border bg-muted/20 p-3 font-mono text-[11px] leading-6', className)} data-testid="trace-json-value"><JsonValue depth={0} value={value} /></div>
}

function JsonValue({ value, name, depth }: { value: unknown; name?: string; depth: number }) {
  const { t } = useTranslation()
  const [expanded, setExpanded] = useState(depth === 0)
  const isArray = Array.isArray(value)
  const isObject = typeof value === 'object' && value !== null
  const entries = isObject ? Object.entries(value) : []
  const label = name === undefined ? null : <span className="text-primary">{JSON.stringify(name)}<span className="text-muted-foreground">: </span></span>
  if (!isObject || !entries.length) return <div className="break-words pl-5">{label}<span className={cn('whitespace-pre-wrap [overflow-wrap:anywhere]', typeof value === 'string' ? 'text-success' : typeof value === 'number' ? 'text-warning' : typeof value === 'boolean' ? 'text-primary' : 'text-muted-foreground')}>{JSON.stringify(value) ?? '—'}</span></div>
  const open = isArray ? '[' : '{'
  const close = isArray ? ']' : '}'
  const count = t(isArray ? 'trace.jsonItems' : 'trace.jsonProperties', { count: entries.length })
  return <div>
    <button aria-expanded={expanded} className="flex w-full items-start gap-1 rounded px-0.5 text-left outline-none hover:bg-muted/60 focus-visible:ring-2 focus-visible:ring-primary/30" onClick={() => setExpanded((current) => !current)} type="button">
      <ChevronRight className={cn('mt-1 size-3.5 shrink-0 text-muted-foreground transition-transform', expanded && 'rotate-90')} />
      <span className="min-w-0 break-words">{label}<span className="text-muted-foreground">{open}{!expanded && ` … ${close}`}</span><span className="ml-2 font-sans text-[10px] text-muted-foreground">{count}</span></span>
    </button>
    {expanded && <><div className="ml-2 border-l border-border pl-3">{entries.map(([key, field]) => <JsonValue depth={depth + 1} key={key} name={isArray ? undefined : key} value={field} />)}</div><div className="pl-5 text-muted-foreground">{close}</div></>}
  </div>
}
