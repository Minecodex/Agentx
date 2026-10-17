import { Download } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '../../shared/ui/button'
import { MarkdownContent } from '../../shared/ui/markdown-content'
import { cn } from '../../shared/lib/cn'
import { TraceJsonValue } from './trace-json-value'

type ItemValue = { json: unknown; binary?: Record<string, unknown>; lineage?: unknown[]; metadata?: Record<string, unknown> }
type PortGroup = { port: string; items: ItemValue[] }

const semanticPriority = ['text', 'result', 'answer', 'stdout', 'stderr', 'statusCode', 'body', 'structuredOutput', 'documents', 'records', 'files', 'citations', 'usage', 'finishReason', 'partial']

export function TraceSemanticValue({ value, className, empty, onDownloadArtifact }: { value: unknown; className?: string; empty?: string; onDownloadArtifact?: (artifactId: string) => void }) {
  const { t } = useTranslation()
  const groups = portGroups(value)
  if (value == null) return <p className={cn('text-xs text-muted-foreground', className)}>{empty ?? t('trace.noBusinessContent')}</p>
  if (groups) return <div className={cn('space-y-3', className)}>{groups.map((group) => <PortItems group={group} key={group.port} onDownloadArtifact={onDownloadArtifact} />)}</div>
  return <SemanticFields className={className} onDownloadArtifact={onDownloadArtifact} value={value} />
}

export function SemanticFields({ value, className, onDownloadArtifact }: { value: unknown; className?: string; onDownloadArtifact?: (artifactId: string) => void }) {
  const { t } = useTranslation()
  if (!isRecord(value)) return <Value value={value} />
  const entries = orderedEntries(value)
  if (!entries.length) return <p className="text-xs text-muted-foreground">{t('trace.emptyObject')}</p>
  return <dl className={cn('grid gap-2.5', className)}>{entries.map(([key, field]) => <div className="grid grid-cols-[minmax(92px,128px)_minmax(0,1fr)] items-start gap-3" key={key}>
    <dt className="break-words text-[10px] text-muted-foreground">{key}</dt>
    <dd className="min-w-0 break-words text-xs text-foreground"><Value onDownloadArtifact={onDownloadArtifact} value={field} /></dd>
  </div>)}</dl>
}

export function JsonBlock({ value, className }: { value: unknown; className?: string }) {
  return <TraceJsonValue className={className} value={value} />
}

export function primaryText(value: unknown): string | undefined {
  if (typeof value === 'string') return value
  if (isItem(value)) return primaryText(value.json)
  if (Array.isArray(value)) return value.length === 1 ? primaryText(value[0]) : undefined
  if (!isRecord(value)) return undefined
  for (const key of ['result', 'text', 'answer', 'output', 'body']) {
    const result = value[key]
    if (typeof result === 'string') return result
  }
  const groups = portGroups(value)
  if (groups?.length === 1 && groups[0].items.length === 1) return primaryText(groups[0].items[0].json)
  const values = Object.values(value)
  return values.length === 1 ? primaryText(values[0]) : undefined
}

function PortItems({ group, onDownloadArtifact }: { group: PortGroup; onDownloadArtifact?: (artifactId: string) => void }) {
  const { t } = useTranslation()
  const label = normalizedPort(group.port)
  return <section className="overflow-hidden rounded-lg border border-border">
    <header className="flex items-center justify-between bg-muted/35 px-3 py-2 text-[10px]"><strong>{label}</strong><span className="text-muted-foreground">{t('trace.itemCount', { count: group.items.length })}</span></header>
    {group.items.length === 1 ? <div className="p-3"><SemanticFields onDownloadArtifact={onDownloadArtifact} value={group.items[0].json} />{hasAdvancedItemData(group.items[0]) && <RawItem item={group.items[0]} />}</div> : <ol className="divide-y divide-border">{group.items.map((item, index) => <li className="p-3" key={index}><details><summary className="cursor-pointer text-[11px] font-medium">{t('trace.itemIndex', { index: index + 1 })}</summary><div className="mt-3"><SemanticFields onDownloadArtifact={onDownloadArtifact} value={item.json} />{hasAdvancedItemData(item) && <RawItem item={item} />}</div></details></li>)}</ol>}
  </section>
}

function RawItem({ item }: { item: ItemValue }) {
  const { t } = useTranslation()
  return <details className="mt-3 border-t border-border pt-2"><summary className="cursor-pointer text-[10px] text-muted-foreground">{t('trace.rawItem')}</summary><JsonBlock className="mt-2" value={item} /></details>
}

function Value({ value, onDownloadArtifact }: { value: unknown; onDownloadArtifact?: (artifactId: string) => void }) {
  const { t } = useTranslation()
  if (value === null) return <span className="text-muted-foreground">null</span>
  if (value === undefined) return <span className="text-muted-foreground">—</span>
  if (typeof value === 'string') return <MarkdownContent>{value}</MarkdownContent>
  if (typeof value === 'number' || typeof value === 'boolean') return <span>{String(value)}</span>
  if (isRecord(value) && typeof value.artifactId === 'string' && onDownloadArtifact) return <Button onClick={() => onDownloadArtifact(value.artifactId as string)} size="sm" variant="secondary"><Download className="size-3.5" />{t('trace.downloadArtifact')}</Button>
  if (isRecord(value) && isUsage(value)) return <span>{t('trace.usageValue', { input: value.inputTokens ?? 0, output: value.outputTokens ?? 0, total: value.totalTokens ?? 0 })}</span>
  return <JsonBlock value={value} />
}

function orderedEntries(value: Record<string, unknown>) {
  const priority = new Map(semanticPriority.map((key, index) => [key, index]))
  return Object.entries(value).sort(([left], [right]) => (priority.get(left) ?? 1_000) - (priority.get(right) ?? 1_000) || left.localeCompare(right))
}

function portGroups(value: unknown): PortGroup[] | undefined {
  if (Array.isArray(value) && value.every(isItem)) return [{ port: 'main', items: value }]
  if (!isRecord(value)) return undefined
  const entries = Object.entries(value)
  if (!entries.length || !entries.every(([, items]) => Array.isArray(items) && items.every(isItem))) return undefined
  return entries.map(([port, items]) => ({ port, items: items as ItemValue[] }))
}

function normalizedPort(port: string) { return port.replace(/:\d+$/, '') }
function isUsage(value: Record<string, unknown>) { return ['inputTokens', 'outputTokens', 'totalTokens'].some((key) => key in value) }
function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === 'object' && value !== null && !Array.isArray(value) }
function isItem(value: unknown): value is ItemValue { return isRecord(value) && 'json' in value }
function hasAdvancedItemData(item: ItemValue) { return Boolean(Object.keys(item.binary ?? {}).length || item.lineage?.length || Object.keys(item.metadata ?? {}).length) }
