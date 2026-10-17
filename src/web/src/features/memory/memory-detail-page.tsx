import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Activity } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useParams } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { ExternalConnection, HealthCheck } from '../../shared/api/types'
import { ResourceDetailLayout } from '../../shared/components/resource-detail-layout'
import { localizedValue } from '../../shared/lib/localized-value'
import { Button } from '../../shared/ui/button'
import { EntityFormDialog } from '../../shared/components/entity-form-dialog'
import { useMemoryFields } from './use-memory-fields'
import { Card } from '../../shared/ui/card'
import { useToast } from '../../shared/ui/toast'

export function MemoryDetailPage() {
  const { id = '' } = useParams(); const { t } = useTranslation(); const auth = useAuth(); const { showToast } = useToast(); const [health, setHealth] = useState<HealthCheck>()
  const resource = useQuery({ queryKey: ['memory', id], queryFn: () => apiRequest<ExternalConnection>(`/memory/connections/${id}`) })
  const queryClient = useQueryClient()
  const [editing, setEditing] = useState(false)
  const fields = useMemoryFields(resource.data)
  const save = useMutation({ mutationFn: (values: Record<string, string>) => apiRequest(`/memory/connections/${id}`, { method: 'PATCH', body: jsonBody({ name: values.name, endpoint: values.endpoint, healthPath: values.healthPath, credentialId: values.credential || null, ownerDepartmentId: values.department, status: values.status, version: resource.data?.version }) }), onSuccess: async () => { await Promise.all([queryClient.invalidateQueries({ queryKey: ['memory'] }), queryClient.invalidateQueries({ queryKey: ['memory', id] })]); showToast(t('applications.saved')) } })
  const test = useMutation({ mutationFn: () => apiRequest<HealthCheck>(`/memory/connections/${id}/test-connection`, { method: 'POST' }), onSuccess: (value) => { setHealth(value); showToast(localizedValue(t, 'memory', value.status)) }, onError: (error: Error) => showToast(error.message) })
  const value = resource.data
  return <><ResourceDetailLayout actions={auth.hasPermission('memory:manage') ? <div className="flex gap-2"><Button onClick={() => setEditing(true)} variant="secondary">{t('common.edit')}</Button><Button disabled={test.isPending} onClick={() => test.mutate()} variant="secondary"><Activity className="size-4" />{t('memory.connectionTest')}</Button></div> : undefined} description={t('memory.description')} details={value ? [{ label: t('memory.connectionName'), value: value.name }, { label: t('memory.endpoint'), value: value.endpoint }, { label: t('memory.healthPath'), value: value.healthPath }, { label: t('memory.department'), value: value.ownerDepartmentId }] : []} error={resource.error} loading={resource.isLoading} name={value?.name} status={value?.status}>{health && <Card className="p-4 text-xs text-muted-foreground">{localizedValue(t, 'memory', health.status)} · {health.latencyMs ?? 0} ms{health.errorMessage ? ` · ${health.errorMessage}` : ''}</Card>}</ResourceDetailLayout>{editing && <EntityFormDialog open fields={fields} title={t('common.edit')} cancelLabel={t('common.cancel')} submitLabel={t('common.save')} onClose={() => setEditing(false)} onSubmit={values => save.mutateAsync(values).then(() => undefined)} />}</>
}
