import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import { FlaskConical, Plus, Ruler } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate, useSearchParams } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { Dataset, DatasetVersion, EvaluationProfile, EvaluationRun, PageResponse, Workflow, WorkflowVersion } from '../../shared/api/types'
import { EntityCell } from '../../shared/components/entity-cell'
import { EntityDeleteButton } from '../../shared/components/entity-delete-button'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'
import { ListPage } from '../../shared/components/list-page'
import { PrerequisiteAction, type Prerequisite } from '../../shared/components/prerequisite-action'
import { StatusBadge } from '../../shared/components/status-badge'
import { localizedValue } from '../../shared/lib/localized-value'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../shared/ui/tabs'
import { useToast } from '../../shared/ui/toast'
import { EvaluationProfileDialog, type EvaluationProfileDraft } from './evaluation-profile-dialog'

type VersionOption = { id: string; label: string }

export function EvaluationsPage() {
  const { t } = useTranslation()
  const auth = useAuth()
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [selectedRuns, setSelectedRuns] = useState<string[]>([])
  const [runOpen, setRunOpen] = useState(false)
  const [profileOpen, setProfileOpen] = useState(false)
  const activeTab = searchParams.get('tab') === 'profiles' ? 'profiles' : 'runs'
  const runs = useQuery({ queryKey: ['evaluations'], queryFn: () => apiRequest<EvaluationRun[]>('/evaluations') })
  const profiles = useQuery({ queryKey: ['evaluation-profiles'], enabled: auth.hasPermission('evaluation_profile:view'), queryFn: () => apiRequest<EvaluationProfile[]>('/evaluation-profiles') })
  const workflowVersions = useQuery({ queryKey: ['evaluation-workflow-versions'], queryFn: async () => { const workflows = await apiRequest<PageResponse<Workflow>>('/workflows?pageSize=100&status=active'); const values = await Promise.all(workflows.items.map(async (workflow) => (await apiRequest<WorkflowVersion[]>(`/workflows/${workflow.id}/versions`)).map((version) => ({ id: version.id, label: `${workflow.name} · v${version.versionNumber}` })))); return values.flat() } })
  const datasetVersions = useQuery({ queryKey: ['evaluation-dataset-versions'], queryFn: async () => { const datasets = await apiRequest<PageResponse<Dataset>>('/datasets?pageSize=100&status=active'); const values = await Promise.all(datasets.items.map(async (dataset) => (await apiRequest<DatasetVersion[]>(`/datasets/${dataset.id}/versions`)).map((version) => ({ id: version.id, label: `${dataset.name} · v${version.versionNumber}` })))); return values.flat() } })
  const createRun = useMutation({
    mutationFn: (values: Record<string, string>) => apiRequest<EvaluationRun>('/evaluations', { method: 'POST', body: jsonBody({ name: values.name, workflowVersionId: values.workflowVersionId, datasetVersionId: values.datasetVersionId, evaluationProfileVersionId: values.profileVersionId, parameters: JSON.parse(values.parameters || '{}') }) }),
    onSuccess: async (value) => { await queryClient.invalidateQueries({ queryKey: ['evaluations'] }); showToast(t('evaluations.created')); navigate(`/evaluations/${value.id}`) },
  })
  const createProfile = useMutation({
    mutationFn: (value: EvaluationProfileDraft) => apiRequest<EvaluationProfile>('/evaluation-profiles', { method: 'POST', body: jsonBody(value) }),
    onSuccess: async () => { await queryClient.invalidateQueries({ queryKey: ['evaluation-profiles'] }); showToast(t('evaluations.created')) },
  })
  const runColumns = useMemo<Array<ColumnDef<EvaluationRun>>>(() => [
    { id: 'compare', header: t('evaluations.compare'), cell: ({ row }) => <input aria-label={t('evaluations.compareRun', { name: row.original.name })} checked={selectedRuns.includes(row.original.id)} disabled={!selectedRuns.includes(row.original.id) && selectedRuns.length >= 5} onChange={(event) => setSelectedRuns((current) => event.target.checked ? [...current, row.original.id] : current.filter((id) => id !== row.original.id))} type="checkbox" /> },
    { accessorKey: 'name', header: t('evaluations.report'), cell: ({ row }) => <EntityCell detail={row.original.workflowName} icon={FlaskConical} name={row.original.name} /> },
    { accessorKey: 'workflowName', header: t('evaluations.workflowVersion') },
    { accessorKey: 'datasetName', header: t('evaluations.dataset') },
    { accessorKey: 'resultCount', header: t('evaluations.results') },
    { accessorKey: 'status', header: t('common.status'), cell: ({ row }) => <StatusBadge label={localizedValue(t, 'common', row.original.status)} status={row.original.status === 'completed' ? 'completed' : row.original.status === 'created' ? 'draft' : row.original.status === 'cancelled' ? 'inactive' : row.original.status as 'running' | 'failed'} /> },
    { id: 'actions', header: '', cell: ({ row }) => <Button asChild size="sm" variant="ghost"><Link to={`/evaluations/${row.original.id}`}>{t('evaluations.report')}</Link></Button> },
  ], [selectedRuns, t])
  const runFields: EntityFormField[] = [
    { name: 'name', label: t('common.name'), required: true },
    { name: 'workflowVersionId', label: t('evaluations.workflowVersion'), type: 'select', required: true, options: options(workflowVersions.data) },
    { name: 'datasetVersionId', label: t('evaluations.dataset'), type: 'select', required: true, options: options(datasetVersions.data) },
    { name: 'profileVersionId', label: t('evaluations.profile'), type: 'select', required: true, options: (profiles.data ?? []).map((item) => ({ value: item.versionId, label: `${item.name} · v${item.versionNumber}` })) },
    { name: 'parameters', label: t('evaluations.parameters'), type: 'textarea', defaultValue: '{}' },
  ]
  const prerequisites: Prerequisite[] = [
    { key: 'workflow-version', label: t('evaluations.prerequisites.workflowVersion'), met: Boolean(workflowVersions.data?.length), href: auth.hasPermission('workflow:create') ? '/workflows' : undefined, actionLabel: t('evaluations.prerequisites.goWorkflows') },
    { key: 'dataset-version', label: t('evaluations.prerequisites.datasetVersion'), met: Boolean(datasetVersions.data?.length), href: auth.hasPermission('dataset:manage') ? '/datasets' : undefined, actionLabel: t('evaluations.prerequisites.goDatasets') },
    { key: 'evaluation-profile', label: t('evaluations.prerequisites.profile'), met: Boolean(profiles.data?.length), href: auth.hasPermission('evaluation_profile:manage') ? '/evaluations?tab=profiles' : undefined, actionLabel: t('evaluations.prerequisites.createProfile') },
  ]
  return <>
    <Tabs onValueChange={(value) => setSearchParams(value === 'profiles' ? { tab: 'profiles' } : {})} value={activeTab}>
      <div className="px-8 pt-7"><TabsList className="border-b border-border"><TabsTrigger value="runs">{t('evaluations.title')}</TabsTrigger><TabsTrigger value="profiles">{t('evaluations.profiles')}</TabsTrigger></TabsList></div>
      <TabsContent value="runs"><ListPage action={<div className="flex items-center gap-2"><Button disabled={selectedRuns.length < 2} onClick={() => navigate(`/evaluations/${selectedRuns[0]}?${new URLSearchParams({ tab: 'compare', runIds: selectedRuns.join(',') })}`)} variant="secondary">{t('evaluations.compare')} ({selectedRuns.length}/5)</Button>{auth.hasPermission('evaluation:manage') ? <PrerequisiteAction description={t('evaluations.prerequisites.runDescription')} loading={workflowVersions.isLoading || datasetVersions.isLoading || profiles.isLoading} onReady={() => setRunOpen(true)} requirements={prerequisites}><Plus className="size-4" />{t('evaluations.create')}</PrerequisiteAction> : undefined}</div>} columns={runColumns} data={runs.data ?? []} description={t('evaluations.description')} getSearchText={(row) => `${row.name} ${row.workflowName} ${row.datasetName}`} getStatus={(row) => row.status === 'created' ? 'draft' : row.status === 'cancelled' ? 'inactive' : row.status as 'completed' | 'running' | 'failed'} searchPlaceholder={t('evaluations.search')} statusOptions={[{ value: 'draft', label: t('common.draft') }, { value: 'completed', label: t('common.completed') }, { value: 'inactive', label: t('common.inactive') }]} title={t('evaluations.title')} /></TabsContent>
      <TabsContent className="px-8 py-6" value="profiles"><ProfileSection action={auth.hasPermission('evaluation_profile:manage') && <Button onClick={() => setProfileOpen(true)} size="sm"><Plus className="size-4" />{t('evaluations.newProfile')}</Button>} canDelete={auth.hasPermission('evaluation_profile:delete')} onDeleted={async () => { await queryClient.invalidateQueries({ queryKey: ['evaluation-profiles'] }); showToast(t('evaluations.deleted')) }} profiles={profiles.data ?? []} /></TabsContent>
    </Tabs>
    {runOpen && <EntityFormDialog cancelLabel={t('common.cancel')} fields={runFields} onClose={() => setRunOpen(false)} onSubmit={(values) => createRun.mutateAsync(values).then(() => undefined)} open submitLabel={t('common.save')} title={t('evaluations.run')} />}
    {profileOpen && <EvaluationProfileDialog onClose={() => setProfileOpen(false)} onSubmit={(value) => createProfile.mutateAsync(value).then(() => undefined)} open />}
  </>
}

function options(items?: VersionOption[]) { return (items ?? []).map((item) => ({ value: item.id, label: item.label })) }
function ProfileSection({ action, canDelete, onDeleted, profiles }: { action?: React.ReactNode; canDelete: boolean; onDeleted: () => Promise<void>; profiles: EvaluationProfile[] }) {
  const { t } = useTranslation()
  return <Card className="overflow-hidden"><div className="flex h-14 items-center justify-between border-b border-border px-5"><h2 className="flex items-center gap-2 text-sm font-semibold"><Ruler className="size-4 text-primary" />{t('evaluations.profiles')}</h2>{action}</div><div className="divide-y divide-border px-5">{profiles.map((profile) => <div className="flex items-center py-4" key={profile.id}><div><p className="text-xs font-medium">{profile.name}</p><p className="mt-1 text-[11px] text-muted-foreground">v{profile.versionNumber} · {profile.rules.length} {t('evaluations.scoringRules')} · {localizedValue(t, 'evaluations.aggregationOptions', profile.aggregation)}</p></div><div className="flex-1" /><StatusBadge label={localizedValue(t, 'common', profile.status)} status={profile.status === 'active' ? 'active' : 'inactive'} /><EntityDeleteButton canDelete={canDelete} deletePath={`/evaluation-profiles/${profile.id}`} entityId={profile.id} entityName={profile.name} entityType="evaluation_profile" onDeleted={onDeleted} /></div>)}{profiles.length === 0 && <p className="py-10 text-center text-xs text-muted-foreground">{t('evaluations.noProfiles')}</p>}</div></Card>
}
