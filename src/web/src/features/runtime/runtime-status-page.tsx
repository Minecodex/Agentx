import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { TFunction } from 'i18next'
import { Box, Boxes, CircleGauge, Play, Save, ShieldCheck, Trash2 } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest, jsonBody } from '../../shared/api/client'
import type { QuotaPolicy, RetentionItem, RetentionRun, RuntimeStatus, WorkerCapability } from '../../shared/api/types'
import { ConfirmDialog } from '../../shared/components/confirm-dialog'
import { EmptyState } from '../../shared/components/empty-state'
import { MetricCard } from '../../shared/components/metric-card'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { localizedValue } from '../../shared/lib/localized-value'
import { Badge } from '../../shared/ui/badge'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { Input } from '../../shared/ui/input'
import { useToast } from '../../shared/ui/toast'

export function RuntimeStatusPage() {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  const { showToast } = useToast()
  const queryClient = useQueryClient()
  const [selectedRun, setSelectedRun] = useState<string>()
  const [confirmCleanup, setConfirmCleanup] = useState(false)
  const status = useQuery({ queryKey: ['runtime-status'], queryFn: () => apiRequest<RuntimeStatus>('/runtime/status'), refetchInterval: 15_000 })
  const quotas = useQuery({ queryKey: ['runtime-quotas'], queryFn: () => apiRequest<QuotaPolicy[]>('/runtime/quotas'), refetchInterval: 15_000 })
  const capabilities = useQuery({ queryKey: ['runtime-capabilities'], queryFn: () => apiRequest<WorkerCapability[]>('/runtime/capabilities'), refetchInterval: 15_000 })
  const retention = useQuery({ queryKey: ['retention-runs'], queryFn: () => apiRequest<RetentionRun[]>('/retention-runs'), refetchInterval: (query) => query.state.data?.some((run) => run.status === 'queued' || run.status === 'running') ? 2_000 : false })
  const items = useQuery({ queryKey: ['retention-items', selectedRun], queryFn: () => apiRequest<RetentionItem[]>(`/retention-runs/${selectedRun}/items`), enabled: Boolean(selectedRun), refetchInterval: selectedRun ? 2_000 : false })
  const updateQuota = useMutation({ mutationFn: (policy: QuotaPolicy) => apiRequest<QuotaPolicy[]>('/runtime/quotas', { method: 'PUT', body: jsonBody({ policies: [{ dimension: policy.dimension, hardLimit: policy.hardLimit, periodSeconds: policy.periodSeconds }] }) }), onSuccess: () => queryClient.invalidateQueries({ queryKey: ['runtime-quotas'] }), onError: (error: Error) => showToast(error.message) })
  const createRetention = useMutation({ mutationFn: (dryRun: boolean) => apiRequest<RetentionRun>('/retention-runs', { method: 'POST', body: jsonBody({ dryRun, artifactRetentionDays: 30, traceRetentionDays: 180, messageRetentionDays: 180, evaluationRetentionDays: 365 }) }), onSuccess: async (run) => { setSelectedRun(run.id); setConfirmCleanup(false); await queryClient.invalidateQueries({ queryKey: ['retention-runs'] }) }, onError: (error: Error) => showToast(error.message) })

  const problem = status.error ?? quotas.error ?? capabilities.error ?? retention.error ?? items.error
  if (problem || status.isLoading || quotas.isLoading || capabilities.isLoading || retention.isLoading) {
    return <PageContainer>
      <PageHeader description={t('runtime.runtimeDescription')} title={t('runtime.runtimeStatus')} />
      <Card className="mt-6" role={problem ? 'alert' : 'status'}>
        <EmptyState description={problem?.message ?? t('common.loading')} title={problem ? t('common.loadFailed') : t('common.loading')} />
      </Card>
    </PageContainer>
  }

  return <PageContainer>
    <PageHeader description={t('runtime.runtimeDescription')} title={t('runtime.runtimeStatus')} />
    <section className="mt-6 grid grid-cols-2 overflow-hidden rounded-lg border border-border bg-surface lg:grid-cols-4"><MetricCard label={t('runtime.dashboard.metrics.running')} value={status.data?.running == null ? '—' : String(status.data.running)} /><MetricCard label={t('common.waiting')} value={status.data?.waiting == null ? '—' : String(status.data.waiting)} /><MetricCard label={t('runtime.failedToday')} value={status.data?.failedToday == null ? '—' : String(status.data.failedToday)} /><MetricCard label={t('runtime.activeSandboxes')} value={status.data?.activeSandboxes == null ? '—' : String(status.data.activeSandboxes)} /></section>
    <Card className="mt-5 overflow-hidden"><SectionTitle icon={<Boxes className="size-4 text-primary" />} title={t('runtime.runtimeComponents')} /><div className="divide-y divide-border">{status.data?.components.map((component) => <div className="grid gap-2 px-5 py-4 text-xs sm:grid-cols-[minmax(180px,1fr)_120px_120px_220px] sm:items-center" key={component.component}><span className="flex items-center gap-2 font-medium"><CircleGauge className="size-4 text-muted-foreground" />{localizedValue(t, 'runtime.components', component.component)}</span><Badge className="w-fit" tone={component.status === 'ready' ? 'success' : component.status === 'unknown' ? 'neutral' : 'warning'}>{localizedValue(t, 'common', component.status)}</Badge><span>{component.instances ?? '—'} {t('runtime.instances')}</span><span className="text-muted-foreground">{component.lastHeartbeat ? formatDateTime(component.lastHeartbeat) : t('runtime.noHeartbeat')}</span></div>)}</div></Card>
    <div className="mt-5 grid gap-5 xl:grid-cols-2">
      <Card className="overflow-hidden"><SectionTitle icon={<CircleGauge className="size-4 text-primary" />} title={t('runtime.runtimeQuotas')} /><p className="border-b border-border px-5 py-3 text-xs text-muted-foreground">{t('runtime.governance.quotaDescription')}</p><div className="divide-y divide-border">{quotas.data?.map((policy) => <QuotaRow key={policy.dimension} onSave={(value) => updateQuota.mutate({ ...policy, hardLimit: value })} pending={updateQuota.isPending} policy={policy} t={t} />)}</div></Card>
      <WorkerCapabilitiesCard capabilities={capabilities.data ?? []} t={t} />
    </div>
    <Card className="mt-5 overflow-hidden"><div className="flex items-center border-b border-border px-5 py-4"><h2 className="flex items-center gap-2 text-sm font-semibold"><Trash2 className="size-4 text-primary" />{t('runtime.retention')}</h2><div className="ml-auto flex gap-2"><Button disabled={createRetention.isPending} onClick={() => createRetention.mutate(true)} size="sm" variant="secondary"><Play className="size-3.5" />{t('runtime.dryRun')}</Button><Button disabled={createRetention.isPending} onClick={() => setConfirmCleanup(true)} size="sm" variant="danger"><Trash2 className="size-3.5" />{t('runtime.cleanup')}</Button></div></div><p className="border-b border-border px-5 py-3 text-xs text-muted-foreground">{t('runtime.governance.retentionDescription')}</p><div className="grid min-h-48 lg:grid-cols-[minmax(300px,0.8fr)_minmax(0,1.2fr)]"><div className="border-r border-border">{retention.data?.map((run) => <button className={`grid w-full grid-cols-[1fr_auto] gap-2 border-b border-border px-5 py-3 text-left text-xs hover:bg-muted/50 ${selectedRun === run.id ? 'bg-primary/5' : ''}`} key={run.id} onClick={() => setSelectedRun(run.id)} type="button"><span><strong>{run.dryRun ? t('runtime.dryRun') : t('runtime.cleanup')}</strong><span className="mt-1 block text-[10px] text-muted-foreground">{formatDateTime(run.createdAt)} · {run.candidateCount} / {run.deletedCount}</span></span><Badge tone={run.status === 'completed' ? 'success' : run.status === 'failed' ? 'danger' : 'warning'}>{localizedValue(t, 'runtime.governance.runStatus', run.status)}</Badge></button>)}</div><div className="max-h-80 overflow-auto divide-y divide-border">{items.data?.map((item) => <div className="grid gap-2 px-5 py-3 text-[11px] sm:grid-cols-[150px_minmax(0,1fr)_100px]" key={item.id}><span>{localizedValue(t, 'runtime.retentionDataType', item.dataType)}</span><span className="truncate text-muted-foreground" title={item.targetId}>{item.targetId}{item.reason ? ` · ${localizedValue(t, 'runtime.retentionReasons', item.reason)}` : ''}</span><Badge className="w-fit" tone={item.status === 'deleted' ? 'success' : item.status === 'blocked' || item.status === 'failed' ? 'danger' : 'neutral'}>{localizedValue(t, 'runtime.retentionStatus', item.status)}</Badge></div>)}</div></div></Card>
    {status.data?.sandboxCompatibility !== undefined && <Card className="mt-5 p-5"><div className="flex items-center gap-2"><ShieldCheck className="size-4 text-primary" /><h2 className="text-sm font-semibold">{t('runtime.sandboxCompatibility')}</h2><Badge className="ml-auto" tone="primary"><Box className="mr-1 size-3" />OpenSandbox</Badge></div><pre className="mt-4 overflow-auto rounded-md bg-muted p-3 text-[11px] leading-5">{JSON.stringify(status.data.sandboxCompatibility, null, 2)}</pre></Card>}
    <ConfirmDialog cancelLabel={t('common.cancel')} confirmLabel={t('runtime.cleanup')} description={t('runtime.cleanupConfirmation')} onClose={() => setConfirmCleanup(false)} onConfirm={() => createRetention.mutateAsync(false).then(() => undefined)} open={confirmCleanup} pending={createRetention.isPending} title={t('runtime.cleanup')} />
  </PageContainer>
}

function SectionTitle({ icon, title }: { icon: React.ReactNode; title: string }) { return <div className="border-b border-border px-5 py-4"><h2 className="flex items-center gap-2 text-sm font-semibold">{icon}{title}</h2></div> }

function QuotaRow({ policy, pending, onSave, t }: { policy: QuotaPolicy; pending: boolean; onSave: (value: string) => void; t: TFunction }) {
  const config = quotaCopy(policy.dimension, t)
  const [value, setValue] = useState(formatQuotaValue(policy.hardLimit, policy.dimension))
  useEffect(() => setValue(formatQuotaValue(policy.hardLimit, policy.dimension)), [policy.hardLimit, policy.dimension])
  const submitValue = parseQuotaValue(value, policy.dimension)
  return <div className="grid items-center gap-3 px-5 py-3 text-xs sm:grid-cols-[minmax(180px,1fr)_100px_90px_120px_36px]"><span><strong>{config.label}</strong><span className="mt-1 block text-[10px] text-muted-foreground">{t('runtime.governance.used')} {formatQuotaValue(policy.periodUsage, policy.dimension)} {config.unit} · {t('runtime.governance.reserved')} {formatQuotaValue(policy.activeReserved, policy.dimension)} {config.unit}</span></span><Input aria-label={config.label} className="h-8" onChange={(event) => setValue(event.target.value)} step={quotaStep(policy.dimension)} type="number" value={value} /><span className="text-muted-foreground">{config.unit}</span><span className="text-muted-foreground">{periodLabel(policy.periodSeconds, config.period)}</span><Button aria-label={`${t('common.save')} ${config.label}`} disabled={pending || submitValue === policy.hardLimit} onClick={() => onSave(submitValue)} size="icon" variant="ghost"><Save className="size-3.5" /></Button></div>
}

function quotaCopy(dimension: string, t: TFunction) {
  return {
    label: t(`runtime.quota.${dimension}.label`, { defaultValue: t('common.unknownValue', { value: dimension }) }),
    unit: t(`runtime.quota.${dimension}.unit`, { defaultValue: '' }),
    period: t(`runtime.quota.${dimension}.period`, { defaultValue: '' }),
  }
}

function quotaScale(dimension: string) {
  if (dimension === 'cpu_millis') return 1000
  if (dimension === 'memory_bytes' || dimension === 'disk_bytes' || dimension === 'artifact_bytes') return 1024 ** 3
  if (dimension === 'ttl_seconds') return 3600
  return 1
}

function trimDecimal(value: number) {
  return value.toFixed(6).replace(/\.?0+$/, '') || '0'
}

function formatQuotaValue(value: string, dimension: string) {
  const parsed = Number(value)
  return Number.isFinite(parsed) ? trimDecimal(parsed / quotaScale(dimension)) : value
}

function parseQuotaValue(value: string, dimension: string) {
  const parsed = Number(value)
  if (!Number.isFinite(parsed) || parsed < 0) return value
  return String(Math.round(parsed * quotaScale(dimension)))
}

function quotaStep(dimension: string) {
  return quotaScale(dimension) === 1 ? 1 : 0.001
}

function periodLabel(seconds: number | null | undefined, fallback: string) {
  if (!seconds) return fallback
  if (seconds === 86_400) return fallback
  if (seconds % 3600 === 0) return `${seconds / 3600}h`
  if (seconds % 60 === 0) return `${seconds / 60}m`
  return `${seconds}s`
}

function WorkerCapabilitiesCard({ capabilities, t }: { capabilities: WorkerCapability[]; t: TFunction }) {
  const grouped = useMemo(() => {
    const groups = new Map<string, { capability: string; count: number; ready: boolean }>()
    for (const item of capabilities) {
      const current = groups.get(item.capability) ?? { capability: item.capability, count: 0, ready: false }
      current.count += 1
      current.ready ||= item.status === 'ready'
      groups.set(item.capability, current)
    }
    return [...groups.values()].sort((left, right) => left.capability.localeCompare(right.capability))
  }, [capabilities])
  return <Card className="overflow-hidden"><SectionTitle icon={<ShieldCheck className="size-4 text-primary" />} title={t('runtime.workerCapabilities')} /><p className="border-b border-border px-5 py-3 text-xs text-muted-foreground">{t('runtime.governance.capabilityDescription')}</p><div className="divide-y divide-border">{grouped.map((item) => <div className="grid gap-2 px-5 py-3 text-xs sm:grid-cols-[minmax(0,1fr)_150px_120px]" key={item.capability}><span className="font-medium">{localizedValue(t, 'runtime.capability', item.capability)}</span><span className="text-muted-foreground">{t('runtime.governance.workerCount', { count: item.count })}</span><Badge className="w-fit" tone={item.ready ? 'success' : 'warning'}>{localizedValue(t, 'runtime.capabilityStatus', item.ready ? 'ready' : 'unavailable')}</Badge></div>)}</div></Card>
}
