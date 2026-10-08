import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Ban, ExternalLink, Play } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useParams, useSearchParams } from 'react-router-dom'

import { apiRequest } from '../../shared/api/client'
import type { EvaluationReport } from '../../shared/api/types'
import { ConfirmDialog } from '../../shared/components/confirm-dialog'
import { EmptyState } from '../../shared/components/empty-state'
import { MetricCard } from '../../shared/components/metric-card'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { StatusBadge } from '../../shared/components/status-badge'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { localizedValue } from '../../shared/lib/localized-value'
import { Badge } from '../../shared/ui/badge'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../shared/ui/tabs'
import { EvaluationComparison } from './evaluation-comparison'
import { useToast } from '../../shared/ui/toast'

const activeStatuses = new Set(['queued', 'running'])

export function EvaluationDetailPage() {
  const { id = '' } = useParams()
  const [params, setParams] = useSearchParams()
  const tab = params.get('tab') === 'compare' ? 'compare' : 'report'
  const compareIds = [...new Set((params.get('runIds') ?? id).split(',').filter(Boolean))].slice(0, 5)
  const { t } = useTranslation()
  const { formatNumber } = useLocaleFormat()
  const { showToast } = useToast()
  const queryClient = useQueryClient()
  const [confirmCancel, setConfirmCancel] = useState(false)
  const report = useQuery({ queryKey: ['evaluation-report', id], queryFn: () => apiRequest<EvaluationReport>(`/evaluations/${id}/report`), refetchInterval: (query) => query.state.data && activeStatuses.has(query.state.data.run.status) ? 1_000 : false })
  const action = useMutation({ mutationFn: (name: 'start' | 'cancel') => apiRequest(`/evaluations/${id}/${name}`, { method: 'POST' }), onSuccess: async () => { setConfirmCancel(false); await queryClient.invalidateQueries({ queryKey: ['evaluation-report', id] }) }, onError: (error: Error) => showToast(error.message) })
  if (!report.data) return <PageContainer><EmptyState title={report.isLoading ? t('common.loading') : t('common.loadFailed')} description={String(report.error ?? '')} /></PageContainer>
  const { run, results, metrics } = report.data
  const metricValue = (key: string) => getMetricValue(metrics, key)
  const actionBar = run.status === 'created' ? <Button disabled={action.isPending} onClick={() => action.mutate('start')}><Play className="size-4" />{t('evaluations.start')}</Button> : activeStatuses.has(run.status) ? <Button disabled={action.isPending} onClick={() => setConfirmCancel(true)} variant="secondary"><Ban className="size-4" />{t('common.cancel')}</Button> : undefined

  return <PageContainer>
    <PageHeader action={actionBar} description={`${run.workflowName} · ${run.datasetName}`} title={run.name} />
    <Tabs className="mt-5" value={tab} onValueChange={(value) => setParams((current) => { const next = new URLSearchParams(current); next.set('tab', value); return next })}><TabsList><TabsTrigger value="report">{t('evaluations.report')}</TabsTrigger><TabsTrigger value="compare">{t('evaluations.compare')}</TabsTrigger></TabsList><TabsContent value="report">
    <div className="mt-5"><StatusBadge label={localizedValue(t, 'common', run.status)} status={run.status === 'created' ? 'draft' : run.status === 'cancelled' ? 'inactive' : run.status as 'running' | 'completed' | 'failed'} /></div>
    <section className="mt-6 grid grid-cols-2 overflow-hidden rounded-lg border border-border bg-surface lg:grid-cols-4"><MetricCard label={t('evaluations.caseCount')} value={formatNumber(results.length)} /><MetricCard label={t('evaluations.passRate')} value={`${(metricValue('pass_rate') * 100).toFixed(1)}%`} /><MetricCard label={t('evaluations.averageScore')} value={metricValue('average_score').toFixed(3)} /><MetricCard label={t('evaluations.totalCost')} value={formatNumber(metricValue('total_cost_micros'))} /></section>
    <Card className="mt-5 overflow-hidden"><div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('evaluations.caseResults')}</h2></div>{results.length === 0 ? <div className="p-8"><EmptyState title={run.status === 'created' ? t('evaluations.notRun') : t('evaluations.waitingForResults')} description={run.status === 'created' ? t('evaluations.startEvaluationDescription') : t('evaluations.waitingForResultsDescription')} /></div> : <div className="divide-y divide-border">{results.map((result) => <article className="px-5 py-4" key={result.caseId}><div className="grid gap-3 text-xs lg:grid-cols-[minmax(180px,1fr)_100px_100px_120px_100px]"><div><strong className="block">{result.caseKey}</strong><span className="mt-1 block text-[10px] text-muted-foreground">{result.sourceCaseId}</span></div><Badge className="w-fit" tone={result.status === 'passed' || result.status === 'completed' ? 'success' : result.status === 'failed' || result.status === 'error' ? 'danger' : 'neutral'}>{localizedValue(t, 'common', result.status)}</Badge><span>{result.score?.toFixed(3) ?? '—'}</span><span>{result.durationMs ? `${formatNumber(result.durationMs)} ms` : '—'} · {formatNumber(result.costMicros)}</span><span>{result.targetExecutionId ? <Button asChild size="sm" variant="ghost"><Link to={`/executions/${result.targetExecutionId}`}><ExternalLink className="size-3.5" />Trace</Link></Button> : '—'}</span></div>{result.ruleResults.length > 0 && <div className="mt-3 grid gap-2 border-t border-border pt-3">{result.ruleResults.map((rule) => <div className="grid items-center gap-2 text-[11px] text-muted-foreground lg:grid-cols-[minmax(180px,1fr)_100px_100px_minmax(180px,1fr)]" key={rule.id}><span>{rule.name} · {rule.evaluatorType}</span><span>{localizedValue(t, 'common', rule.status)}</span><span>{rule.score?.toFixed(3) ?? '—'}</span><span className="truncate">{rule.evaluatorType === 'llm_judge' ? <JudgeResult detail={rule.detail as JudgeDetail | null | undefined} traceId={typeof (rule.detail as JudgeDetail | null | undefined)?.evaluatorExecutionId === 'string' ? String((rule.detail as JudgeDetail).evaluatorExecutionId) : rule.evaluatorExecutionId} /> : rule.evaluatorExecutionId ? <Link className="text-primary hover:underline" to={`/executions/${rule.evaluatorExecutionId}`}>{t('evaluations.evaluatorTrace')}</Link> : JSON.stringify(rule.detail)}</span></div>)}</div>}{result.errorMessage && <p className="mt-2 text-xs text-danger">{result.errorCode}: {result.errorMessage}</p>}</article>)}</div>}</Card>
    </TabsContent><TabsContent value="compare"><EvaluationComparison runIds={compareIds} onChange={(ids) => setParams({ tab: 'compare', runIds: ids.join(',') })} /></TabsContent></Tabs>
    <ConfirmDialog cancelLabel={t('common.cancel')} confirmLabel={t('common.confirm')} description={t('evaluations.confirmCancel')} onClose={() => setConfirmCancel(false)} onConfirm={() => action.mutateAsync('cancel').then(() => undefined)} open={confirmCancel} pending={action.isPending} title={t('common.cancel')} />
  </PageContainer>
}

function getMetricValue(metrics: unknown, key: string) {
  if (Array.isArray(metrics)) {
    const metric = metrics.find((item) => {
      if (typeof item !== 'object' || item === null) return false
      return (item as Record<string, unknown>).key === key
    })
    if (typeof metric !== 'object' || metric === null) return 0
    const value = (metric as Record<string, unknown>).value
    return typeof value === 'number' ? value : 0
  }
  if (typeof metrics !== 'object' || metrics === null) return 0
  const camelKey = key.replace(/_([a-z])/g, (_, letter: string) => letter.toUpperCase())
  const value = (metrics as Record<string, unknown>)[key] ?? (metrics as Record<string, unknown>)[camelKey]
  return typeof value === 'number' ? value : 0
}

type JudgeDetail = { modelResult?: { passed?: unknown, score?: unknown, reason?: unknown }, evaluatorExecutionId?: unknown }

function JudgeResult({ detail, traceId }: { detail: JudgeDetail | null | undefined, traceId?: string | null }) {
  const { t } = useTranslation()
  const model = detail?.modelResult
  const reason = typeof model?.reason === 'string' ? model.reason : JSON.stringify(model)
  return <span className="flex min-w-0 items-center gap-2">
    <span className="truncate text-[11px]">{reason}</span>
    {traceId && <Link className="shrink-0 text-primary hover:underline" to={`/executions/${traceId}`}>{t('evaluations.evaluatorTrace')}</Link>}
  </span>
}
