import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, UsersRound } from 'lucide-react'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams } from 'react-router-dom'
import { useAuth } from '../../app/providers/auth-provider'
import { ApiClientError, apiRequest, jsonBody } from '../../shared/api/client'
import type { Workflow, WorkflowDeployment, WorkflowEnvironment, WorkflowVersionSummary } from '../../shared/api/types'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { Button } from '../../shared/ui/button'
import { useToast } from '../../shared/ui/toast'
import { WorkflowPublishDialog } from './workflow-publish-dialog'
import { WorkflowMembersDialog } from './workflow-members-dialog'
import { WorkflowEnvironmentsCard, WorkflowReleaseHistoryDialog, WorkflowVersionsCard } from './workflow-release-overview'

export function WorkflowDetailPage() {
  const { workflowId = '' } = useParams()
  const { t } = useTranslation()
  const auth = useAuth()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [publishVersion, setPublishVersion] = useState<WorkflowVersionSummary>()
  const [historyOpen, setHistoryOpen] = useState(false)
  const [memberOpen, setMemberOpen] = useState(false)
  const workflow = useQuery({ queryKey: ['workflow', workflowId], queryFn: () => apiRequest<Workflow>(`/workflows/${workflowId}`) })
  const versions = useQuery({ queryKey: ['workflow-versions', workflowId], queryFn: () => apiRequest<WorkflowVersionSummary[]>(`/workflows/${workflowId}/versions`) })
  const deployments = useQuery({ queryKey: ['workflow-deployments', workflowId], queryFn: () => apiRequest<WorkflowDeployment[]>(`/workflows/${workflowId}/deployments`) })
  const environments = useQuery({ queryKey: ['environments'], refetchOnWindowFocus: 'always', queryFn: () => apiRequest<WorkflowEnvironment[]>('/environments') })
  const invalidate = () => Promise.all([
    queryClient.invalidateQueries({ queryKey: ['workflow', workflowId] }), queryClient.invalidateQueries({ queryKey: ['workflow-versions', workflowId] }),
    queryClient.invalidateQueries({ queryKey: ['workflow-deployments', workflowId] }), queryClient.invalidateQueries({ queryKey: ['workflows'] }),
  ])
  const createKey=useRef(crypto.randomUUID())
  const create = useMutation({ mutationFn: () => apiRequest<{ id: string }>(`/workflows/${workflowId}/versions`, { method: 'POST', headers: { 'Idempotency-Key': createKey.current } }), onSuccess: async version => { createKey.current=crypto.randomUUID(); await invalidate(); navigate(`/workflows/${workflowId}/versions/${version.id}/editor`) }, onError: (error: Error) => showToast(error.message) })
  const archive = useMutation({ mutationFn: (version: WorkflowVersionSummary) => apiRequest(`/workflows/${workflowId}/versions/${version.id}`, { method: 'PATCH', body: jsonBody({ archived: !version.archived }) }), onSuccess: invalidate, onError: (error: Error) => showToast(error.message) })
  const rollback = useMutation({ mutationFn: (deployment: WorkflowDeployment) => apiRequest(`/workflows/${workflowId}/deployments/${deployment.environmentId}/rollback`, { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body: jsonBody({ targetWorkflowVersionId: deployment.workflowVersionId }) }), onSuccess: async () => { await invalidate(); setHistoryOpen(false) }, onError: (error: Error) => showToast(error.message) })
  if (workflow.isLoading) return <PageContainer><p className="text-sm text-muted-foreground">{t('workflows.loading')}</p></PageContainer>
  if (!workflow.data) return <PageContainer><p role="alert" className="text-sm text-danger">{String(workflow.error ?? t('workflows.loadFailed'))}</p></PageContainer>
  const value = workflow.data
  const versionItems = [...versions.data ?? []].sort((left, right) => right.versionNumber - left.versionNumber)
  const deploymentItems = [...deployments.data ?? []].sort((left, right) => Date.parse(right.createdAt) - Date.parse(left.createdAt))
  const canEdit = auth.hasPermission('workflow:edit') && value.canEdit
  const canPublish = auth.hasPermission('workflow:publish') && value.canEdit
  const error = versions.error ?? deployments.error ?? environments.error
  return <PageContainer>
    <PageHeader title={value.name} description={value.description ?? t('workflows.description')} action={<div className="flex items-center gap-2"><Button variant="secondary" onClick={() => setMemberOpen(true)}><UsersRound className="size-4" />{t('workflows.versionCanvas.manageMembers')}</Button>{canEdit && <Button disabled={create.isPending} onClick={() => create.mutate()}><Plus className="size-4" />{t('workflows.versionCanvas.newCanvas')}</Button>}</div>} />
    <div className="mt-5 grid gap-5">
      {error && <p role="alert" className="text-xs text-danger">{error.message}</p>}
      <WorkflowEnvironmentsCard deployments={deploymentItems} environments={(environments.data ?? []).filter(item => item.status === 'active')} onHistory={() => setHistoryOpen(true)} />
      {versions.isLoading ? <p className="text-xs text-muted-foreground">{t('workflows.loading')}</p> : <WorkflowVersionsCard workflowId={workflowId} versions={versionItems} deployments={deploymentItems} canEdit={canEdit} canPublish={canPublish} canArchive={value.canEdit && auth.hasPermission('workflow:archive')} onPublish={setPublishVersion} onArchive={version => archive.mutate(version)} />}
    </div>
    {publishVersion && <WorkflowPublishDialog environmentsLoading={environments.isLoading} environmentsError={environments.error?.message} onRetryEnvironments={() => { void environments.refetch() }} version={publishVersion} environments={(environments.data ?? []).filter(item => item.status === 'active')} deployments={deploymentItems} onClose={() => setPublishVersion(undefined)} onSubmit={async (input, key) => {
      const deployment = await apiRequest<WorkflowDeployment>(`/workflows/${workflowId}/deployments`, { method: 'POST', headers: { 'Idempotency-Key': key }, body: jsonBody(input) }).catch(async error=>{
        if(error instanceof ApiClientError && error.status===409)await invalidate()
        throw error
      })
      await invalidate(); showToast(t('workflows.publishedToEnvironment', { version: deployment.versionNumber, environment: deployment.environmentName }))
    }} />}
    <WorkflowReleaseHistoryDialog canRollback={canPublish} deployments={deploymentItems} onClose={() => setHistoryOpen(false)} onRollback={deployment => rollback.mutate(deployment)} open={historyOpen} rollingBack={rollback.isPending} />
    {memberOpen && <WorkflowMembersDialog workflowId={workflowId} ownerId={value.ownerUserId} canManage={value.canEdit && auth.hasPermission('workflow:manage_member')} onClose={() => setMemberOpen(false)} />}
  </PageContainer>
}
