import { useQuery } from '@tanstack/react-query'
import { Activity, AlertTriangle, Braces, LoaderCircle } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest } from '../../shared/api/client'
import type { Execution, NodeExecution } from '../../shared/api/types'
import { cn } from '../../shared/lib/cn'
import { formatCost } from '../../shared/lib/cost-format'
import { Button } from '../../shared/ui/button'
import { TraceNodeView } from './trace-node-view'
import { JsonBlock, primaryText } from './trace-semantic-value'
import { TraceWaterfall } from './trace-waterfall'
import { duration, statusTone } from './trace-model'
import { Badge } from '../../shared/ui/badge'

type Props = {
  executionId: string
  className?: string
  active?: boolean
  execution?: Execution
  nodes?: NodeExecution[]
  onNodeSelect?: (node: NodeExecution) => void
  onDownloadArtifact?: (artifactId: string) => void
}

export function TraceWorkspace({ executionId, className, active = true, execution: suppliedExecution, nodes: suppliedNodes, onNodeSelect, onDownloadArtifact }: Props) {
  const { t } = useTranslation()
  const [view, setView] = useState<'nodes' | 'waterfall'>('nodes')
  const [showRaw, setShowRaw] = useState(false)
  const executionQuery = useQuery({ queryKey: ['execution', executionId], queryFn: () => apiRequest<Execution>(`/executions/${executionId}`), enabled: active && !suppliedExecution, retry: false, refetchInterval: (query) => !query.state.data || !['succeeded', 'failed', 'timed_out', 'cancelled'].includes(query.state.data.status) ? 1500 : false })
  const execution = suppliedExecution ?? executionQuery.data
  const nodesQuery = useQuery({ queryKey: ['execution-nodes', executionId], queryFn: () => apiRequest<{ items: NodeExecution[] }>(`/executions/${executionId}/nodes`), enabled: active && !suppliedNodes, retry: false, refetchInterval: (query) => !execution || !['succeeded', 'failed', 'timed_out', 'cancelled'].includes(execution.status) || !query.state.data || query.state.data.items.some((node) => !['succeeded', 'failed', 'timed_out', 'cancelled', 'skipped'].includes(node.status)) ? 1500 : false })
  const nodes = suppliedNodes ?? nodesQuery.data?.items
  if (!execution || !nodes) {
    const error = executionQuery.error ?? nodesQuery.error
    return <div className={cn('grid min-h-64 place-items-center p-6 text-center text-xs', error ? 'text-danger' : 'text-muted-foreground', className)}>{error ? <span><AlertTriangle className="mx-auto mb-2 size-5" />{t('trace.authoritativeDataUnavailable')}</span> : <span><LoaderCircle className="mx-auto mb-2 size-5 animate-spin" />{t('trace.loadingExecutionData')}</span>}</div>
  }
  return <section className={cn('flex min-h-0 flex-col overflow-hidden bg-background', className)} data-testid="trace-workspace">
    <ExecutionResult execution={execution} onToggleRaw={() => setShowRaw((value) => !value)} showRaw={showRaw} />
    <div className="mx-3 mt-3 flex min-h-12 shrink-0 flex-wrap items-center gap-2 rounded-t-xl border border-border bg-surface px-3 py-2">
      <div className="inline-flex rounded-lg bg-muted p-1" role="tablist" aria-label={t('trace.views.label')}><button aria-selected={view === 'nodes'} className={cn('h-7 rounded-md px-3 text-[11px] text-muted-foreground', view === 'nodes' && 'bg-surface text-primary shadow-sm')} onClick={() => setView('nodes')} role="tab" type="button">{t('trace.views.nodes')}</button><button aria-selected={view === 'waterfall'} className={cn('h-7 rounded-md px-3 text-[11px] text-muted-foreground', view === 'waterfall' && 'bg-surface text-primary shadow-sm')} onClick={() => setView('waterfall')} role="tab" type="button">{t('trace.views.waterfall')}</button></div>
      <span className="text-[10px] text-muted-foreground">{t(view === 'nodes' ? 'trace.views.nodesHint' : 'trace.views.waterfallHint')}</span><span className="flex-1" />
      <Button aria-pressed={showRaw} onClick={() => setShowRaw((value) => !value)} size="sm" variant="ghost"><Braces className="size-3.5" />{t(showRaw ? 'trace.hideFinalJson' : 'trace.showFinalJson')}</Button>
    </div>
    <div className="mx-3 mb-3 min-h-0 flex-1 overflow-hidden rounded-b-xl border border-t-0 border-border bg-surface">{view === 'nodes' ? <TraceNodeView execution={execution} nodes={nodes} onDownloadArtifact={onDownloadArtifact} onNodeSelect={onNodeSelect} /> : <TraceWaterfall active={active} className="h-full min-h-0" executionId={executionId} onDownloadArtifact={onDownloadArtifact} onNodeSelect={(nodeExecutionId) => { const node = nodes.find((item) => item.id === nodeExecutionId); if (node) onNodeSelect?.(node) }} />}</div>
  </section>
}

function ExecutionResult({ execution, showRaw, onToggleRaw }: { execution: Execution; showRaw: boolean; onToggleRaw: () => void }) {
  const { t } = useTranslation()
  const output = primaryText(execution.output)
  return <article className="mx-3 mt-3 shrink-0 overflow-hidden rounded-xl border border-border bg-surface shadow-sm" data-testid="trace-final-output"><div className="grid grid-cols-[minmax(280px,1.8fr)_repeat(4,minmax(90px,.45fr))] max-[900px]:grid-cols-2">
    <button className="min-w-0 p-4 text-left max-[900px]:col-span-2 max-[900px]:border-b max-[900px]:border-border" onClick={onToggleRaw} type="button"><span className="flex items-center gap-2 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground"><Activity className="size-3.5 text-primary" />{t('trace.finalOutput')}</span><strong className="mt-2 block truncate text-sm">{output ?? t('trace.structuredFinalOutput')}</strong></button>
    <Metric label={t('trace.status')} value={<Badge tone={statusTone(execution.status)}>{t(`common.${execution.status}`, { defaultValue: execution.status })}</Badge>} />
    <Metric label={t('trace.totalDuration')} value={duration(execution.durationMs)} />
    <Metric label={t('trace.tokens')} value={`${execution.inputTokens ?? 0} / ${execution.outputTokens ?? 0}`} />
    <Metric label={t('trace.cost')} value={formatCost(execution.costMicros, execution.costCurrency)} />
  </div>{showRaw && <div className="border-t border-border bg-muted/20 p-3"><JsonBlock value={execution.output} /></div>}</article>
}

function Metric({ label, value }: { label: string; value: React.ReactNode }) { return <div className="border-l border-border p-4 max-[900px]:border-b"><span className="block text-[10px] text-muted-foreground">{label}</span><strong className="mt-2 block text-xs">{value}</strong></div> }
