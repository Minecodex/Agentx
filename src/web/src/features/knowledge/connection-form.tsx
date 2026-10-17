import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { Credential, Department, ExternalConnection, PageResponse } from '../../shared/api/types'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'

export function ConnectionForm({ connection, onClose, onSaved }: { connection?: ExternalConnection; onClose: () => void; onSaved: () => Promise<void> }) {
  const { t } = useTranslation()
  const departments = useQuery({ queryKey: ['departments'], queryFn: () => apiRequest<Department[]>('/departments') })
  const credentials = useQuery({ queryKey: ['credentials', 'options'], queryFn: () => apiRequest<PageResponse<Credential>>('/credentials?pageSize=100&status=active') })
  const fields: EntityFormField[] = [
    { name: 'name', label: t('knowledge.connectionName'), defaultValue: connection?.name, required: true },
    { name: 'provider', label: t('knowledge.provider'), type: 'select', defaultValue: connection?.provider ?? 'lightrag', required: true, options: [{ value: 'lightrag', label: 'LightRAG' }, { value: 'ragflow', label: 'RAGFlow' }] },
    { name: 'endpoint', label: t('knowledge.endpoint'), defaultValue: connection?.endpoint, required: true },
    { name: 'credentialId', label: t('knowledge.credential'), type: 'select', defaultValue: connection?.credentialId ?? 'none', options: [{ value: 'none', label: t('knowledge.noCredential') }, ...(credentials.data?.items ?? []).map(item => ({ value: item.id, label: item.name }))] },
    { name: 'ownerDepartmentId', label: t('knowledge.department'), type: 'select', defaultValue: connection?.ownerDepartmentId, required: true, options: (departments.data ?? []).map(item => ({ value: item.id, label: item.name })) },
    ...(connection ? [{ name: 'status', label: t('common.status'), type: 'select' as const, defaultValue: connection.status, options: [{ value: 'active', label: t('common.active') }, { value: 'disabled', label: t('common.inactive') }] }] : []),
  ]
  return <EntityFormDialog open title={t(connection ? 'knowledge.editConnection' : 'knowledge.create')} fields={fields} cancelLabel={t('common.cancel')} submitLabel={t('common.save')} onClose={onClose} onSubmit={async values => {
    const body = { ...values, credentialId: values.credentialId === 'none' ? null : values.credentialId, ...(connection ? { version: connection.version } : {}) }
    await apiRequest(`/knowledge/connections${connection ? `/${connection.id}` : ''}`, { method: connection ? 'PATCH' : 'POST', body: jsonBody(body) })
    await onSaved()
  }} />
}
