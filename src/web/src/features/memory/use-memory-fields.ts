import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'

import { apiRequest } from '../../shared/api/client'
import type { Credential, Department, ExternalConnection, PageResponse } from '../../shared/api/types'
import type { EntityFormField } from '../../shared/components/entity-form-dialog'

export function useMemoryFields(connection?: ExternalConnection): EntityFormField[] {
  const { t } = useTranslation()
  const departments = useQuery({ queryKey: ['departments'], queryFn: () => apiRequest<Department[]>('/departments') })
  const credentials = useQuery({ queryKey: ['credentials', 'options'], queryFn: () => apiRequest<PageResponse<Credential>>('/credentials?pageSize=100&status=active') })
  return [
    { name: 'name', label: t('memory.connectionName'), required: true, defaultValue: connection?.name },
    { name: 'endpoint', label: t('memory.endpoint'), required: true, defaultValue: connection?.endpoint },
    { name: 'healthPath', label: t('memory.healthPath'), required: true, defaultValue: connection?.healthPath ?? '/health' },
    { name: 'credential', label: t('memory.credential'), type: 'select', defaultValue: connection?.credentialId ?? '', options: [{ value: '', label: t('memory.noCredential') }, ...(credentials.data?.items ?? []).map(item => ({ value: item.id, label: item.name }))] },
    { name: 'department', label: t('memory.department'), type: 'select', required: true, defaultValue: connection?.ownerDepartmentId, options: (departments.data ?? []).map(item => ({ value: item.id, label: item.name })) },
    ...(connection ? [{ name: 'status', label: t('common.status'), type: 'select' as const, required: true, defaultValue: connection.status, options: [{ value: 'active', label: t('common.active') }, { value: 'disabled', label: t('common.inactive') }] }] : []),
  ]
}
