import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import { BrainCircuit, Plus } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { Credential, Department, Model, PageResponse } from '../../shared/api/types'
import { EntityCell } from '../../shared/components/entity-cell'
import { EntityDeleteButton } from '../../shared/components/entity-delete-button'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'
import { ListPage } from '../../shared/components/list-page'
import { StatusBadge } from '../../shared/components/status-badge'
import { localizedValue } from '../../shared/lib/localized-value'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { Button } from '../../shared/ui/button'
import { useToast } from '../../shared/ui/toast'

export function ModelsPage() {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  const auth = useAuth()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [open, setOpen] = useState(false)
  const models = useQuery({ queryKey: ['models'], queryFn: () => apiRequest<PageResponse<Model>>('/models/aliases?pageSize=100') })
  const departments = useQuery({ queryKey: ['departments'], queryFn: () => apiRequest<Department[]>('/departments') })
  const credentials = useQuery({ queryKey: ['credentials', 'options'], queryFn: () => apiRequest<PageResponse<Credential>>('/credentials?pageSize=100&status=active') })
  const createModel = useMutation({
    mutationFn: async (values: Record<string, string>) => {
      let defaults: unknown = {}
      try { defaults = values.parameters ? JSON.parse(values.parameters) as unknown : {} } catch { throw new Error(t('models.invalidJson')) }
      const inputPrice = values.input.trim()
      const outputPrice = values.output.trim()
      return apiRequest<Model>('/models/aliases', {
        method: 'POST',
        body: jsonBody({
          connectionName: values.connectionName,
          providerType: values.providerType,
          endpoint: values.endpoint,
          credentialId: values.credential || null,
          alias: values.alias,
          modelName: values.modelName,
          maxInputTokens: Number(values.maxInputTokens),
          maxOutputTokens: Number(values.maxOutputTokens),
          ownerDepartmentId: values.department,
          defaultParameters: defaults,
          capabilities: values.capabilities.split(',').map((item: string) => item.trim()).filter(Boolean),
          price: { currency: values.currency.trim(), inputPerMillion: inputPrice, outputPerMillion: outputPrice },
        }),
      })
    },
    onSuccess: async () => { await queryClient.invalidateQueries({ queryKey: ['models'] }); showToast(t('models.created')) },
  })
  const columns = useMemo<Array<ColumnDef<Model>>>(() => [
    { accessorKey: 'alias', header: t('models.fields.modelName'), cell: ({ row }) => <EntityCell detail={`${row.original.connectionName} · ${row.original.modelName}`} icon={BrainCircuit} name={row.original.alias} /> },
    { accessorKey: 'providerType', header: t('models.provider'), cell: () => t('models.openaiCompatible') },
    { accessorKey: 'modelName', header: t('models.model') },
    { accessorKey: 'connectionStatus', header: t('models.connectionStatus'), cell: ({ row }) => <StatusBadge label={localizedValue(t, 'models', row.original.connectionStatus)} status={connectionStatus(row.original.connectionStatus)} /> },
    { accessorKey: 'status', header: t('common.status'), cell: ({ row }) => <StatusBadge label={localizedValue(t, 'common', row.original.status)} status={row.original.status === 'active' ? 'active' : 'inactive'} /> },
    { accessorKey: 'updatedAt', header: t('common.updatedAt'), cell: ({ row }) => formatDateTime(row.original.updatedAt) },
    { id: 'actions', header: '', cell: ({ row }) => <div className="flex gap-1"><Button asChild size="sm" variant="ghost"><Link to={`/models/${row.original.id}`}>{t('models.details')}</Link></Button><EntityDeleteButton canDelete={auth.hasPermission('model:delete')} deletePath={`/models/aliases/${row.original.id}`} entityId={row.original.id} entityName={row.original.alias} entityType="model" onDeleted={async () => { await queryClient.invalidateQueries({ queryKey: ['models'] }); showToast(t('models.deleted')) }} /></div> },
  ], [auth, formatDateTime, queryClient, showToast, t])
  const fields: EntityFormField[] = [
    { name: 'connectionName', label: t('models.connectionName'), required: true },
    { name: 'providerType', label: t('models.providerType'), type: 'select', defaultValue: 'openai_compatible', required: true, options: [{ value: 'openai_compatible', label: t('models.openaiCompatible') }] },
    { name: 'endpoint', label: t('models.endpoint'), required: true, placeholder: 'https://api.example.com/v1' },
    { name: 'credential', label: t('models.credential'), type: 'select', options: [{ value: '', label: t('models.noCredential') }, ...(credentials.data?.items ?? []).map((item) => ({ value: item.id, label: item.name }))] },
    { name: 'alias', label: t('models.fields.modelName'), defaultValue: 'gpt-5.6-sol', required: true },
    { name: 'modelName', label: t('models.fields.upstreamModelId'), defaultValue: 'gpt-5.6-sol', required: true },
    { name: 'maxInputTokens', label: t('models.maxInputTokens'), type: 'number', defaultValue: '1050000', min: 1, required: true, step: 1 },
    { name: 'maxOutputTokens', label: t('models.maxOutputTokens'), type: 'number', defaultValue: '128000', min: 1, required: true, step: 1 },
    { name: 'currency', label: t('models.currency'), defaultValue: 'USD', required: true },
    { name: 'input', label: t('models.inputPrice'), type: 'number', min: 0, required: true, step: 'any' },
    { name: 'output', label: t('models.outputPrice'), type: 'number', min: 0, required: true, step: 'any' },
    { name: 'department', apiName: 'ownerDepartmentId', label: t('models.department'), type: 'select', required: true, options: (departments.data ?? []).map((item) => ({ value: item.id, label: item.name })) },
    { name: 'parameters', label: t('models.defaultParameters'), type: 'textarea', defaultValue: '{}' },
    { name: 'capabilities', label: t('models.capabilities'), placeholder: 'vision, audio' },
  ]
  return <>
    <ListPage loading={models.isLoading} error={models.error} action={auth.hasPermission('model:manage') ? <Button onClick={() => setOpen(true)}><Plus className="size-4" />{t('models.create')}</Button> : undefined} columns={columns} data={models.data?.items ?? []} description={t('models.description')} getSearchText={(row) => `${row.alias} ${row.connectionName} ${row.modelName}`} getStatus={(row) => row.status === 'active' ? 'active' : 'inactive'} searchPlaceholder={t('models.search')} statusOptions={[{ value: 'active', label: t('common.active') }, { value: 'inactive', label: t('common.inactive') }]} title={t('models.title')} />
    {open && <EntityFormDialog cancelLabel={t('common.cancel')} fields={fields} onClose={() => setOpen(false)} onSubmit={async (values) => { await createModel.mutateAsync(values) }} open submitLabel={t('common.save')} title={t('models.create')} />}
  </>
}

function connectionStatus(value: string): 'untested' | 'healthy' | 'unhealthy' {
  return value === 'healthy' || value === 'unhealthy' ? value : 'untested'
}
