import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import { Blocks, CloudCog, Plus } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { PageResponse, Workflow } from '../../shared/api/types'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'
import { EntityDeleteButton } from '../../shared/components/entity-delete-button'
import { EntityCell } from '../../shared/components/entity-cell'
import { ListPage } from '../../shared/components/list-page'
import { StatusBadge } from '../../shared/components/status-badge'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { Button } from '../../shared/ui/button'
import { useToast } from '../../shared/ui/toast'

export function WorkflowsPage() {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  const auth = useAuth()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [open, setOpen] = useState(false)
  const query = useQuery({ queryKey: ['workflows'], queryFn: () => apiRequest<PageResponse<Workflow>>('/workflows?pageSize=100') })
  const create = useMutation({
    mutationFn: (values: Record<string, string>) => apiRequest<Workflow>('/workflows', { method: 'POST', body: jsonBody({ name: values.name, description: values.description || null, visibility: values.visibility }) }),
    onSuccess: async (workflow) => { await queryClient.invalidateQueries({ queryKey: ['workflows'] }); showToast(t('workflows.created')); navigate(`/workflows/${workflow.id}`) },
  })
  const columns = useMemo<Array<ColumnDef<Workflow>>>(() => [
    { accessorKey: 'name', header: t('workflows.title'), cell: ({ row }) => <EntityCell detail={row.original.description ?? t('workflows.draftLabel')} icon={Blocks} name={row.original.name} /> },
    { accessorKey: 'status', header: t('common.status'), cell: ({ row }) => <StatusBadge status={row.original.status === 'active' ? 'active' : 'inactive'} /> },
    { accessorKey: 'draftRevision', header: t('workflows.draftRevision') },
    { accessorKey: 'latestVersion', header: t('workflows.latestVersion'), cell: ({ row }) => row.original.latestVersion ? `v${row.original.latestVersion}` : '—' },
    { accessorKey: 'ownerName', header: t('common.owner') },
    { accessorKey: 'updatedAt', header: t('common.updatedAt'), cell: ({ row }) => formatDateTime(row.original.updatedAt) },
    { id: 'actions', header: '', cell: ({ row }) => <div className="flex gap-1"><Button asChild size="sm" variant="ghost"><Link to={`/workflows/${row.original.id}`}>{t('workflows.details')}</Link></Button><Button asChild size="sm" variant="ghost"><Link to={`/workflows/${row.original.id}/editor`}>{t('workflows.openEditor')}</Link></Button><EntityDeleteButton canDelete={auth.hasPermission('workflow:delete')} deletePath={`/workflows/${row.original.id}`} entityId={row.original.id} entityName={row.original.name} entityType="workflow" onDeleted={async () => { await queryClient.invalidateQueries({ queryKey: ['workflows'] }); showToast(t('workflows.deleted')) }} /></div> },
  ], [auth, formatDateTime, queryClient, showToast, t])
  const fields: EntityFormField[] = [
    { name: 'name', label: t('workflows.workflowName'), required: true },
    { name: 'description', label: t('workflows.fields.description'), type: 'textarea' },
    { name: 'visibility', label: t('workflows.visibility'), type: 'select', defaultValue: 'private', required: true, options: [{ value: 'private', label: t('workflows.private') }, { value: 'department', label: t('workflows.departmentVisible') }, { value: 'company', label: t('workflows.companyVisible') }] },
  ]
  return <>
    <ListPage loading={query.isLoading} error={query.error} action={<div className="flex gap-2"><Button asChild variant="secondary"><Link to="/environments"><CloudCog className="size-4" />{t('workflows.manageEnvironments')}</Link></Button>{auth.hasPermission('workflow:create') && <Button onClick={() => setOpen(true)}><Plus className="size-4" />{t('workflows.createWorkflow')}</Button>}</div>} columns={columns} data={query.data?.items ?? []} description={t('workflows.description')} getSearchText={(row) => `${row.name} ${row.ownerName}`} getStatus={(row) => row.status === 'active' ? 'active' : 'inactive'} searchPlaceholder={t('workflows.search')} statusOptions={[{ value: 'active', label: t('common.active') }, { value: 'inactive', label: t('common.inactive') }]} title={t('workflows.title')} />
    {open && <EntityFormDialog cancelLabel={t('common.cancel')} fields={fields} onClose={() => setOpen(false)} onSubmit={(values) => create.mutateAsync(values).then(() => undefined)} open submitLabel={t('common.save')} title={t('workflows.createWorkflow')} />}
  </>
}
