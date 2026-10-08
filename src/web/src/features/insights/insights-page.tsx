import { useQuery } from '@tanstack/react-query'
import { Activity } from 'lucide-react'
import { Link, useNavigate } from 'react-router-dom'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import type { PageResponse, Workflow } from '../../shared/api/types'
import { Select } from '../../shared/ui/select'
import { apiRequest } from '../../shared/api/client'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { TopBarChart } from '../../shared/components/charts/bar-chart'
import { TrendChart } from '../../shared/components/charts/trend-chart'
import { Card } from '../../shared/ui/card'

type AggregateRow = { dimensions: Record<string, unknown>, metrics: Record<string, number> }
type AggregateResponse = { rows?: AggregateRow[] }

const WINDOW_DAYS = 14

function windowBounds(days: number) {
  const to = new Date()
  const from = new Date(to.getTime() - days * 24 * 3600 * 1000)
  return { from: from.toISOString(), to: to.toISOString() }
}

export function InsightsPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const workflows = useQuery({ queryKey: ['insights-workflows'], queryFn: () => apiRequest<PageResponse<Workflow>>('/workflows?pageSize=100') })
  const [window] = useState(() => windowBounds(WINDOW_DAYS))
  const [workflowFilter, setWorkflowFilter] = useState('')
  const filters = workflowFilter.trim() ? { workflowId: workflowFilter.trim() } : {}

  const useAggregate = (metrics: string[], dimensions: string[], extraFilters: Record<string, string> = {}) => useQuery({
    queryKey: ['insights-aggregate', metrics, dimensions, window.from, filters, extraFilters],
    queryFn: () => apiRequest<AggregateResponse>('/insights/aggregates', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ...window, metrics, dimensions, filters: { ...filters, ...extraFilters }, limit: 500 }) }),
    retry: false,
  })
  const successRate = useAggregate(['succeeded_count', 'count'], ['day'])
  const cost = useAggregate(['cost_micros'], ['day'])
  const errors = useAggregate(['count'], ['error_code'])
  const nodeDuration = useAggregate(['duration_p50', 'duration_p95'], ['span_name'], { spanKind: 'node' })

  const nodeFailures = useAggregate(['failed_count'], ['span_name'], { spanKind: 'node' })
  const toolFailures = useAggregate(['failed_count'], ['span_name'], { spanKind: 'runtime_call', resourceType: 'mcp_tool' })
  const errorRows = (errors.data?.rows ?? []).filter((row) => row.metrics.count > 0 && row.dimensions.errorCode).map((row) => ({ bucket: String(row.dimensions.errorCode), count: row.metrics.count })).sort((a, b) => b.count - a.count).slice(0, 10)
  const errorUrl = (code: string) => `/executions?${new URLSearchParams({ errorCodes: code, createdAfter: window.from, createdBefore: window.to, ...(workflowFilter ? { workflowIds: workflowFilter } : {}) })}`
  const degraded = [successRate, cost, errors, nodeDuration, nodeFailures, toolFailures].some((query) => query.isError)
  const bucket = (row: AggregateRow, key: string) => String(row.dimensions[key] ?? '—')
  const trendData = (query: ReturnType<typeof useAggregate>) => (query.data?.rows ?? []).map((row) => ({
    bucket: bucket(row, 'day').slice(0, 10),
    successRate: row.metrics.count > 0 ? Number(((row.metrics.succeededCount / row.metrics.count) * 100).toFixed(2)) : undefined,
    invocations: row.metrics.count,
    costMicros: row.metrics.costMicros,
  }))
  return <PageContainer>
    <PageHeader description={t('insights.description')} title={t('insights.title')} />
    {degraded && <Card className="mb-5 border-warning/30 bg-warning/10 p-4 text-xs text-warning">{t('insights.degraded')}</Card>}
    <Card className="mb-5 flex flex-wrap items-center gap-3 p-4">
      <Activity className="size-4 text-primary" />
      <span className="text-xs font-medium">{t('insights.filterWorkflow')}</span>
      <Select aria-label={t('insights.filterWorkflow')} className="w-72" onValueChange={(value) => setWorkflowFilter(value === 'all' ? '' : value)} options={[{ value: 'all', label: t('common.all') }, ...(workflows.data?.items ?? []).map((item) => ({ value: item.id, label: item.name }))]} value={workflowFilter || 'all'} />
      <span className="text-[11px] text-muted-foreground">{t('insights.windowHint', { days: WINDOW_DAYS })}</span>
      {successRate.isError && <span className="text-[11px] text-danger">{t('insights.degraded')}</span>}
    </Card>
    <div className="grid gap-5 xl:grid-cols-2">
      <Card className="overflow-hidden">
        <div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('insights.successTrend')}</h2></div>
        <div className="p-3"><TrendChart data={trendData(successRate)} series={[{ key: 'successRate', label: t('insights.successRatePercent') }, { key: 'invocations', label: t('insights.invocations') }]} valueSuffix="" /></div>
      </Card>
      <Card className="overflow-hidden">
        <div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('insights.costTrend')}</h2></div>
        <div className="p-3"><TrendChart data={trendData(cost)} series={[{ key: 'costMicros', label: t('insights.costMicros'), color: 'var(--ui-success, #0f9f6e)' }]} /></div>
      </Card>
      <Card className="overflow-hidden">
        <div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('insights.errorDistribution')}</h2></div>
        <div className="p-3">{errors.data ? <TopBarChart colorFor={(row) => String(row.bucket) === '—' ? 'var(--ui-muted)' : 'var(--ui-danger, #dc4c64)'} data={errorRows} onSelect={(row) => navigate(errorUrl(String(row.bucket)))} valueKey="count" /> : <p className="p-6 text-center text-xs text-muted-foreground">{t('common.loading')}</p>}<div className="mt-3 flex flex-wrap gap-3 px-3 pb-3">{errorRows.map((row) => <Link className="text-xs text-primary hover:underline" key={row.bucket} to={errorUrl(row.bucket)}>{row.bucket} ({row.count})</Link>)}</div></div>
      </Card>
      <Card className="overflow-hidden">
        <div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('insights.nodeDuration')}</h2></div>
        <div className="p-3">{nodeDuration.data ? <TopBarChart data={(nodeDuration.data.rows ?? []).map((row) => ({ bucket: bucket(row, 'spanName'), durationP50: row.metrics.durationP50, durationP95: row.metrics.durationP95 })).sort((a, b) => (b.durationP95 ?? 0) - (a.durationP95 ?? 0)).slice(0, 10)} valueKey="durationP95" /> : <p className="p-6 text-center text-xs text-muted-foreground">{t('common.loading')}</p>}</div>
      </Card>
      {[{ title: t('insights.nodeFailures'), query: nodeFailures }, { title: t('insights.toolFailures'), query: toolFailures }].map(({ title, query }) => <Card className="overflow-hidden" key={title}><h2 className="border-b border-border px-5 py-4 text-sm font-semibold">{title}</h2><div className="p-3"><TopBarChart data={(query.data?.rows ?? []).filter((row) => row.metrics.failedCount > 0).map((row) => ({ bucket: bucket(row, 'spanName'), count: row.metrics.failedCount })).sort((a, b) => b.count - a.count).slice(0, 10)} valueKey="count" /></div></Card>)}
    </div>
  </PageContainer>
}
