import type { WorkflowDraft } from "../../shared/api/types"
import { WorkflowDraftsCard } from "./workflow-drafts-card"
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Archive, GitBranch, Plus, Rocket, ShieldCheck, UsersRound } from 'lucide-react'
import { useState, type Dispatch, type ReactNode, type SetStateAction } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { PageResponse, ResourceValidation, User, Workflow, WorkflowDeployment, WorkflowEnvironment, WorkflowMember, WorkflowVersion } from '../../shared/api/types'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { localizedValue } from '../../shared/lib/localized-value'
import { Badge } from '../../shared/ui/badge'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { StatusBadge } from '../../shared/components/status-badge'
import { useToast } from '../../shared/ui/toast'
import { WorkflowPublishDialog, type WorkflowPublishInput, type WorkflowPublishStage } from './workflow-publish-dialog'
import { WorkflowEnvironmentsCard, WorkflowReleaseHistoryDialog, WorkflowReleaseOverview, WorkflowVersionsCard } from './workflow-release-overview'

type ExecutionCommandResponse = { executionId: string; status: string; replayed: boolean }
type PublishDefaults = { environmentId?: string; versionId?: string }

export function WorkflowDetailPage() {
  const { workflowId = '' } = useParams()
  const { t } = useTranslation()
  const auth = useAuth()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [publishOpen, setPublishOpen] = useState(false)
  const [publishDefaults, setPublishDefaults] = useState<PublishDefaults>({})
  const [historyOpen, setHistoryOpen] = useState(false)
  const [memberOpen, setMemberOpen] = useState(false)
  const workflow = useQuery({ queryKey: ['workflow', workflowId], queryFn: () => apiRequest<Workflow>(`/workflows/${workflowId}`) })
  const [selectedDraftId, setSelectedDraftId] = useState('')
  const drafts = useQuery({queryKey:['workflow-drafts',workflowId],queryFn:() => apiRequest<WorkflowDraft[]>(`/workflows/${workflowId}/drafts`)})
  const selectedDraft = drafts.data?.find(d => d.id === selectedDraftId && d.status === 'active')
  const versions = useQuery({ queryKey: ['workflow-versions', workflowId], queryFn: () => apiRequest<WorkflowVersion[]>(`/workflows/${workflowId}/versions`) })
  const deployments = useQuery({ queryKey: ['workflow-deployments', workflowId], queryFn: () => apiRequest<WorkflowDeployment[]>(`/workflows/${workflowId}/deployments`) })
  const environments = useQuery({ queryKey: ['environments'], queryFn: () => apiRequest<WorkflowEnvironment[]>('/environments') })
  const members = useQuery({ queryKey: ['workflow-members', workflowId], queryFn: () => apiRequest<WorkflowMember[]>(`/workflows/${workflowId}/members`) })
  const validation = useQuery({ enabled: Boolean(selectedDraft), queryKey: ['workflow-validation', workflowId, selectedDraftId], queryFn: () => apiRequest<ResourceValidation>(`/workflows/${workflowId}/drafts/${selectedDraftId}/resource-validation`) })
  const users = useQuery({ queryKey: ['users', 'workflow-member'], queryFn: () => apiRequest<PageResponse<User>>('/users?pageSize=100') })
  const invalidate = () => Promise.all([
    queryClient.invalidateQueries({ queryKey: ['workflow', workflowId] }),
    queryClient.invalidateQueries({ queryKey: ['workflow-versions', workflowId] }),
    queryClient.invalidateQueries({ queryKey: ['workflow-deployments', workflowId] }),
    queryClient.invalidateQueries({ queryKey: ['workflow-members', workflowId] }),
    queryClient.invalidateQueries({ queryKey: ['workflow-validation', workflowId] }),
  ])
  const createVersionRequest = () => { if (!selectedDraft) throw new Error(t("workflows.selectDraft")); return apiRequest<WorkflowVersion>(`/workflows/${workflowId}/versions`, { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body: jsonBody({ draftId:selectedDraft.id, draftRevision:selectedDraft.revision }) }) }
  const createVersion = useMutation({ mutationFn: createVersionRequest, onSuccess: async (version) => { await invalidate(); showToast(t('workflows.versionSaved', { version: version.versionNumber })) }, onError: (error: Error) => showToast(error.message) })
  const runVersion = useMutation({
    mutationFn: (versionId: string) => apiRequest<ExecutionCommandResponse>(`/workflow-versions/${versionId}/executions`, { method: 'POST', body: jsonBody({ input: {}, idempotencyKey: crypto.randomUUID() }) }),
    onSuccess: (execution) => navigate(`/executions/${execution.executionId}`),
    onError: (error: Error) => showToast(error.message),
  })
  const archive = useMutation({ mutationFn: () => apiRequest(`/workflows/${workflowId}/archive`, { method: 'POST' }), onSuccess: async () => { await invalidate(); navigate('/workflows') }, onError: (error: Error) => showToast(error.message) })
  const rollback = useMutation({ mutationFn: (deployment: WorkflowDeployment) => apiRequest(`/workflows/${workflowId}/deployments/${deployment.environmentId}/rollback`, { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body: jsonBody({ targetWorkflowVersionId: deployment.workflowVersionId }) }), onSuccess: async (_, deployment) => { await invalidate(); setHistoryOpen(false); showToast(t('workflows.rolledBackToVersion', { environment: deployment.environmentName, version: deployment.versionNumber })) }, onError: (error: Error) => showToast(error.message) })
  const addMember = async (values: Record<string, string>) => { await apiRequest(`/workflows/${workflowId}/members`, { method: 'POST', body: jsonBody({ userId: values.user, memberRole: values.role }) }); await invalidate() }
  const value = workflow.data
  if (workflow.isLoading) return <PageContainer><p className="text-sm text-muted-foreground">{t('workflows.loading')}</p></PageContainer>
  if (!value) return <PageContainer><p className="text-sm text-danger">{String(workflow.error ?? t('workflows.loadFailed'))}</p></PageContainer>

  const versionItems = [...(versions.data ?? [])].sort((left, right) => right.versionNumber - left.versionNumber)
  const deploymentItems = [...(deployments.data ?? [])].sort((left, right) => Date.parse(right.createdAt) - Date.parse(left.createdAt))
  const environmentItems = (environments.data ?? []).filter((item) => item.status === 'active')
  const primaryEnvironment = environmentItems.find((item) => item.code.toLowerCase() === 'development') ?? environmentItems[0]
  const primaryDeployment = primaryEnvironment ? deploymentItems.find((item) => item.environmentId === primaryEnvironment.id && item.status === 'active') : undefined
  const dirty = Boolean(selectedDraft && !versionItems.some(v => v.sourceDraftId === selectedDraft.id && v.sourceRevision === selectedDraft.revision))
  const canPublish = auth.hasPermission('workflow:publish') && value.status === 'active'
  const openPublish = (defaults: PublishDefaults = {}) => { setPublishDefaults(defaults); setPublishOpen(true) }
  const submitPublish = async (input: WorkflowPublishInput, setStage: Dispatch<SetStateAction<WorkflowPublishStage>>) => {
    let version = versionItems.find((item) => item.id === input.workflowVersionId)
    if (input.useDraft) {
      setStage('creating')
      version = await createVersionRequest()
    }
    if (!version) throw new Error(t('workflows.publishDialog.versionRequired'))
    const active = deploymentItems.find((item) => item.environmentId === input.environmentId && item.status === 'active')
    if (active?.workflowVersionId === version.id) throw new Error(t('workflows.publishDialog.noOp', { environment: active.environmentName, version: version.versionNumber }))
    setStage('publishing')
    await apiRequest(`/workflows/${workflowId}/deployments`, { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body: jsonBody({ environmentId: input.environmentId, workflowVersionId: version.id }) })
    await invalidate()
    const environment = environmentItems.find((item) => item.id === input.environmentId)
    showToast(t('workflows.publishedToEnvironment', { environment: environment?.name, version: version.versionNumber }))
  }
  const memberFields: EntityFormField[] = [
    { name: 'user', label: t('navigation.header.user'), type: 'select', required: true, options: (users.data?.items ?? []).map((item) => ({ value: item.id, label: `${item.displayName} · ${item.username}` })) },
    { name: 'role', label: t('navigation.header.roles'), type: 'select', defaultValue: 'viewer', required: true, options: ['viewer', 'editor', 'manager'].map((item) => ({ value: item, label: localizedValue(t, 'workflows.memberRoles', item) })) },
  ]

  return <PageContainer>
    <PageHeader action={<div className="flex flex-wrap justify-end gap-2"><Button variant="secondary" disabled={!selectedDraft} onClick={() => { if (selectedDraft) navigate(`/workflows/${workflowId}/drafts/${selectedDraft.id}/editor`) }}><GitBranch className="size-4" />{t('workflows.openEditor')}</Button>{auth.hasPermission('workflow:publish') && <Button disabled={!canPublish || environments.isLoading || environmentItems.length === 0} onClick={() => openPublish({ environmentId: primaryEnvironment?.id })}><Rocket className="size-4" />{t('workflows.publishToEnvironment')}</Button>}</div>} description={value.description ?? t('workflows.description')} title={value.name} />
    <div className="mt-5 flex flex-wrap items-center gap-3 text-xs"><StatusBadge status={value.status === 'active' ? 'active' : 'inactive'} /><span>{localizedValue(t, 'workflows.visibilities', value.visibility)}</span><span className="text-muted-foreground">{t('workflows.owner', { owner: value.ownerName })}</span>{dirty && <Badge tone="warning">{t('workflows.unpublishedChanges')}</Badge>}</div>
    <WorkflowReleaseOverview deployments={deploymentItems} primaryDeployment={primaryDeployment} primaryEnvironment={primaryEnvironment} versions={versionItems} workflow={value} />

    <div className="mt-5 grid items-start gap-5 xl:grid-cols-[minmax(0,1.45fr)_minmax(320px,.7fr)]">
      <div className="grid gap-5">
        <WorkflowDraftsCard workflowId={workflowId} drafts={drafts.data ?? []} versions={versionItems} canEdit={value.canEdit && value.status === "active"} selectedId={selectedDraftId} onSelect={setSelectedDraftId} />
        <ResourceValidationCard validation={validation.data} />
        <WorkflowVersionsCard canCreate={canPublish} canRun={auth.hasPermission('execution:run')} creating={createVersion.isPending} deployments={deploymentItems} draftDirty={dirty} onCreate={() => createVersion.mutate()} onPublish={(versionId) => openPublish({ environmentId: primaryEnvironment?.id, versionId })} onRun={(versionId) => runVersion.mutate(versionId)} running={runVersion.isPending} versions={versionItems} />
      </div>
      <div className="grid gap-5">
        <Card className="overflow-hidden"><div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('workflows.overview')}</h2></div><dl className="space-y-4 p-5 text-xs"><Info label={t('workflows.visibility')} value={localizedValue(t, 'workflows.visibilities', value.visibility)} /><Info label={t('workflows.workflowId')} value={value.id} /><Info label={t('workflows.serviceIdentity')} value={value.serviceIdentityId} /></dl>{auth.hasPermission('workflow:archive') && value.status === 'active' && <div className="border-t border-border p-4"><Button className="w-full" disabled={archive.isPending} onClick={() => { if (window.confirm(t('workflows.confirmArchive'))) archive.mutate() }} variant="secondary"><Archive className="size-4" />{t('workflows.archive')}</Button></div>}</Card>
        <WorkflowEnvironmentsCard canPublish={canPublish} deployments={deploymentItems} environments={environmentItems} onHistory={() => setHistoryOpen(true)} onPublish={(environmentId) => openPublish({ environmentId })} />
        <SectionCard action={auth.hasPermission('workflow:manage_member') ? <Button onClick={() => setMemberOpen(true)} size="sm" variant="secondary"><Plus className="size-3.5" />{t('workflows.addMember')}</Button> : undefined} icon={UsersRound} title={t('workflows.members')}>{members.data?.map((member) => <Row key={member.userId} primary={member.displayName} secondary={`${member.username} · ${localizedValue(t, 'workflows.memberRoles', member.memberRole)}`} />)}{members.data?.length === 0 && <Empty />}</SectionCard>
      </div>
    </div>

    <WorkflowPublishDialog defaultEnvironmentId={publishDefaults.environmentId} defaultVersionId={publishDefaults.versionId} deployments={deploymentItems} dirty={dirty} draftRevision={selectedDraft?.revision ?? 0} environments={environmentItems} onClose={() => setPublishOpen(false)} onSubmit={submitPublish} open={publishOpen} versions={versionItems} />
    <WorkflowReleaseHistoryDialog canRollback={canPublish} deployments={deploymentItems} onClose={() => setHistoryOpen(false)} onRollback={(deployment) => rollback.mutate(deployment)} open={historyOpen} rollingBack={rollback.isPending} />
    {memberOpen && <EntityFormDialog cancelLabel={t('common.cancel')} fields={memberFields} onClose={() => setMemberOpen(false)} onSubmit={addMember} open submitLabel={t('common.save')} title={t('workflows.addMember')} />}
  </PageContainer>
}

function ResourceValidationCard({ validation }: { validation?: ResourceValidation }) {
  const { t } = useTranslation()
  return <Card className="overflow-hidden"><div className="flex items-center gap-2 border-b border-border px-5 py-4"><ShieldCheck className="size-4 text-primary" /><h2 className="text-sm font-semibold">{t('workflows.resources')}</h2></div><div className="p-5"><div className={`rounded-lg border p-4 text-xs ${validation?.valid ? 'border-success/20 bg-success/10 text-success' : 'border-warning/20 bg-warning/10 text-warning'}`}><div className="flex items-center gap-2"><ShieldCheck className="size-4" />{validation?.valid ? t('workflows.validationPassed') : t('workflows.validationFailed', { count: validation?.missingGrants.length ?? 0 })}<span className="ml-auto text-[10px] opacity-75">{validation?.valid ? t('workflows.readyToRelease') : t('workflows.resolveBeforeRelease')}</span></div>{validation?.missingGrants.map((issue) => <p className="mt-2 font-mono text-[11px]" key={`${issue.nodeId}-${issue.resourceId}-${issue.operation}`}>{issue.nodeId}: {localizedValue(t, 'resourceGrants.resourceTypes', issue.resourceType)}/{issue.resourceId} · {localizedValue(t, 'resourceGrants.validationReasons', issue.reason)}</p>)}</div></div></Card>
}

function Info({ label, value }: { label: string; value: string }) { return <div><dt className="text-muted-foreground">{label}</dt><dd className="mt-1 break-all font-mono text-[11px]">{value}</dd></div> }
function Row({ primary, secondary }: { primary: string; secondary: string }) { return <div className="border-t border-border py-3 first:border-t-0"><p className="text-xs font-medium">{primary}</p><p className="mt-1 text-[11px] text-muted-foreground">{secondary}</p></div> }
function Empty() { const { t } = useTranslation(); return <p className="py-4 text-xs text-muted-foreground">{t('workflows.noData')}</p> }
function SectionCard({ action, children, icon: Icon, title }: { action?: ReactNode; children: ReactNode; icon: typeof UsersRound; title: string }) { return <Card className="p-5"><div className="flex items-center"><Icon className="mr-2 size-4 text-primary" /><h2 className="text-sm font-semibold">{title}</h2><div className="flex-1" />{action}</div><div className="mt-4">{children}</div></Card> }
