import { ToolEffectCard } from './tool-effect-card'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ExternalLink, Hand, RotateCcw, UserRoundCog, X } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useParams } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { Approval, ApprovalAction } from '../../shared/api/types'
import { ConfirmDialog } from '../../shared/components/confirm-dialog'
import { EmptyState } from '../../shared/components/empty-state'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { StatusBadge } from '../../shared/components/status-badge'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { localizedValue } from '../../shared/lib/localized-value'
import { Badge } from '../../shared/ui/badge'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { useToast } from '../../shared/ui/toast'
import { approvalStatus } from './approval-status'

type Candidate = { userId: string; displayName: string }
type RuntimeApproval = Approval & { decision?: { decision?: string; reason?: string | null } | null }
type ApprovalActionName = 'claim' | 'release' | 'reassign' | 'cancel' | 'decide'
type TerminalAction = { kind: 'cancel' } | { kind: 'decide'; id: string; label: string } | null

export function ApprovalDetailPage() {
  const { id = '' } = useParams()
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  const auth = useAuth()
  const { showToast } = useToast()
  const queryClient = useQueryClient()
  const [terminalAction, setTerminalAction] = useState<TerminalAction>(null)
  const [reassignOpen, setReassignOpen] = useState(false)
  const approval = useQuery({ queryKey: ['approval', id], queryFn: () => apiRequest<RuntimeApproval>(`/approvals/${id}`), refetchInterval: (query) => query.state.data?.resumeStatus === 'pending' ? 1_000 : false })
  const actions = useQuery({ queryKey: ['approval-actions', id], queryFn: () => apiRequest<ApprovalAction[]>(`/approvals/${id}/actions`) })
  const candidates = useQuery({ queryKey: ['approval-candidates', id], queryFn: () => apiRequest<Candidate[]>(`/approvals/${id}/candidates`) })
  const refreshRelated = async () => Promise.all([
    queryClient.invalidateQueries({ queryKey: ['approval-actions', id] }),
    queryClient.invalidateQueries({ queryKey: ['approvals'] }),
    queryClient.invalidateQueries({ queryKey: ['notifications'] }),
  ])
  const act = useMutation({
    mutationFn: ({ action, targetUserId, decisionId, reason }: { action: ApprovalActionName; targetUserId?: string; decisionId?: string; reason?: string }) => apiRequest<RuntimeApproval>(`/approvals/${id}/${action}`, { method: 'POST', body: jsonBody(action === 'decide' ? { version: approval.data?.version, decisionId, reason: reason?.trim() || null, idempotencyKey: crypto.randomUUID() } : { version: approval.data?.version, ...(action === 'reassign' ? { targetUserId } : {}) }) }),
    onSuccess: async (value) => { queryClient.setQueryData(['approval', id], value); setTerminalAction(null); setReassignOpen(false); await refreshRelated(); showToast(t('approvals.approvalUpdated')) },
    onError: async (error: Error) => { showToast(error.message); await queryClient.invalidateQueries({ queryKey: ['approval', id] }); await refreshRelated() },
  })
  if (!approval.data) return <PageContainer><EmptyState title={approval.isLoading ? t('common.loading') : t('common.loadFailed')} description={String(approval.error ?? '')} /></PageContainer>
  const value = approval.data
  const toolOperation = value.subject.kind === 'agent_tool_effect'
  const requestPayload = toolOperation && value.requestPayload && typeof value.requestPayload === 'object' && 'arguments' in value.requestPayload ? value.requestPayload.arguments : value.requestPayload
  const buttonLabel = (button: { id: string; label: string }) => value.subject.kind === 'agent_tool_effect' && ['approved', 'rejected'].includes(button.id) ? t(button.id === 'approved' ? 'applicationMcp.approve' : 'applicationMcp.reject') : button.label
  const mine = value.claimedBy === auth.user?.id
  const active = value.status === 'pending' || value.status === 'claimed'
  const decidedButton = value.buttons.find((button) => button.id === value.decision?.decision)
  const decisionLabel = decidedButton ? buttonLabel(decidedButton) : value.decision?.decision
  const candidateFields: EntityFormField[] = [{ name: 'targetUserId', label: t('approvals.assignee'), type: 'select', required: true, options: (candidates.data ?? []).map((item) => ({ value: item.userId, label: item.displayName })) }]
  const decisionFields: EntityFormField[] = [{ name: 'reason', label: t('approvals.reason'), type: 'textarea', required: false }]
  const actionBar = <div className="flex flex-wrap gap-2">
    {auth.hasPermission('approval:manage') && active && <Button onClick={() => setReassignOpen(true)} variant="secondary"><UserRoundCog className="size-4" />{t('approvals.assignee')}</Button>}
    {auth.hasPermission('approval:manage') && active && <Button onClick={() => setTerminalAction({ kind: 'cancel' })} variant="secondary"><X className="size-4" />{t('common.cancel')}</Button>}
    {auth.hasPermission('approval:act') && value.status === 'pending' && <Button onClick={() => act.mutate({ action: 'claim' })}><Hand className="size-4" />{t('approvals.claim')}</Button>}
    {auth.hasPermission('approval:act') && value.status === 'claimed' && mine && <>{value.buttons.map((button) => <Button key={button.id} onClick={() => setTerminalAction({ kind: 'decide', id: button.id, label: buttonLabel(button) })}>{buttonLabel(button)}</Button>)}<Button onClick={() => act.mutate({ action: 'release' })} variant="secondary"><RotateCcw className="size-4" />{t('approvals.release')}</Button></>}
  </div>
  return <PageContainer>
    <PageHeader action={active ? actionBar : undefined} description={`${value.workflowName} · ${value.nodeId}`} title={value.subject.kind === 'agent_tool_effect' ? t('applicationMcp.operationApproval', { name: value.subject.toolName }) : value.title} />
    <div className="mt-5 flex items-center gap-3"><StatusBadge label={localizedValue(t, 'approvals.statuses', value.status)} status={approvalStatus(value.status)} /><span className="text-xs text-muted-foreground">{t('approvals.resumeStatus')}</span><Badge tone={value.resumeStatus === 'succeeded' ? 'success' : value.resumeStatus === 'failed' || value.resumeStatus === 'blocked_runtime' ? 'danger' : value.resumeStatus === 'pending' ? 'warning' : 'neutral'}>{localizedValue(t, 'approvals.resumeStatuses', value.resumeStatus)}</Badge>{decisionLabel && <><span className="text-xs text-muted-foreground">{t('approvals.decision')}</span><Badge tone="primary">{decisionLabel}</Badge></>}<Button asChild className="ml-auto" size="sm" variant="ghost"><Link to={`/executions/${value.executionId}`}><ExternalLink className="size-3.5" />{t('approvals.trace')}</Link></Button></div>
    <ToolEffectCard subject={value.subject}/>
    <div className="mt-6 grid grid-cols-[minmax(0,1.4fr)_minmax(340px,0.6fr)] gap-5"><Card className="p-5"><h2 className="text-sm font-semibold">{t('approvals.request')}</h2><p className="mt-3 text-xs leading-6 text-muted-foreground">{toolOperation ? t('applicationMcp.protectedHint') : value.description ?? '—'}</p><pre className="mt-5 overflow-auto rounded-lg bg-canvas p-4 text-[11px]">{JSON.stringify(requestPayload, null, 2)}</pre></Card><Card className="overflow-hidden"><div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('approvals.actionHistory')}</h2></div><div className="divide-y divide-border px-5">{actions.data?.map((item) => <div className="py-4" key={item.id}><div className="flex justify-between text-xs"><strong>{item.actorName}</strong><span className="text-muted-foreground">{localizedValue(t, 'approvals.actionTypes', item.actionType)}</span></div><p className="mt-1 text-[11px] text-muted-foreground">{localizedValue(t, 'approvals.statuses', item.fromStatus)} → {localizedValue(t, 'approvals.statuses', item.toStatus)} · {formatDateTime(item.createdAt)}</p></div>)}</div></Card></div>
    {reassignOpen && <EntityFormDialog cancelLabel={t('common.cancel')} fields={candidateFields} onClose={() => setReassignOpen(false)} onSubmit={(values) => act.mutateAsync({ action: 'reassign', targetUserId: values.targetUserId }).then(() => undefined)} open submitLabel={t('common.save')} title={t('approvals.assignee')} />}
    {terminalAction?.kind === 'decide' && <EntityFormDialog cancelLabel={t('common.cancel')} fields={decisionFields} onClose={() => setTerminalAction(null)} onSubmit={(values) => act.mutateAsync({ action: 'decide', decisionId: terminalAction.id, reason: values.reason }).then(() => undefined)} open submitLabel={terminalAction.label} title={terminalAction.label} />}
    <ConfirmDialog cancelLabel={t('common.cancel')} confirmLabel={t('common.confirm')} description={`${t('common.cancel')}?`} onClose={() => setTerminalAction(null)} onConfirm={async () => { if (terminalAction?.kind === 'cancel') await act.mutateAsync({ action: 'cancel' }) }} open={terminalAction?.kind === 'cancel'} pending={act.isPending} title={t('common.cancel')} />
  </PageContainer>
}
