import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useSearchParams } from 'react-router-dom'
import { useAuth } from '../../../app/providers/auth-provider'
import { apiRequest } from '../../../shared/api/client'
import { ConfirmDialog } from '../../../shared/components/confirm-dialog'
import { Button } from '../../../shared/ui/button'
import { Card } from '../../../shared/ui/card'
import { EvaluationCreate } from './evaluation-create'
import { EvaluationList } from './evaluation-list'
import { EvaluationReport } from './report'
import { testingPath, type Batch } from './types'

const active = new Set(['queued', 'running'])
export function ApplicationEvaluations({ applicationId }: { applicationId: string }) {
  const { t } = useTranslation()
  const auth = useAuth()
  const client = useQueryClient()
  const [params, setParams] = useSearchParams()
  const batchId = params.get('batchId')
  const path = `${testingPath(applicationId)}/evaluations`
  const batches = useQuery({ queryKey: ['application-evaluations', applicationId], queryFn: () => apiRequest<Batch[]>(path), refetchInterval: query => query.state.data?.some(value => (active.has(value.status) || value.runs.some(run => ['created', 'queued', 'running'].includes(run.status)))) ? 2000 : false })
  const report = useQuery({ queryKey: ['application-evaluation-report', applicationId, batchId], enabled: Boolean(batchId), queryFn: () => apiRequest<Batch>(`${path}/${batchId}/report`), refetchInterval: query => !query.state.data || (active.has(query.state.data.status) || query.state.data.runs.some(run => ['created', 'queued', 'running'].includes(run.status))) ? 1500 : false })
  const [creating, setCreating] = useState(false)
  const [confirmCancel, setConfirmCancel] = useState(false)
  const cancel = useMutation({ mutationFn: () => apiRequest(`${path}/${batchId}/cancel`, { method: 'POST' }), onSuccess: async () => { setConfirmCancel(false); await client.invalidateQueries({ queryKey: ['application-evaluation-report', applicationId] }); await client.invalidateQueries({ queryKey: ['application-evaluations', applicationId] }) } })
  const select = (id?: string) => setParams(current => { const next = new URLSearchParams(current); if (id) next.set('batchId', id); else next.delete('batchId'); return next })
  return <div className="space-y-4"><div className="flex items-center justify-between"><div className="flex items-center gap-3">{batchId && <Button size="sm" variant="ghost" onClick={() => select()}>{t('testing.backRuns')}</Button>}<h3 className="text-sm font-semibold">{report.data?.name ?? t('testing.runs')}</h3></div><div className="flex gap-2">{report.data && active.has(report.data.status) && auth.hasPermission('evaluation:manage') && <Button variant="secondary" onClick={() => setConfirmCancel(true)}>{t('testing.cancel')}</Button>}{auth.hasPermission('evaluation:manage') && <Button onClick={() => setCreating(true)}>{t('testing.createEvaluation')}</Button>}</div></div>
    {batchId ? report.data ? <EvaluationReport report={report.data} /> : report.error ? <p role="alert" className="text-danger">{report.error.message}</p> : <p>{t('common.loading')}</p> : <Card><EvaluationList batches={batches.data ?? []} onSelect={select} />{batches.isLoading && <p className="p-5">{t('common.loading')}</p>}{batches.error && <p role="alert" className="p-5 text-danger">{batches.error.message}</p>}{batches.data?.length === 0 && <p className="p-8 text-center text-sm text-muted-foreground">{t('testing.noRuns')}</p>}</Card>}
    {creating && <EvaluationCreate applicationId={applicationId} onClose={() => setCreating(false)} onCreated={async id => { await client.invalidateQueries({ queryKey: ['application-evaluations', applicationId] }); select(id) }} />}
    <ConfirmDialog open={confirmCancel} title={t('testing.cancel')} description={t('testing.cancelConfirm')} cancelLabel={t('common.cancel')} confirmLabel={t('common.confirm')} pending={cancel.isPending} onClose={() => setConfirmCancel(false)} onConfirm={() => cancel.mutateAsync().then(() => undefined)} />
    {cancel.error && <p role="alert" className="text-xs text-danger">{cancel.error.message}</p>}
  </div>
}
