import { useQueries } from '@tanstack/react-query'
import { AlertTriangle, Bot, Box, ChevronDown, LoaderCircle } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import type { NodeExecution, TraceSpan, TraceSpanDetail } from '../../shared/api/types'
import { useNodeNames } from '../workflow-designer/api/node-display'
import { Badge } from '../../shared/ui/badge'
import { cn } from '../../shared/lib/cn'
import { formatCost } from '../../shared/lib/cost-format'
import { TraceContents, contentPreview } from './trace-content-detail'
import { TraceSemanticValue } from './trace-semantic-value'
import { duration, statusTone } from './trace-model'
import { traceSpanDetailQuery, useExecutionTrace } from './use-trace'

export function TraceNodeCard({ executionId, node, defaultExpanded = false, onNodeSelect, onDownloadArtifact }: { executionId: string; node: NodeExecution; defaultExpanded?: boolean; onNodeSelect?: (node: NodeExecution) => void; onDownloadArtifact?: (artifactId: string) => void }) {
  const { t } = useTranslation()
  const { resolveNodeName } = useNodeNames()
  const [expanded, setExpanded] = useState(defaultExpanded)
  const elapsed = nodeDuration(node)
  const modelLike = /model|agent/i.test(`${node.nodeType} ${node.capability}`)
  return <article className={cn('relative ml-14 overflow-visible rounded-xl border border-border bg-surface transition-shadow', expanded && 'border-primary/35 shadow-sm')} data-testid="trace-node-card">
    <div className="absolute -left-14 top-3.5 flex w-12 justify-center"><span className="grid size-9 place-items-center rounded-lg border-4 border-surface bg-primary/10 text-primary shadow-[0_0_0_1px_var(--color-border)]">{modelLike ? <Bot className="size-4" /> : <Box className="size-4" />}</span></div>
    <button aria-expanded={expanded} className="grid w-full grid-cols-[minmax(160px,1fr)_auto_auto] items-center gap-4 px-4 py-3 text-left" onClick={() => { setExpanded((value) => !value); onNodeSelect?.(node) }} type="button">
      <span className="min-w-0"><strong className="block truncate text-sm">{resolveNodeName(node.nodeName, node.nodeType)}</strong><small className="mt-0.5 block truncate text-[10px] text-muted-foreground">{node.nodeType} · {t('trace.runLabel', { run: node.runIndex, iteration: node.iterationIndex })} · {duration(elapsed)}{modelLike && node.costCurrency ? ` · ${formatCost(node.costMicros, node.costCurrency)}` : ''}</small></span>
      <Badge tone={statusTone(node.status)}>{t(`common.${node.status}`, { defaultValue: node.status })}</Badge>
      <ChevronDown className={cn('size-4 text-muted-foreground transition-transform', expanded && 'rotate-180')} />
    </button>
    {expanded && <TraceNodeDetails active executionId={executionId} node={node} onDownloadArtifact={onDownloadArtifact} />}
  </article>
}

export function TraceNodeDetails({ executionId, node, active = true, onDownloadArtifact }: { executionId: string; node: NodeExecution; active?: boolean; onDownloadArtifact?: (artifactId: string) => void }) {
  const { t } = useTranslation()
  const trace = useExecutionTrace(executionId, active, node.id)
  const detailQueries = useQueries({ queries: trace.spans.filter((span) => span.hasDetails).map((span) => ({
    ...traceSpanDetailQuery(executionId, span, trace.trace?.ingestedWatermark),
    enabled: active,
  })) })
  const details = useMemo(() => new Map(detailQueries.flatMap((query) => query.data ? [[query.data.span.spanId, query.data] as const] : [])), [detailQueries])
  const resolved = [...details.values()].map((detail) => contentPreview(detail, 'resolved_parameters')).find((value) => value !== undefined)
  return <div className="border-t border-border">
    <div className="divide-y divide-border" data-testid="trace-node-io">
      <NodeContentSection title={t('trace.businessInputAndParameters')}><div className="space-y-4"><div><h4 className="mb-2 text-xs text-muted-foreground">{t('trace.upstreamItems')}</h4><TraceSemanticValue empty={t('trace.authoritativeInputEmpty')} onDownloadArtifact={onDownloadArtifact} value={node.input} /></div><details className="rounded-lg border border-border p-3"><summary className="cursor-pointer text-xs font-medium text-muted-foreground">{t('trace.resolvedParameters')}</summary><div className="mt-3">{resolved !== undefined ? <TraceSemanticValue value={resolved} /> : <DiagnosticPending detailQueries={detailQueries} trace={trace} />}</div></details></div></NodeContentSection>
      <NodeContentSection title={t('trace.semanticOutput')}><TraceSemanticValue empty={node.errorCode ? `${node.errorCode}: ${node.errorMessage ?? ''}` : t('trace.authoritativeOutputEmpty')} onDownloadArtifact={onDownloadArtifact} value={node.output} /></NodeContentSection>
    </div>
    <section className="border-t border-border bg-muted/20 p-4"><div className="mb-3 flex items-center gap-2"><h3 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">{t('trace.internalProcess')}</h3><span className="text-[10px] text-muted-foreground">{t('trace.internalProcessHint')}</span></div><InternalSpans details={details} executionId={executionId} onDownloadArtifact={onDownloadArtifact} trace={trace} /></section>
  </div>
}

function NodeContentSection({ title, children }: { title: string; children: React.ReactNode }) {
  const [expanded, setExpanded] = useState(false)
  return <section className="min-w-0">
    <h3><button aria-expanded={expanded} className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left text-xs font-semibold outline-none hover:bg-muted/30 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary/30" onClick={() => setExpanded((current) => !current)} type="button">{title}<ChevronDown className={cn('size-4 shrink-0 text-muted-foreground transition-transform', expanded && 'rotate-180')} /></button></h3>
    {expanded && <div className="px-4 pb-4">{children}</div>}
  </section>
}

function InternalSpans({ trace, details, executionId, onDownloadArtifact }: { trace: ReturnType<typeof useExecutionTrace>; details: Map<string, TraceSpanDetail>; executionId: string; onDownloadArtifact?: (artifactId: string) => void }) {
  const { t } = useTranslation()
  const { resolveSpanName } = useNodeNames()
  const [open, setOpen] = useState<Set<string>>(new Set())
  if (trace.isLoading) return <DiagnosticMessage icon={<LoaderCircle className="size-3.5 animate-spin" />} text={t('trace.syncing')} />
  if (trace.isError) return <DiagnosticMessage icon={<AlertTriangle className="size-3.5" />} text={t('trace.diagnosticUnavailable')} tone="warning" />
  const nodeSpanIds = new Set(trace.spans.filter((span) => span.spanKind === 'node').map((span) => span.spanId))
  const internal = trace.spans.filter((span) => span.spanKind !== 'node')
  if (!internal.length) return <p className="text-[10px] text-muted-foreground">{trace.trace?.complete === false ? t('trace.syncing') : t('trace.noInternalSpans')}</p>
  const byId = new Map(trace.spans.map((span) => [span.spanId, span]))
  return <>{trace.trace?.complete === false && <DiagnosticMessage icon={<LoaderCircle className="size-3.5 animate-spin" />} text={t('trace.syncing')} />}<div className={cn('space-y-2 border-l border-primary/25 pl-4', trace.trace?.complete === false && 'mt-3')}>{internal.map((span) => {
    const depth = internalDepth(span, byId, nodeSpanIds)
    const detail = details.get(span.spanId)
    const expanded = open.has(span.spanId)
    return <article className="relative rounded-lg border border-border bg-surface" key={span.spanId} style={{ marginLeft: depth * 14 }}><span className="absolute -left-[17px] top-4 w-4 border-t border-primary/25" /><button aria-expanded={expanded} className="grid w-full grid-cols-[minmax(150px,1fr)_auto_auto_auto] items-center gap-3 px-3 py-2 text-left" onClick={() => setOpen((current) => toggle(current, span.spanId))} type="button"><span className="min-w-0"><strong className="block truncate text-[11px]">{resolveSpanName(span.spanName)}</strong><small className="text-[9px] text-muted-foreground">{t(`trace.kinds.${span.spanKind}`)}</small></span><Badge className="text-[9px]" tone={statusTone(span.status)}>{t(`common.${span.status}`, { defaultValue: span.status })}</Badge><span className="text-[9px] text-muted-foreground">{duration(span.durationMs)}</span><ChevronDown className={cn('size-3.5 text-muted-foreground transition-transform', expanded && 'rotate-180')} /></button>{expanded && <div className="border-t border-border p-3">{detail ? <TraceContents contents={detail.contents} executionId={executionId} onDownloadArtifact={onDownloadArtifact} /> : <p className="text-[10px] text-muted-foreground">{span.hasDetails ? t('trace.syncing') : t('trace.lifecycleOnly')}</p>}</div>}</article>
  })}</div></>
}

function DiagnosticPending({ trace, detailQueries }: { trace: ReturnType<typeof useExecutionTrace>; detailQueries: ReadonlyArray<{ isError: boolean; isLoading: boolean }> }) {
  const { t } = useTranslation()
  if (trace.isLoading || trace.trace?.complete === false || detailQueries.some((query) => query.isLoading)) return <span className="text-[10px] text-muted-foreground">{t('trace.syncing')}</span>
  if (trace.isError || detailQueries.some((query) => query.isError)) return <span className="text-[10px] text-warning">{t('trace.diagnosticUnavailable')}</span>
  return <span className="text-[10px] text-muted-foreground">{t('trace.noResolvedParameters')}</span>
}

function DiagnosticMessage({ icon, text, tone = 'muted' }: { icon: React.ReactNode; text: string; tone?: 'muted' | 'warning' }) { return <p className={cn('flex items-center gap-2 text-[10px]', tone === 'warning' ? 'text-warning' : 'text-muted-foreground')}>{icon}{text}</p> }
function toggle(current: Set<string>, id: string) { const next = new Set(current); if (next.has(id)) next.delete(id); else next.add(id); return next }
function internalDepth(span: TraceSpan, byId: Map<string, TraceSpan>, nodeSpanIds: Set<string>) { let depth = 0; let parent = span.parentSpanId; while (parent && !nodeSpanIds.has(parent)) { const value = byId.get(parent); if (!value) break; depth += 1; parent = value.parentSpanId } return depth }
function nodeDuration(node: NodeExecution) { if (!node.startedAt) return null; return Math.max(0, (node.endedAt ? new Date(node.endedAt).getTime() : Date.now()) - new Date(node.startedAt).getTime()) }
