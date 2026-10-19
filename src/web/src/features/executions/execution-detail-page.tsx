import { WorkflowInteractions } from '../../shared/components/workflow-interactions'
import { WorkflowHookStatus } from '../../shared/components/workflow-hook-status'
import type { RuntimeDetails } from '../../shared/api/types'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowLeft, Ban, GitFork, Link2, RefreshCw } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate, useParams } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { Approval, Checkpoint, Execution, ExecutionEvent, ForkExecutionRequest, NodeExecution, PageResponse, SideEffectConfirmationRequest } from '../../shared/api/types'
import { EndUserLabel } from '../../shared/components/end-user-label'
import { ConfirmDialog } from '../../shared/components/confirm-dialog'
import { EmptyState } from '../../shared/components/empty-state'
import { StatusBadge } from '../../shared/components/status-badge'
import { Button } from '../../shared/ui/button'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../shared/ui/tabs'
import { useToast } from '../../shared/ui/toast'
import { TraceWorkspace } from '../traces/trace-workspace'
import { useExecutionArtifactDownload } from '../traces/use-execution-artifact-download'
import { localizedValue } from '../../shared/lib/localized-value'
import { designSourceLabel, executionStatus } from './execution-format'
import { ExecutionForkDialog } from './execution-fork-dialog'
import { ExecutionRecoveryRail } from './execution-recovery-rail'
import { SideEffectDialog } from './side-effect-dialog'

type ItemResponse<T> = { items: T[] }
type CommandResponse = { executionId: string; status: string; replayed: boolean }

const terminalStatuses = new Set(['succeeded', 'failed', 'cancelled', 'timed_out'])

export function ExecutionDetailPage() {
  const { id = '' } = useParams()
  const { t } = useTranslation()
  const auth = useAuth()
  const canViewExecution = auth.hasPermission('execution:view')
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const downloadArtifact = useExecutionArtifactDownload(id)
  const [selectedId, setSelectedId] = useState<string>()
  const [forkOpen, setForkOpen] = useState(false)
  const [cancelOpen, setCancelOpen] = useState(false)
  const [confirmationNode, setConfirmationNode] = useState<NodeExecution>()
  const [mainView, setMainView] = useState('trace')
  const finalizedExecution = useRef<string>()
  const forkTrigger = useRef<HTMLButtonElement>(null)

  const execution = useQuery({
    queryKey: ['execution', id],
    queryFn: () => apiRequest<Execution>(`/executions/${id}`),
    refetchInterval: (query) => terminalStatuses.has(query.state.data?.status ?? '') ? false : 2_000,
  })
  const interactions = useQuery({ queryKey: ['execution-interactions', id], queryFn: () => apiRequest<RuntimeDetails>(`/executions/${id}/runtime-details`), enabled: auth.hasPermission('trace:view'), refetchInterval: execution.data && terminalStatuses.has(execution.data.status) ? false : 1000 })
  const nodes = useQuery({ queryKey: ['execution-nodes', id], queryFn: () => apiRequest<ItemResponse<NodeExecution>>(`/executions/${id}/nodes`), refetchInterval: execution.data && !terminalStatuses.has(execution.data.status) ? 2_000 : false })
  const checkpoints = useQuery({ enabled: canViewExecution, queryKey: ['execution-checkpoints', id], queryFn: () => apiRequest<ItemResponse<Checkpoint>>(`/executions/${id}/checkpoints`), refetchInterval: execution.data && !terminalStatuses.has(execution.data.status) ? 2_000 : false })
  const events = useQuery({ enabled: canViewExecution, queryKey: ['execution-recovery-events', id], queryFn: () => apiRequest<{ items: ExecutionEvent[] }>(`/executions/${id}/events?after=0&limit=200`), refetchInterval: execution.data && !terminalStatuses.has(execution.data.status) ? 2_000 : false })
  const approvals = useQuery({ enabled: auth.hasPermission('approval:view'), queryKey: ['approvals', 'execution', id], queryFn: () => apiRequest<PageResponse<Approval>>('/approvals?pageSize=100'), refetchInterval: execution.data && !terminalStatuses.has(execution.data.status) ? 2_000 : false })

  useEffect(() => {
    const status = execution.data?.status
    if (!status || !terminalStatuses.has(status)) return
    const terminalKey = `${id}:${status}`
    if (finalizedExecution.current === terminalKey) return
    finalizedExecution.current = terminalKey
    void Promise.all([
      queryClient.invalidateQueries({ queryKey: ['execution-nodes', id] }),
      queryClient.invalidateQueries({ queryKey: ['execution-node', id] }),
      queryClient.invalidateQueries({ queryKey: ['execution-checkpoints', id] }),
      queryClient.invalidateQueries({ queryKey: ['execution-recovery-events', id] }),
      queryClient.invalidateQueries({ queryKey: ['execution-trace-spans', id] }),
      queryClient.invalidateQueries({ queryKey: ['approvals', 'execution', id] }),
    ])
  }, [execution.data?.status, id, queryClient])

  const nodeItems = useMemo(() => nodes.data?.items ?? [], [nodes.data?.items])
  useEffect(() => {
    if (!selectedId && nodeItems.length) setSelectedId(nodeItems.at(-1)?.id)
    if (selectedId && nodeItems.length && !nodeItems.some((node) => node.id === selectedId)) setSelectedId(nodeItems.at(-1)?.id)
  }, [nodeItems, selectedId])

  const refresh = async () => Promise.all([
    queryClient.invalidateQueries({ queryKey: ['execution', id] }),
    queryClient.invalidateQueries({ queryKey: ['execution-nodes', id] }),
    queryClient.invalidateQueries({ queryKey: ['execution-checkpoints', id] }),
    queryClient.invalidateQueries({ queryKey: ['execution-recovery-events', id] }),
    queryClient.invalidateQueries({ queryKey: ['execution-trace-spans', id] }),
    queryClient.invalidateQueries({ queryKey: ['executions'] }),
  ])
  const closeFork = () => {
    setForkOpen(false)
    requestAnimationFrame(() => forkTrigger.current?.focus())
  }
  const cancel = useMutation({
    mutationFn: () => apiRequest(`/executions/${id}/cancel`, { method: 'POST' }),
    onSuccess: async () => { setCancelOpen(false); await refresh(); showToast(t('executions.cancelledToast')) },
    onError: (error: Error) => showToast(error.message),
  })
  const fork = useMutation({
    mutationFn: (request: ForkExecutionRequest) => apiRequest<CommandResponse>(`/executions/${id}/fork`, { method: 'POST', body: jsonBody(request) }),
    onSuccess: async (result) => { setForkOpen(false); await queryClient.invalidateQueries({ queryKey: ['executions'] }); navigate(`/executions/${result.executionId}`) },
  })
  const confirmation = useMutation({
    mutationFn: (request: SideEffectConfirmationRequest) => apiRequest(`/executions/${id}/side-effect-confirmations`, { method: 'POST', body: jsonBody(request) }),
    onSuccess: async () => { setConfirmationNode(undefined); await refresh(); showToast(t('executions.sideEffectSubmitted')) },
    onError: (error: Error) => showToast(error.message),
  })

  if (!execution.data) return <div className="p-6"><EmptyState title={execution.isLoading ? t('executions.loadingExecution') : execution.error instanceof Error && execution.error.message.includes('403') ? t('executions.noPermission') : t('executions.loadFailedTitle')} description={execution.error instanceof Error ? execution.error.message : t('executions.loadingDescription')} /></div>

  const value = execution.data
  const active = !terminalStatuses.has(value.status)
  const selectedNode = nodeItems.find((node) => node.id === selectedId)
  const executionApprovals = approvals.data?.items.filter((approval) => approval.executionId === id) ?? []
  return <div className="min-h-full bg-background">
    <header className="border-b border-border bg-surface px-4 py-3 lg:px-6">
      <div className="grid grid-cols-[2.25rem_minmax(0,1fr)] items-center gap-x-3 gap-y-2 md:flex md:flex-wrap md:gap-3">
        <Button asChild aria-label={t('executions.backToList')} size="icon" variant="ghost"><Link to={canViewExecution ? '/executions' : '/playground'}><ArrowLeft className="size-4" /></Link></Button>
        <div className="min-w-0 md:flex-1"><div className="flex min-w-0 items-center gap-2"><h1 className="min-w-0 flex-1 truncate text-base font-semibold">{value.workflowName}</h1><StatusBadge label={localizedValue(t, 'common', value.status)} status={executionStatus(value.status)} /></div><p className="mt-1 truncate text-[10px] text-muted-foreground">{t('executions.executionId', { id: value.id })} · {designSourceLabel(value.source, value.workflowVersionNumber, (revision, draft) => t('executions.draftRevision', {revision, draft}))} · {localizedValue(t, 'executions.executionTypes', value.executionType)}</p></div>
        <div className="col-start-2 flex flex-wrap items-center gap-x-5 gap-y-1 text-[10px] text-muted-foreground md:col-auto"><span><strong className="block text-xs text-foreground">{duration(value)}</strong>{t('executions.durationLabel')}</span><span><strong className="block text-xs text-foreground">{localizedValue(t, 'executions.triggerTypes', value.triggerType)}</strong>{t('executions.triggerLabel')}</span><span><strong className="block max-w-36 truncate text-xs text-foreground">{value.traceId}</strong>Trace</span></div>
        <div className="col-start-2 flex items-center justify-self-end gap-2 md:col-auto md:ml-auto">
          <Button aria-label={t('executions.refresh')} onClick={() => void refresh()} size="icon" variant="ghost"><RefreshCw className="size-4" /></Button>
          {auth.hasPermission('execution:cancel') && active && <Button onClick={() => setCancelOpen(true)} variant="secondary"><Ban className="size-4" />{t('executions.cancelExecution')}</Button>}
          {auth.hasPermission('execution:fork') && <Button disabled={!checkpoints.data?.items.length} onClick={() => setForkOpen(true)} ref={forkTrigger}><GitFork className="size-4" />{t('executions.forkExecution')}</Button>}
        </div>
      </div>
      {(value.parentExecutionId || value.callerExecutionId) && <div className="mt-2 flex flex-wrap gap-3 border-t border-border pt-2 text-[10px] text-muted-foreground">{value.parentExecutionId && <Link className="inline-flex items-center gap-1 text-primary hover:underline" to={`/executions/${value.parentExecutionId}`}><Link2 className="size-3" />{t('executions.parentExecution', { id: value.parentExecutionId })}</Link>}{value.callerExecutionId && <Link className="inline-flex items-center gap-1 text-primary hover:underline" to={`/executions/${value.callerExecutionId}`}><Link2 className="size-3" />{t('executions.callerExecution', { id: value.callerExecutionId })}</Link>}</div>}
      {value.endUser && <div className="mt-2 flex items-center gap-3 text-xs"><span className="text-muted-foreground">{t('applications.endUser')}</span><EndUserLabel user={value.endUser} /></div>}
      {value.errorMessage && <p className="mt-3 border-l-2 border-danger bg-danger/10 px-3 py-2 text-xs text-danger"><strong>{value.errorCode}</strong> {value.errorMessage}</p>}
    </header>

    <Tabs onValueChange={setMainView} value={mainView}>
      <TabsList className="h-11 border-b border-border bg-surface px-6"><TabsTrigger onClick={() => setMainView('trace')} value="trace">{t('executions.mainViews.trace')}</TabsTrigger>{canViewExecution && <TabsTrigger onClick={() => setMainView('recovery')} value="recovery">{t('executions.mainViews.recovery')}</TabsTrigger>}</TabsList>
      <TabsContent value="trace"><WorkflowInteractions messages={interactions.data?.messages} waits={interactions.data?.inputWaits} />{value.applicationId && auth.hasPermission('application:view') && <WorkflowHookStatus applicationId={value.applicationId} executionId={id} invocationId={value.invocationId} />}<TraceWorkspace active={mainView === 'trace'} className="min-h-[620px]" execution={value} executionId={id} nodes={nodeItems} onDownloadArtifact={downloadArtifact} onNodeSelect={(node) => setSelectedId(node.id)} /></TabsContent>
      {canViewExecution && <TabsContent value="recovery"><div className="min-h-[620px] overflow-hidden"><ExecutionRecoveryRail approvals={executionApprovals} canConfirm={auth.hasPermission('execution:fork')} checkpoints={checkpoints.data?.items ?? []} events={events.data?.items ?? []} nodes={nodeItems} onConfirm={setConfirmationNode} /></div></TabsContent>}
    </Tabs>

    <ExecutionForkDialog checkpoints={checkpoints.data?.items ?? []} initialNodeId={selectedNode?.nodeId} nodes={nodeItems} onClose={closeFork} onSubmit={(request) => fork.mutateAsync(request).then(() => undefined)} open={forkOpen} pending={fork.isPending} />
    <SideEffectDialog checkpointId={value.forkCheckpointId ?? undefined} node={confirmationNode} onClose={() => setConfirmationNode(undefined)} onSubmit={(request) => confirmation.mutateAsync(request).then(() => undefined)} pending={confirmation.isPending} />
    <ConfirmDialog cancelLabel={t('executions.return')} confirmLabel={t('executions.cancelExecution')} description={t('executions.cancelDescription')} onClose={() => setCancelOpen(false)} onConfirm={() => cancel.mutateAsync().then(() => undefined)} open={cancelOpen} pending={cancel.isPending} title={t('executions.cancelTitle')} />
  </div>
}

function duration(execution: Execution) {
  const milliseconds = execution.durationMs ?? (Date.now() - new Date(execution.startedAt).getTime())
  if (milliseconds < 1_000) return `${milliseconds} ms`
  if (milliseconds < 60_000) return `${(milliseconds / 1_000).toFixed(1)} s`
  return `${Math.floor(milliseconds / 60_000)}m ${Math.floor((milliseconds % 60_000) / 1_000)}s`
}
