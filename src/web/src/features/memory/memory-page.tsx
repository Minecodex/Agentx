import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import { MemoryStick, Plus } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { ExternalConnection } from '../../shared/api/types'
import { EntityCell } from '../../shared/components/entity-cell'
import { EntityDeleteButton } from '../../shared/components/entity-delete-button'
import { EntityFormDialog } from '../../shared/components/entity-form-dialog'
import { ListPage } from '../../shared/components/list-page'
import { StatusBadge } from '../../shared/components/status-badge'
import { localizedValue } from '../../shared/lib/localized-value'
import { Button } from '../../shared/ui/button'
import { useToast } from '../../shared/ui/toast'
import { useMemoryFields } from './use-memory-fields'

export function MemoryPage() {
  const { t } = useTranslation(); const auth = useAuth(); const queryClient = useQueryClient(); const { showToast } = useToast(); const [open, setOpen] = useState<'new'>()
  const resources = useQuery({ queryKey: ['memory'], queryFn: () => apiRequest<ExternalConnection[]>('/memory/connections') })
  const create = useMutation({ mutationFn: (values: Record<string, string>) => apiRequest<ExternalConnection>('/memory/connections', { method: 'POST', body: jsonBody({ name: values.name, endpoint: values.endpoint, healthPath: values.healthPath, credentialId: values.credential || null, ownerDepartmentId: values.department, configuration: {} }) }), onSuccess: async () => { await queryClient.invalidateQueries({ queryKey: ['memory'] }); showToast(t('memory.created')) } })
  const columns = useMemo<Array<ColumnDef<ExternalConnection>>>(() => [
    { accessorKey: 'name', header: t('memory.title'), cell: ({ row }) => <EntityCell detail={row.original.endpoint} icon={MemoryStick} name={row.original.name} /> }, { accessorKey: 'status', header: t('common.status'), cell: ({ row }) => <StatusBadge label={localizedValue(t, 'common', row.original.status)} status={row.original.status === 'active' ? 'active' : 'inactive'} /> }, { id: 'actions', header: '', cell: ({ row }) => <div className="flex gap-1"><Button asChild size="sm" variant="ghost"><Link to={`/memory/${row.original.id}`}>{t('memory.details')}</Link></Button><EntityDeleteButton canDelete={auth.hasPermission('memory:delete')} deletePath={`/memory/connections/${row.original.id}`} entityId={row.original.id} entityName={row.original.name} entityType="memory" onDeleted={async () => { await queryClient.invalidateQueries({ queryKey: ['memory'] }); showToast(t('memory.deleted')) }} /></div> },
  ], [auth, queryClient, showToast, t])
  const newFields = useMemoryFields()
  return <><ListPage loading={resources.isLoading} error={resources.error} action={auth.hasPermission('memory:manage') ? <div className="flex gap-2"><Button onClick={() => setOpen('new')}><Plus className="size-4" />{t('memory.create')}</Button></div> : undefined} columns={columns} data={resources.data ?? []} description={t('memory.description')} getSearchText={(row) => `${row.name} ${row.endpoint}`} getStatus={(row) => row.status === 'active' ? 'active' : 'inactive'} searchPlaceholder={t('memory.search')} statusOptions={[{ value: 'active', label: t('common.active') }, { value: 'inactive', label: t('common.inactive') }]} title={t('memory.title')} />{open && <EntityFormDialog cancelLabel={t('common.cancel')} fields={newFields} onClose={() => setOpen(undefined)} onSubmit={(values) => create.mutateAsync(values).then(() => undefined)} open submitLabel={t('common.save')} title={t('memory.create')} />}</>
}
