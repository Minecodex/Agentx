import { useMutation, useQuery } from '@tanstack/react-query'
import { ChevronDown, ChevronUp, GitFork, GripHorizontal } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest } from '../../../shared/api/client'
import type { Checkpoint } from '../../../shared/api/types'
import { useLocaleFormat } from '../../../shared/lib/locale-format'
import { localizedValue } from '../../../shared/lib/localized-value'
import { Button } from '../../../shared/ui/button'
import { Select } from '../../../shared/ui/select'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../../shared/ui/tabs'
import { useToast } from '../../../shared/ui/toast'
import type { Execution, PageResponse } from '../../../shared/api/types'
import { TraceWorkspace } from '../../traces/trace-workspace'
import { useExecutionArtifactDownload } from '../../traces/use-execution-artifact-download'
import type { ExecutionEvent } from '../api/studio-api'
import { useModelDeltas } from '../api/use-model-deltas'

export function RuntimePanel({ workflowId, executionId, events, onExecutionChange, onNodeSelect }: { workflowId: string; executionId?: string; events: ExecutionEvent[]; onExecutionChange: (executionId: string) => void; onNodeSelect?: (nodeId: string) => void }) {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  const { showToast } = useToast()
  const downloadArtifact = useExecutionArtifactDownload(executionId)
  const [height, setHeight] = useState(220)
  const [collapsed, setCollapsed] = useState(!executionId)
  const [tab, setTab] = useState('events')
  const executions = useQuery({ queryKey: ['studio-executions', workflowId, executionId], queryFn: () => apiRequest<PageResponse<Execution>>(`/executions?workflowIds=${encodeURIComponent(workflowId)}&limit=100`), enabled: Boolean(workflowId), refetchInterval: (query) => { if (!executionId) return false; const execution = query.state.data?.items?.find((item: Execution) => item.id === executionId); if (!execution) return 1500; return !['succeeded', 'failed', 'timed_out', 'cancelled'].includes(execution.status) ? 3000 : false } })
  const selected = executions.data?.items?.find((item) => item.id === executionId)
  const streaming = useModelDeltas(executionId, Boolean(executionId) && (!selected || !['succeeded', 'failed', 'timed_out', 'cancelled'].includes(selected.status)))
  const checkpoints = useQuery({ queryKey: ['studio-checkpoints', executionId], queryFn: () => apiRequest<{ items: Checkpoint[] }>(`/executions/${executionId}/checkpoints`), enabled: Boolean(executionId), refetchInterval: () => { if (!executionId || !executions.data?.items) return false; const execution = executions.data.items.find((e: Execution) => e.id === executionId); return execution && !['succeeded', 'failed', 'timed_out', 'cancelled'].includes(execution.status) ? 1500 : false } })
  const fork = useMutation({ mutationFn: (checkpointId: string) => apiRequest<{ executionId: string }>(`/executions/${executionId}/fork`, { method: 'POST', body: JSON.stringify({ checkpointId, mode: 'whole', inputOverrides: {}, sideEffectDecisions: {}, idempotencyKey: crypto.randomUUID() }), headers: { 'Content-Type': 'application/json' } }), onSuccess: (value) => { onExecutionChange(value.executionId); showToast(t('studio.toasts.forkStarted')) }, onError: (error: Error) => showToast(error.message) })
  useEffect(() => { if (executionId) setCollapsed(false) }, [executionId])
  const executionOptions = (executions.data?.items ?? []).map((execution) => ({ value: execution.id, label: `${localizedValue(t, 'common', execution.status)} · ${formatDateTime(execution.startedAt)}` }))
  if (executionId && !executionOptions.some((option) => option.value === executionId)) executionOptions.unshift({ value: executionId, label: `${t('studio.runtime.currentExecutionUnavailable')} · ${executionId}` })
  const executionSelect = <div className="flex min-w-0 items-center gap-2" data-testid="execution-history"><span className="shrink-0 text-[10px] font-medium text-muted-foreground">{t('studio.runtime.executionHistory')}</span><Select aria-label={t('studio.runtime.selectExecution')} className="h-7 min-w-56 max-w-80 text-[10px]" disabled={executions.isLoading || !executionOptions.length} onValueChange={onExecutionChange} options={executionOptions} placeholder={executions.isLoading ? t('studio.runtime.loadingExecutions') : t('studio.runtime.noExecution')} value={executionId ?? ''} /></div>
  const beginResize = (event: React.PointerEvent) => {
    const startY = event.clientY; const startHeight = height
    const move = (next: PointerEvent) => setHeight(Math.max(160, Math.min(window.innerHeight * 0.8, startHeight + startY - next.clientY)))
    const end = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', end) }
    window.addEventListener('pointermove', move); window.addEventListener('pointerup', end)
  }
  if (collapsed) return <section className="relative flex h-10 shrink-0 items-center gap-3 border-t border-border bg-surface px-4" data-testid="runtime-rail"><button aria-label={t('studio.runtime.expand')} className="flex items-center gap-2 text-xs text-muted-foreground hover:text-foreground" onClick={() => setCollapsed(false)} type="button"><ChevronUp className="size-4" />{t('studio.runtime.title')}</button>{executionSelect}</section>
  const openTrace = () => setHeight((current) => Math.max(current, Math.min(560, window.innerHeight * 0.8)))
  return <section className="relative z-20 shrink-0 border-t border-border bg-surface" data-testid="runtime-rail" style={{ height }}>
    <button aria-label={t('studio.runtime.resize')} className="absolute -top-2 left-1/2 z-30 grid h-4 w-10 -translate-x-1/2 cursor-row-resize place-items-center rounded-sm border border-border bg-surface text-muted-foreground shadow-sm" onPointerDown={beginResize} type="button"><GripHorizontal className="size-3" /></button>
    <div className="h-full overflow-hidden px-4">
      <Tabs className="flex h-full min-h-0 flex-col" onValueChange={(value) => { setTab(value); if (value === 'trace') openTrace() }} value={tab}>
        <TabsList className="h-10 shrink-0 gap-5"><TabsTrigger className="text-xs" value="events">{t('studio.runtime.events')}</TabsTrigger><TabsTrigger className="text-xs" onClick={openTrace} value="trace">{t('studio.runtime.trace')}</TabsTrigger><TabsTrigger className="text-xs" value="checkpoints">{t('studio.runtime.checkpoints')}</TabsTrigger><span className="flex-1" /><Button aria-label={t('studio.runtime.collapse')} onClick={() => setCollapsed(true)} size="icon" variant="ghost"><ChevronDown className="size-3.5" /></Button></TabsList>
        <div className="flex h-11 shrink-0 items-center border-y border-border bg-muted/20 px-3" data-testid="execution-toolbar">{executionSelect}<span className="ml-3 text-[10px] text-muted-foreground">{t('studio.runtime.executionHistoryHint')}</span></div>
        <TabsContent className="min-h-0 flex-1 overflow-auto py-3" data-testid="execution-events" value="events">{streaming.tails.map((tail) => <article className="mb-3 rounded-md border border-border bg-muted/30 p-3" data-testid="model-stream-tail" key={tail.attemptId}><strong className="text-xs">{tail.nodeKey} · {t('studio.runtime.streamTail')}</strong>{tail.reasoningDeltaText && <details className="mt-2 text-xs text-muted-foreground"><summary>{t('studio.runtime.reasoning')}</summary><p className="whitespace-pre-wrap">{tail.reasoningDeltaText}</p></details>}<p className="mt-2 whitespace-pre-wrap break-words text-xs">{tail.deltaText}</p></article>)}{streaming.error && <p className="text-xs text-danger">{streaming.error}</p>}{events.map((event) => <div className="grid min-w-[560px] grid-cols-[60px_160px_100px_1fr] gap-3 border-b border-border py-1.5 text-[10px]" key={event.sequence}><span>#{event.sequence}</span><span>{event.eventType}</span><span>{localizedValue(t, 'common', event.status)}</span><span className="truncate font-mono text-muted-foreground">{JSON.stringify(event.summary)}</span></div>)}</TabsContent>
        <TabsContent className="min-h-0 flex-1 overflow-hidden pt-3" data-testid="execution-trace" value="trace">{executionId ? <TraceWorkspace active={tab === 'trace'} className="h-full min-h-0" executionId={executionId} onDownloadArtifact={downloadArtifact} onNodeSelect={(node) => onNodeSelect?.(node.nodeId)} /> : <p className="p-4 text-xs text-muted-foreground">{t('studio.runtime.noExecution')}</p>}</TabsContent>
        <TabsContent className="min-h-0 flex-1 overflow-auto py-2" data-testid="execution-checkpoints" value="checkpoints">{checkpoints.data?.items.map((checkpoint) => <div className="flex min-w-[420px] items-center border-b border-border py-2 text-[10px]" key={checkpoint.id}><div><strong>#{checkpoint.sequenceNumber} · {checkpoint.checkpointType}</strong><p className="mt-1 font-mono text-muted-foreground">{checkpoint.stateHash.slice(0, 24)}</p></div><span className="flex-1" /><Button disabled={fork.isPending} onClick={() => fork.mutate(checkpoint.id)} size="sm" variant="ghost"><GitFork className="size-3.5" />{t('studio.runtime.fork')}</Button></div>)}{checkpoints.data?.items.length === 0 && <p className="py-4 text-xs text-muted-foreground">{t('studio.runtime.noCheckpoints')}</p>}</TabsContent>
      </Tabs>
    </div>
  </section>
}
