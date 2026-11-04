import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Activity, History, Pencil, Plus } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useParams } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { Credential, Department, HealthCheck, Model, ModelDeploymentHistory, ModelPrice, PageResponse } from '../../shared/api/types'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'
import { ResourceDetailLayout } from '../../shared/components/resource-detail-layout'
import { StatusBadge } from '../../shared/components/status-badge'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { localizedValue } from '../../shared/lib/localized-value'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { useToast } from '../../shared/ui/toast'
import { ModelParametersField } from './model-parameters-field'
import { reasoningLabel } from './model-reasoning'

export function ModelDetailPage() {
  const { id = '' } = useParams()
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  const auth = useAuth()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [priceOpen, setPriceOpen] = useState(false)
  const [editOpen, setEditOpen] = useState(false)
  const [health, setHealth] = useState<HealthCheck>()
  const model = useQuery({ queryKey: ['model', id], queryFn: () => apiRequest<Model>(`/models/aliases/${id}`) })
  const prices = useQuery({ queryKey: ['model-prices', model.data?.deploymentId], enabled: Boolean(model.data), queryFn: () => apiRequest<ModelPrice[]>(`/models/deployments/${model.data?.deploymentId}/prices`) })
  const history = useQuery({ queryKey: ['model-history', id], queryFn: () => apiRequest<ModelDeploymentHistory[]>(`/models/aliases/${id}/deployment-history`) })
  const departments = useQuery({ queryKey: ['departments'], queryFn: () => apiRequest<Department[]>('/departments') })
  const credentials = useQuery({ queryKey: ['credentials', 'model-options'], queryFn: () => apiRequest<PageResponse<Credential>>('/credentials?pageSize=100&status=active') })
  const test = useMutation({
    mutationFn: () => apiRequest<HealthCheck>(`/models/aliases/${id}/test-connection`, { method: 'POST' }),
    onSuccess: async (result) => { setHealth(result); await invalidate(); showToast(localizedValue(t, 'models', result.status)) },
    onError: (error: Error) => showToast(error.message),
  })

  const invalidate = async () => Promise.all([
    queryClient.invalidateQueries({ queryKey: ['model', id] }),
    queryClient.invalidateQueries({ queryKey: ['models'] }),
    queryClient.invalidateQueries({ queryKey: ['model-history', id] }),
    queryClient.invalidateQueries({ queryKey: ['model-prices'] }),
  ])
  const createPrice = async (values: Record<string, string>) => {
    await apiRequest(`/models/deployments/${model.data?.deploymentId}/prices`, { method: 'POST', body: jsonBody({ currency: values.currency, inputPerMillion: values.input, outputPerMillion: values.output }) })
    await invalidate()
  }
  const editModel = async (values: Record<string, string>) => {
    if (!model.data) return
    let parameters: unknown
    try { parameters = JSON.parse(values.parameters) as unknown } catch { throw new Error(t('models.invalidJson')) }
    const price = { currency: values.currency.trim(), inputPerMillion: values.input.trim(), outputPerMillion: values.output.trim() }
    await apiRequest<Model>(`/models/aliases/${id}`, {
      method: 'PATCH',
      body: jsonBody({
        connectionName: values.connectionName,
        providerType: values.providerType,
        endpoint: values.endpoint,
        credentialId: values.credential || null,
        alias: values.alias,
        status: values.status,
        modelName: values.modelName,
        maxInputTokens: Number(values.maxInputTokens),
        maxOutputTokens: Number(values.maxOutputTokens),
        ownerDepartmentId: values.department,
        defaultParameters: parameters,
        expectedAliasVersion: model.data.aliasVersion,
        price,
      }),
    })
    setHealth(undefined)
    await invalidate()
    showToast(t('models.saved'))
  }

  const value = model.data
  const currentPrice = prices.data?.[0]
  const priceFields: EntityFormField[] = [
    { name: 'currency', label: t('models.currency'), defaultValue: 'USD', required: true },
    { name: 'input', label: t('models.inputPrice'), type: 'number', min: 0, required: true, step: 'any' },
    { name: 'output', label: t('models.outputPrice'), type: 'number', min: 0, required: true, step: 'any' },
  ]
  const editFields: EntityFormField[] = value ? [
    { name: 'connectionName', label: t('models.connectionName'), defaultValue: value.connectionName, required: true },
    { name: 'providerType', label: t('models.providerType'), type: 'select', defaultValue: value.providerType, required: true, options: [{ value: 'openai_compatible', label: t('models.openaiCompatible') }] },
    { name: 'endpoint', label: t('models.endpoint'), defaultValue: value.endpoint, required: true },
    { name: 'credential', label: t('models.credential'), type: 'select', defaultValue: value.credentialId ?? '', options: [{ value: '', label: t('models.noCredential') }, ...(credentials.data?.items ?? []).map((item) => ({ value: item.id, label: item.name }))] },
    { name: 'alias', label: t('models.fields.modelName'), defaultValue: value.alias, required: true },
    { name: 'modelName', label: t('models.fields.upstreamModelId'), defaultValue: value.modelName, required: true },
    { name: 'maxInputTokens', label: t('models.maxInputTokens'), type: 'number', defaultValue: String(value.maxInputTokens), min: 1, required: true, step: 1 },
    { name: 'maxOutputTokens', label: t('models.maxOutputTokens'), type: 'number', defaultValue: String(value.maxOutputTokens), min: 1, required: true, step: 1 },
    { name: 'status', label: t('models.fields.status'), type: 'select', defaultValue: value.status, required: true, options: statusOptions(t) },
    { name: 'currency', label: t('models.currency'), defaultValue: currentPrice?.currency ?? 'USD', required: true },
    { name: 'input', label: t('models.inputPrice'), type: 'number', defaultValue: currentPrice?.inputPerMillion ?? '', min: 0, required: true, step: 'any' },
    { name: 'output', label: t('models.outputPrice'), type: 'number', defaultValue: currentPrice?.outputPerMillion ?? '', min: 0, required: true, step: 'any' },
    { name: 'department', apiName: 'ownerDepartmentId', label: t('models.department'), type: 'select', defaultValue: value.ownerDepartmentId, required: true, options: (departments.data ?? []).map((item) => ({ value: item.id, label: item.name })) },
    { name: 'parameters', label: t('models.generationParameters'), defaultValue: JSON.stringify(value.defaultParameters, null, 2), render: ({ value, update, error }) => <ModelParametersField error={error} onChange={update} value={value} /> },
  ] : []
  const actions = auth.hasPermission('model:manage') ? <div className="flex gap-2">
    <Button disabled={!value || prices.isLoading} onClick={() => setEditOpen(true)} variant="secondary"><Pencil className="size-4" />{t('models.editModel')}</Button>
    <Button disabled={test.isPending} onClick={() => test.mutate()} variant="secondary"><Activity className="size-4" />{t('models.connectionTest')}</Button>
  </div> : undefined
  return <>
    <ResourceDetailLayout actions={actions} description={t('models.description')} details={value ? [
      { label: t('models.connectionName'), value: value.connectionName }, { label: t('models.providerType'), value: t('models.openaiCompatible') },
      { label: t('models.endpoint'), value: value.endpoint }, { label: t('models.fields.modelName'), value: value.alias },
      { label: t('models.fields.upstreamModelId'), value: value.modelName }, { label: t('models.maxInputTokens'), value: value.maxInputTokens },
      { label: t('models.maxOutputTokens'), value: value.maxOutputTokens }, { label: t('models.deploymentRevision'), value: `r${value.revisionNumber}` },
      { label: t('models.department'), value: value.ownerDepartmentId },
      { label: t('models.reasoningEffort'), value: reasoningLabel(value.defaultParameters, t) },
    ] : []} error={model.error} loading={model.isLoading} name={value?.alias} status={value?.status}>
      {value && <Card className="p-4 text-xs"><div className="flex flex-wrap items-center gap-3"><Activity className="size-4 text-primary" /><span className="font-medium">{t('models.connectionStatus')}</span><StatusBadge label={localizedValue(t, 'models', health?.status ?? value.connectionStatus)} status={connectionStatus(health?.status ?? value.connectionStatus)} />{health?.latencyMs != null && <span className="text-muted-foreground">{health.latencyMs} ms</span>}{(health?.checkedAt ?? value.connectionCheckedAt) && <span className="text-muted-foreground">{t('models.checkedAt', { time: formatDateTime(health?.checkedAt ?? value.connectionCheckedAt ?? '') })}</span>}</div>{health?.errorMessage && <p className="mt-3 break-words rounded-md border border-danger/25 bg-danger/5 px-3 py-2 text-danger" role="alert">{health.errorCode && <strong className="mr-2">{health.errorCode}</strong>}{health.errorMessage}</p>}</Card>}
      <Card className="p-5"><div className="flex items-center"><h2 className="text-sm font-semibold">{t('models.priceVersions')}</h2><div className="flex-1" />{auth.hasPermission('model:manage') && <Button onClick={() => setPriceOpen(true)} size="sm" variant="secondary"><Plus className="size-3.5" />{t('models.addPrice')}</Button>}</div><div className="mt-3 divide-y divide-border">{prices.data?.map((price) => <div className="grid grid-cols-3 gap-3 py-3 text-xs" key={price.id}><span>v{price.versionNumber} · {price.currency}</span><span>{t('models.inputPrice')}: {price.inputPerMillion}</span><span>{t('models.outputPrice')}: {price.outputPerMillion}</span></div>)}{prices.data?.length === 0 && <p className="py-3 text-xs text-muted-foreground">{t('models.noData')}</p>}</div></Card>
      <Card className="p-5"><div className="flex items-center gap-2"><History className="size-4 text-primary" /><h2 className="text-sm font-semibold">{t('models.deploymentHistory')}</h2></div><div className="mt-3 divide-y divide-border">{history.data?.map((item) => <div className="grid grid-cols-[90px_1fr_auto] gap-3 py-3 text-xs" key={item.id}><span>r{item.revisionNumber}</span><span>{item.connectionName} · {item.modelName}</span><span className="text-muted-foreground">{formatDateTime(item.changedAt)}</span></div>)}{history.data?.length === 0 && <p className="py-3 text-xs text-muted-foreground">{t('models.noData')}</p>}</div></Card>
    </ResourceDetailLayout>
    {priceOpen && <EntityFormDialog cancelLabel={t('common.cancel')} fields={priceFields} onClose={() => setPriceOpen(false)} onSubmit={createPrice} open submitLabel={t('common.save')} title={t('models.addPrice')} />}
    {editOpen && <EntityFormDialog cancelLabel={t('common.cancel')} fields={editFields} onClose={() => setEditOpen(false)} onSubmit={editModel} open submitLabel={t('common.save')} title={t('models.editModel')} />}
  </>
}

function statusOptions(t: (key: string) => string) {
  return [{ value: 'active', label: t('common.active') }, { value: 'disabled', label: t('common.inactive') }]
}

function connectionStatus(value: string): 'untested' | 'healthy' | 'unhealthy' {
  return value === 'healthy' || value === 'unhealthy' ? value : 'untested'
}
