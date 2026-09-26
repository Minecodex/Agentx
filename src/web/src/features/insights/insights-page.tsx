import { useQuery } from '@tanstack/react-query'
import { Activity } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest } from '../../shared/api/client'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { TopBarChart } from '../../shared/components/charts/bar-chart'
import { TrendChart } from '../../shared/components/charts/trend-chart'
import { Card } from '../../shared/ui/card'
import { Input } from '../../shared/ui/input'

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
  const [window] = useState(() => windowBounds(WINDOW_DAYS))
  const [workflowFilter, setWorkflowFilter] = useState('')
  const filters = workflowFilter.trim() ? { workflowId: workflowFilter.trim() } : {}

  const aggregate = (metrics: string[], dimensions: string[]) => useQuery({
    queryKey: ['insights-aggregate', metrics, dimensions, window.from, filters],
    queryFn: () => apiRequest<AggregateResponse>('/insights/aggregates', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ...window, metrics, dimensions, filters, limit: 500 }) }),
    retry: false,
  })
  const successRate = aggregate(['error_rate', 'count'], ['day'])
  const cost = aggregate(['cost_micros'], ['day'])
  const errors = aggregate(['count'], ['error_code'])
  const nodeDuration = aggregate(['duration_p50', 'duration_p95'], ['span_name'])

  const degraded = [successRate, cost, errors, nodeDuration].some((query) => query.isError)
  const bucket = (row: AggregateRow, key: string) => String(row.dimensions[key] ?? '—')
  const trendData = (query: ReturnType<typeof aggregate>) => (query.data?.rows ?? []).map((row) => ({
    bucket: bucket(row, 'day').slice(0, 10),
    errorRate: row.metrics.error_rate != null ? Number((row.metrics.error_rate * 100).toFixed(2)) : undefined,
    invocations: row.metrics.count,
    costMicros: row.metrics.cost_micros,
  }))
  return <PageContainer>
    <PageHeader description={t('insights.description')} title={t('insights.title')} />
    {degraded && <Card className="mb-5 border-warning/30 bg-warning/10 p-4 text-xs text-warning">{t('insights.degraded')}</Card>}
    <Card className="mb-5 flex flex-wrap items-center gap-3 p-4">
      <Activity className="size-4 text-primary" />
      <span className="text-xs font-medium">{t('insights.filterWorkflow')}</span>
      <Input className="max-w-xs" onChange={(event) => setWorkflowFilter(event.target.value)} placeholder="00000000-0000-…" value={workflowFilter} />
      <span className="text-[11px] text-muted-foreground">{t('insights.windowHint', { days: WINDOW_DAYS })}</span>
      {successRate.isError && <span className="text-[11px] text-danger">{t('insights.degraded')}</span>}
    </Card>
    <div className="grid gap-5 xl:grid-cols-2">
      <Card className="overflow-hidden">
        <div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('insights.successTrend')}</h2></div>
        <div className="p-3"><TrendChart data={trendData(successRate)} series={[{ key: 'errorRate', label: t('insights.errorRatePercent') }, { key: 'invocations', label: t('insights.invocations') }]} valueSuffix="" /></div>
      </Card>
      <Card className="overflow-hidden">
        <div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('insights.costTrend')}</h2></div>
        <div className="p-3"><TrendChart data={trendData(cost)} series={[{ key: 'costMicros', label: t('insights.costMicros'), color: 'var(--ui-success, #0f9f6e)' }]} /></div>
      </Card>
      <Card className="overflow-hidden">
        <div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('insights.errorDistribution')}</h2></div>
        <div className="p-3">{errors.data ? <TopBarChart colorFor={(row) => String(row.bucket) === '—' ? 'var(--ui-muted)' : 'var(--ui-danger, #dc4c64)'} data={(errors.data.rows ?? []).map((row) => ({ bucket: bucket(row, 'errorCode'), count: row.metrics.count })).sort((a, b) => b.count - a.count).slice(0, 10)} valueKey="count" /> : <p className="p-6 text-center text-xs text-muted-foreground">{t('common.loading')}</p>}</div>
      </Card>
      <Card className="overflow-hidden">
        <div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('insights.nodeDuration')}</h2></div>
        <div className="p-3">{nodeDuration.data ? <TopBarChart data={(nodeDuration.data.rows ?? []).map((row) => ({ bucket: bucket(row, 'spanName'), durationP50: row.metrics.duration_p50, durationP95: row.metrics.duration_p95 })).sort((a, b) => (b.durationP95 ?? 0) - (a.durationP95 ?? 0)).slice(0, 10)} valueKey="durationP95" /> : <p className="p-6 text-center text-xs text-muted-foreground">{t('common.loading')}</p>}</div>
      </Card>
    </div>
  </PageContainer>
}
