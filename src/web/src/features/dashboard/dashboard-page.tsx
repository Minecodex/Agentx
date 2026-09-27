import { useQuery } from '@tanstack/react-query'
import { ArrowUpRight, Bot, Plus } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Link, useOutletContext } from 'react-router-dom'

import { apiRequest } from '../../shared/api/client'
import type { DashboardSummary, PageResponse, Workflow } from '../../shared/api/types'
import { EmptyState } from '../../shared/components/empty-state'
import { EntityCell } from '../../shared/components/entity-cell'
import { MetricCard } from '../../shared/components/metric-card'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { StatusBadge } from '../../shared/components/status-badge'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { Table, TableCell, TableContainer, TableHead } from '../../shared/ui/table'

type AggregateRow = { metrics: Record<string, number> }

function value0(backend: number | null | undefined): number { return backend ?? 0 }

export function DashboardPage() {
  const { t } = useTranslation()
  const { formatDateTime, formatNumber } = useLocaleFormat()
  const { tenantName } = useOutletContext<{ tenantName: string }>()
  const summary = useQuery({ queryKey: ['dashboard-summary'], queryFn: () => apiRequest<DashboardSummary>('/dashboard/summary'), refetchInterval: 30_000 })
  const workflows = useQuery({ queryKey: ['workflows', 'dashboard'], queryFn: () => apiRequest<PageResponse<Workflow>>('/workflows?pageSize=5') })
  // plan7 P7-C: the five runtime metrics on the summary endpoint are null by
  // contract; the live values come from the insights aggregates (today's
  // window) and the executions search total (currently running). Both
  // degrade to the previous placeholders when observability is scaled down.
  const startOfDay = new Date(); startOfDay.setHours(0, 0, 0, 0)
  const insights = useQuery({
    queryKey: ['dashboard-insights', startOfDay.toISOString()],
    queryFn: () => apiRequest<{ rows?: AggregateRow[] }>('/insights/aggregates', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ from: startOfDay.toISOString(), to: new Date().toISOString(), metrics: ['count', 'error_rate', 'cost_micros'], dimensions: ['day'], limit: 1, filters: {} }) }),
    retry: false, refetchInterval: 30_000,
  })
  const running = useQuery({ queryKey: ['dashboard-running'], queryFn: () => apiRequest<{ total?: number }>('/executions?statuses=running&limit=1'), retry: false, refetchInterval: 30_000 })
  const row = insights.data?.rows?.[0]?.metrics
  const executionsToday = row?.count ?? value0(summary.data?.executionsToday)
  const failedToday = row && row.count ? Math.round(row.count * (row.error_rate ?? 0)) : value0(summary.data?.failedToday)
  const succeededToday = row ? Math.max(0, executionsToday - failedToday) : value0(summary.data?.succeededToday)
  const costMicrosToday = row?.cost_micros ?? value0(summary.data?.costMicrosToday)
  const runningExecutions = running.data?.total ?? value0(summary.data?.runningExecutions)
  const value = summary.data
  const total = succeededToday + failedToday
  const rate = total ? `${((succeededToday / total) * 100).toFixed(1)}%` : '—'
  const metrics = [{ label: t('runtime.dashboard.metrics.running'), value: formatNumber(runningExecutions), suffix: t('runtime.dashboard.metrics.executions'), note: t('runtime.dashboard.metrics.realtime') }, { label: t('runtime.dashboard.metrics.today'), value: formatNumber(executionsToday), note: `${formatNumber(failedToday)} ${t('common.failed')}` }, { label: t('runtime.dashboard.metrics.successRate'), value: rate, note: `${formatNumber(succeededToday)} ${t('common.success')}` }, { label: t('runtime.dashboard.metrics.cost'), value: `¥ ${(costMicrosToday / 1_000_000).toFixed(4)}`, note: `${formatNumber(value?.pendingApprovals ?? 0)} ${t('common.pending')}`, noteTone: 'muted' as const }]
  return <PageContainer><PageHeader action={<Button asChild><Link to="/workflows"><Plus className="size-4" />{t('runtime.dashboard.newWorkflow')}</Link></Button>} description={t('runtime.dashboard.intro', { tenant: tenantName })} eyebrow={t('runtime.dashboard.eyebrow')} title={t('runtime.dashboard.greeting')} /><section className="mt-7 grid grid-cols-4 overflow-hidden rounded-xl border border-border bg-surface shadow-sm">{metrics.map((metric) => <MetricCard key={metric.label} {...metric} />)}</section><Card className="mt-5 overflow-hidden"><div className="flex h-15 items-center justify-between border-b border-border px-5"><h2 className="text-sm font-semibold">{t('runtime.dashboard.recent')}</h2><Button asChild size="sm" variant="ghost"><Link to="/workflows">{t('common.viewAll')} <ArrowUpRight className="size-3.5" /></Link></Button></div>{workflows.data?.items.length ? <TableContainer><Table><thead className="bg-muted/45 text-[10px] uppercase text-muted-foreground"><tr><TableHead className="w-[52%]">{t('workflows.title')}</TableHead><TableHead>{t('common.status')}</TableHead><TableHead>{t('common.version')}</TableHead><TableHead>{t('common.updatedAt')}</TableHead></tr></thead><tbody>{workflows.data.items.map((workflow) => <tr className="border-t border-border first:border-0 hover:bg-muted/30" key={workflow.id}><TableCell><EntityCell detail={workflow.description ?? t('workflows.revision', { revision: workflow.draftRevision })} icon={Bot} name={workflow.name} /></TableCell><TableCell><StatusBadge status={workflow.status === 'active' ? 'active' : 'inactive'} /></TableCell><TableCell>{workflow.latestVersion ? `v${workflow.latestVersion}` : '—'}</TableCell><TableCell>{formatDateTime(workflow.updatedAt)}</TableCell></tr>)}</tbody></Table></TableContainer> : <EmptyState title={t('applications.noWorkflows')} description={t('applications.noWorkflowsDescription')} />}</Card></PageContainer>
}
