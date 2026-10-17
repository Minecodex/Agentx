import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import { Library, Plus } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest } from '../../shared/api/client'
import type { ExternalConnection, HealthCheck } from '../../shared/api/types'
import { ConfirmDialog } from '../../shared/components/confirm-dialog'
import { EntityCell } from '../../shared/components/entity-cell'
import { ListPage } from '../../shared/components/list-page'
import { StatusBadge } from '../../shared/components/status-badge'
import { Button } from '../../shared/ui/button'
import { useToast } from '../../shared/ui/toast'
import { ConnectionForm } from './connection-form'

export function KnowledgePage() {
  const { t } = useTranslation(); const auth = useAuth(); const client = useQueryClient(); const { showToast } = useToast()
  const [editing, setEditing] = useState<ExternalConnection | 'new'>()
  const [removing, setRemoving] = useState<ExternalConnection>()
  const connections = useQuery({ queryKey: ['knowledge-connections'], queryFn: () => apiRequest<ExternalConnection[]>('/knowledge/connections') })
  const refresh = async () => { await client.invalidateQueries({ queryKey: ['knowledge-connections'] }) }
  const test = useMutation({ mutationFn: (id: string) => apiRequest<HealthCheck>(`/knowledge/connections/${id}/test-connection`, { method: 'POST' }), onSuccess: value => showToast(`${t(value.status === 'healthy' ? 'knowledge.healthy' : 'knowledge.unhealthy')} · ${value.latencyMs ?? 0} ms`), onError: (e: Error) => showToast(e.message) })
  const remove = useMutation({ mutationFn: () => apiRequest(`/knowledge/connections/${removing!.id}`, { method: 'DELETE' }), onSuccess: async () => { setRemoving(undefined); await refresh(); showToast(t('knowledge.deleted')) }, onError: (e: Error) => showToast(e.message) })
  const columns: ColumnDef<ExternalConnection>[] = [
    { accessorKey: 'name', header: t('knowledge.connectionName'), cell: ({ row }) => <EntityCell icon={Library} name={row.original.name} detail={row.original.provider ?? ''} /> },
    { accessorKey: 'endpoint', header: t('knowledge.endpoint') },
    { accessorKey: 'status', header: t('common.status'), cell: ({ row }) => <StatusBadge status={row.original.status === 'active' ? 'active' : 'inactive'} /> },
    { id: 'actions', header: '', cell: ({ row }) => <div className="flex justify-end gap-1">
      <Button asChild size="sm" variant="ghost"><Link to={`/knowledge/${row.original.id}`}>{t('knowledge.details')}</Link></Button>
      {auth.hasPermission('knowledge:manage') && <><Button size="sm" variant="ghost" onClick={() => setEditing(row.original)}>{t('common.edit')}</Button><Button size="sm" variant="ghost" disabled={test.isPending || row.original.status !== 'active'} onClick={() => test.mutate(row.original.id)}>{t('knowledge.connectionTest')}</Button></>}
      {auth.hasPermission('knowledge:delete') && <Button size="sm" variant="ghost" className="text-danger" onClick={() => setRemoving(row.original)}>{t('common.delete')}</Button>}
    </div> },
  ]
  return <>
    <ListPage title={t('knowledge.title')} description={t('knowledge.description')} loading={connections.isLoading} error={connections.error} columns={columns} data={connections.data ?? []} getSearchText={row => `${row.name} ${row.provider} ${row.endpoint}`} searchPlaceholder={t('knowledge.search')} getStatus={row => row.status === 'active' ? 'active' : 'inactive'} statusOptions={[{ value: 'active', label: t('common.active') }, { value: 'inactive', label: t('common.inactive') }]} action={auth.hasPermission('knowledge:manage') ? <Button onClick={() => setEditing('new')}><Plus className="size-4" />{t('knowledge.create')}</Button> : undefined} />
    {editing && <ConnectionForm connection={editing === 'new' ? undefined : editing} onClose={() => setEditing(undefined)} onSaved={refresh} />}
    <ConfirmDialog open={Boolean(removing)} title={t('common.delete')} description={t('knowledge.deleteConnectionDescription', { name: removing?.name })} cancelLabel={t('common.cancel')} confirmLabel={t('common.delete')} pending={remove.isPending} onClose={() => setRemoving(undefined)} onConfirm={() => remove.mutateAsync().then(() => undefined)} />
  </>
}
