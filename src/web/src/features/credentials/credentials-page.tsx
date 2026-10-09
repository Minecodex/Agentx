import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import { KeyRound, Plus } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { Credential, Department, PageResponse } from '../../shared/api/types'
import { EntityCell } from '../../shared/components/entity-cell'
import { EntityDeleteButton } from '../../shared/components/entity-delete-button'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'
import { ListPage } from '../../shared/components/list-page'
import { StatusBadge } from '../../shared/components/status-badge'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { Button } from '../../shared/ui/button'
import { useToast } from '../../shared/ui/toast'

export function CredentialsPage() {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  const auth = useAuth()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [open, setOpen] = useState(false)
  const credentials = useQuery({ queryKey: ['credentials'], queryFn: () => apiRequest<PageResponse<Credential>>('/credentials?pageSize=100') })
  const departments = useQuery({ queryKey: ['departments'], queryFn: () => apiRequest<Department[]>('/departments') })
  const create = useMutation({
    mutationFn: (values: Record<string, string>) => apiRequest<Credential>('/credentials', { method: 'POST', body: jsonBody({ name: values.name, credentialType: values.type, secret: parseSecret(values.type, values.secret, t('credentials.invalidJson')), ownerDepartmentId: values.department }) }),
    onSuccess: async () => { await queryClient.invalidateQueries({ queryKey: ['credentials'] }); showToast(t('credentials.created')) },
  })
  const columns = useMemo<Array<ColumnDef<Credential>>>(() => [
    { accessorKey: 'name', header: t('common.name'), cell: ({ row }) => <EntityCell detail={`${row.original.credentialType} · ${row.original.maskedHint}`} icon={KeyRound} name={row.original.name} /> },
    { accessorKey: 'storageMode', header: t('credentials.storageMode') },
    { accessorKey: 'currentSecretVersion', header: t('common.version'), cell: ({ row }) => `v${row.original.currentSecretVersion}` },
    { accessorKey: 'status', header: t('common.status'), cell: ({ row }) => <StatusBadge status={row.original.status === 'active' ? 'active' : 'inactive'} /> },
    { accessorKey: 'updatedAt', header: t('common.updatedAt'), cell: ({ row }) => formatDateTime(row.original.updatedAt) },
    { id: 'actions', header: '', cell: ({ row }) => <div className="flex gap-1"><Button asChild size="sm" variant="ghost"><Link to={`/credentials/${row.original.id}`}>{t('credentials.details')}</Link></Button><EntityDeleteButton canDelete={auth.hasPermission('credential:delete')} deletePath={`/credentials/${row.original.id}`} entityId={row.original.id} entityName={row.original.name} entityType="credential" onDeleted={async () => { await queryClient.invalidateQueries({ queryKey: ['credentials'] }); showToast(t('credentials.deleted')) }} /></div> },
  ], [auth, formatDateTime, queryClient, showToast, t])
  const fields: EntityFormField[] = [
    { name: 'name', label: t('common.name'), required: true },
    { name: 'type', label: t('credentials.credentialType'), type: 'select', defaultValue: 'api_key', required: true, options: [{ value: 'api_key', label: t('credentials.apiKey') }, { value: 'bearer', label: t('credentials.bearer') }, { value: 'basic', label: t('credentials.basic') }, { value: 'custom_json', label: t('credentials.customJson') }] },
    { name: 'secret', label: t('credentials.secret'), type: 'password', required: true, placeholder: t('credentials.passwordMask') },
    { name: 'department', label: t('credentials.department'), type: 'select', required: true, options: (departments.data ?? []).map((item) => ({ value: item.id, label: item.name })) },
  ]
  return <>
    <ListPage loading={credentials.isLoading} error={credentials.error} action={auth.hasPermission('credential:manage') ? <Button onClick={() => setOpen(true)}><Plus className="size-4" />{t('credentials.createCredential')}</Button> : undefined} columns={columns} data={credentials.data?.items ?? []} description={t('credentials.credentialDescription')} getSearchText={(row) => `${row.name} ${row.credentialType} ${row.maskedHint}`} getStatus={(row) => row.status === 'active' ? 'active' : 'inactive'} searchPlaceholder={t('credentials.searchCredentials')} statusOptions={[{ value: 'active', label: t('common.active') }, { value: 'inactive', label: t('common.inactive') }]} title={t('navigation.credentials')} />
    {open && <EntityFormDialog cancelLabel={t('common.cancel')} fields={fields} onClose={() => setOpen(false)} onSubmit={(values) => create.mutateAsync(values).then(() => undefined)} open submitLabel={t('common.save')} title={t('credentials.createCredential')} />}
  </>
}

function parseSecret(type: string, value: string, invalidMessage: string): unknown {
  if (type === 'basic' || type === 'custom_json') {
    try { return JSON.parse(value) as unknown } catch { throw new Error(invalidMessage) }
  }
  return value
}
