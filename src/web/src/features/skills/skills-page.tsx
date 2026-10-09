import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import { Plus, Sparkles } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { Department, PageResponse, Skill } from '../../shared/api/types'
import { EntityCell } from '../../shared/components/entity-cell'
import { EntityDeleteButton } from '../../shared/components/entity-delete-button'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'
import { ListPage } from '../../shared/components/list-page'
import { StatusBadge } from '../../shared/components/status-badge'
import { Button } from '../../shared/ui/button'
import { useToast } from '../../shared/ui/toast'

export function SkillsPage() {
  const { t } = useTranslation()
  const auth = useAuth()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [open, setOpen] = useState(false)
  const skills = useQuery({ queryKey: ['skills'], queryFn: () => apiRequest<PageResponse<Skill>>('/skills?pageSize=100') })
  const departments = useQuery({ queryKey: ['departments'], queryFn: () => apiRequest<Department[]>('/departments') })
  const create = useMutation({ mutationFn: (values: Record<string, string>) => apiRequest<Skill>('/skills', { method: 'POST', body: jsonBody({ name: values.name, alias: values.alias, description: values.description, ownerDepartmentId: values.department }) }), onSuccess: async () => { await queryClient.invalidateQueries({ queryKey: ['skills'] }); showToast(t('skills.created')) } })
  const columns = useMemo<Array<ColumnDef<Skill>>>(() => [
    { accessorKey: 'name', header: t('skills.fields.name'), cell: ({ row }) => <EntityCell detail={`${row.original.alias} · ${t('skills.workspaceRevision', { revision: row.original.draftRevision })}`} icon={Sparkles} name={row.original.name} /> },
    { accessorKey: 'latestVersion', header: t('skills.version'), cell: ({ row }) => row.original.latestVersion ? `v${row.original.latestVersion}` : '—' },
    { accessorKey: 'draftRevision', header: t('skills.workspaceRevisionShort'), cell: ({ row }) => `r${row.original.draftRevision}` },
    { accessorKey: 'grantCount', header: t('skills.workflows') },
    { accessorKey: 'status', header: t('common.status'), cell: ({ row }) => <StatusBadge status={row.original.status === 'active' ? 'active' : row.original.status === 'draft' ? 'draft' : 'inactive'} /> },
    { id: 'actions', header: '', cell: ({ row }) => <div className="flex gap-1"><Button asChild size="sm" variant="ghost"><Link to={`/skills/${row.original.id}`}>{t('skills.details')}</Link></Button><EntityDeleteButton canDelete={auth.hasPermission('skill:delete')} deletePath={`/skills/${row.original.id}`} entityId={row.original.id} entityName={row.original.name} entityType="skill" onDeleted={async () => { await queryClient.invalidateQueries({ queryKey: ['skills'] }); showToast(t('skills.deleted')) }} /></div> },
  ], [auth, queryClient, showToast, t])
  const fields: EntityFormField[] = [{ name: 'name', label: t('skills.fields.name'), maxLength: 160, required: true }, { name: 'alias', label: t('skills.fields.alias'), description: t('skills.fields.aliasHint'), maxLength: 160, required: true }, { name: 'description', label: t('skills.fields.description'), type: 'textarea', description: t('skills.fields.descriptionHint'), maxLength: 1000, required: true }, { name: 'department', label: t('skills.department'), type: 'select', required: true, options: (departments.data ?? []).map((item) => ({ value: item.id, label: item.name })) }]
  return <>
    <ListPage loading={skills.isLoading} error={skills.error} action={auth.hasPermission('skill:manage') ? <Button onClick={() => setOpen(true)}><Plus className="size-4" />{t('skills.createSkill')}</Button> : undefined} columns={columns} data={skills.data?.items ?? []} description={t('skills.description')} getSearchText={(row) => `${row.name} ${row.alias} ${row.description ?? ''}`} getStatus={(row) => row.status === 'active' ? 'active' : row.status === 'draft' ? 'draft' : 'inactive'} searchPlaceholder={t('skills.search')} statusOptions={[{ value: 'active', label: t('common.active') }, { value: 'draft', label: t('common.draft') }, { value: 'inactive', label: t('common.inactive') }]} title={t('skills.title')} />
    {open && <EntityFormDialog cancelLabel={t('common.cancel')} fields={fields} onClose={() => setOpen(false)} onSubmit={(values) => create.mutateAsync(values).then(() => undefined)} open submitLabel={t('common.save')} title={t('skills.createSkill')} />}
  </>
}
