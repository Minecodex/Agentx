import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { BookOpen, Copy, Edit3, ExternalLink, KeyRound, Plus, RotateCw, ShieldOff, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useParams } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { ApplicationIntegrationDocs, type ApplicationIntegrationDoc, useApplicationIntegrationDocsText } from '../../docs/applications'
import { apiRequest, jsonBody, runtimePublicBaseUrl } from '../../shared/api/client'
import type { Application, ApplicationApiKey, ApplicationDeployment, ApplicationSchedule, ApplicationSession, ApplicationWebhook, DeliveryDetail, DeliveryPage, PublishAttempt, WebhookProviderTemplate, WorkflowEnvironment, WorkflowVersion } from '../../shared/api/types'
import { ConfirmDialog } from '../../shared/components/confirm-dialog'
import { EmptyState } from '../../shared/components/empty-state'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'
import { EntityDeleteButton } from '../../shared/components/entity-delete-button'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { PrerequisiteAction } from '../../shared/components/prerequisite-action'
import { StatusBadge } from '../../shared/components/status-badge'
import { localizedValue } from '../../shared/lib/localized-value'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { Input } from '../../shared/ui/input'
import { Select } from '../../shared/ui/select'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../shared/ui/tabs'
import { useToast } from '../../shared/ui/toast'
import { ProviderLogo, buildChannelConfig, channelConfigFields, effectiveChannelMode, useWebhookProviderTemplates } from './webhook-channel-fields'

type DialogKind = 'edit' | 'deployment' | 'key' | 'webhook' | 'webhookEdit' | 'schedule' | 'scheduleEdit' | 'sessionUpgrade' | null
type KeyAction = { keyId: string; action: 'rotate' | 'revoke' } | null
type DeploymentAction = { deploymentId: string; attemptId?: string; action: 'retry' | 'rollback' } | null
const pendingDeploymentStates = new Set(['building', 'copying', 'preparing', 'prepared', 'activating'])

export function ApplicationDetailPage() {
  const { id = '' } = useParams()
  const { t } = useTranslation()
  const integrationDocsText = useApplicationIntegrationDocsText()
  const { formatDateTime } = useLocaleFormat()
  const auth = useAuth()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [dialog, setDialog] = useState<DialogKind>(null)
  const [selectedSchedule, setSelectedSchedule] = useState<ApplicationSchedule | null>(null)
  const [selectedWebhook, setSelectedWebhook] = useState<ApplicationWebhook | null>(null)
  const [selectedChannelId, setSelectedChannelId] = useState<string | null>(null)
  const [selectedSession, setSelectedSession] = useState<ApplicationSession | null>(null)
  const [keyConfirmation, setKeyConfirmation] = useState<KeyAction>(null)
  const [deploymentConfirmation, setDeploymentConfirmation] = useState<DeploymentAction>(null)
  const [shownSecret, setShownSecret] = useState<string | null>(null)
  const [integrationDoc, setIntegrationDoc] = useState<ApplicationIntegrationDoc | null>(null)

  const application = useQuery({ queryKey: ['application', id], queryFn: () => apiRequest<Application>(`/applications/${id}`) })
  const deployments = useQuery({
    queryKey: ['application-deployments', id],
    queryFn: () => apiRequest<ApplicationDeployment[]>(`/applications/${id}/deployments`),
    refetchInterval: (query) => (query.state.data as ApplicationDeployment[] | undefined)?.some((item) => pendingDeploymentStates.has(item.status)) ? 2000 : false,
  })
  const keys = useQuery({ queryKey: ['application-keys', id], enabled: auth.hasPermission('application:manage_key'), queryFn: () => apiRequest<ApplicationApiKey[]>(`/applications/${id}/api-keys`) })
  const webhooks = useQuery({
    queryKey: ['application-webhooks', id],
    queryFn: () => apiRequest<ApplicationWebhook[]>(`/applications/${id}/webhooks`),
    refetchInterval: (query) => (query.state.data as ApplicationWebhook[] | undefined)?.some((item) => item.channelMode === 'stream' && item.connectionStatus !== 'connected' && item.connectionStatus !== 'disconnected') ? 5000 : false,
  })
  const schedules = useQuery({ queryKey: ['application-schedules', id], queryFn: () => apiRequest<ApplicationSchedule[]>(`/applications/${id}/schedules`) })
  const templates = useWebhookProviderTemplates()
  const sessions = useQuery({ queryKey: ['application-sessions', id], queryFn: () => apiRequest<ApplicationSession[]>(`/applications/${id}/sessions`) })
  const versions = useQuery({ queryKey: ['workflow-versions', application.data?.workflowId], enabled: Boolean(application.data), queryFn: () => apiRequest<WorkflowVersion[]>(`/workflows/${application.data?.workflowId}/versions`) })
  const [deliveryStatus, setDeliveryStatus] = useState('')
  const deliveries = useQuery({
    queryKey: ['application-deliveries', id, deliveryStatus],
    queryFn: () => apiRequest<DeliveryPage>(`/deliveries?applicationId=${id}&limit=50${deliveryStatus ? `&status=${deliveryStatus}` : ''}`),
    refetchInterval: (query) => (query.state.data as DeliveryPage | undefined)?.items.some((item) => item.status === 'pending' || item.status === 'delivering') ? 5000 : false,
  })
  const deliveryRetry = useMutation({
    mutationFn: (deliveryId: string) => apiRequest<DeliveryDetail>(`/deliveries/${deliveryId}/retry`, { method: 'POST' }),
    onSuccess: async () => { await queryClient.invalidateQueries({ queryKey: ['application-deliveries', id] }); showToast(t('applications.saved')) },
    onError: (error: Error) => showToast(error.message),
  })
  const environments = useQuery({ queryKey: ['environments'], queryFn: () => apiRequest<WorkflowEnvironment[]>('/environments') })
  const refresh = async () => queryClient.invalidateQueries({ predicate: (query) => String(query.queryKey[0]).startsWith('application') })

  const save = useMutation<unknown, Error, { kind: Exclude<DialogKind, null>; values: Record<string, string> }>({
    mutationFn: ({ kind, values }) => {
      if (kind === 'edit') return apiRequest<Application>(`/applications/${id}`, { method: 'PATCH', body: jsonBody({ name: values.name, description: values.description || null, visibility: values.visibility, status: values.status, version: application.data?.version }) })
      if (kind === 'deployment') return apiRequest<ApplicationDeployment>(`/applications/${id}/deployments`, { method: 'POST', body: jsonBody({ workflowVersionId: values.workflowVersionId, environmentId: values.environmentId, sessionVersionPolicy: values.sessionVersionPolicy }) })
      if (kind === 'key') return apiRequest<ApplicationApiKey>(`/applications/${id}/api-keys`, { method: 'POST', body: jsonBody({ name: values.name }) })
      if (kind === 'webhook') return apiRequest<ApplicationWebhook>(`/applications/${id}/webhooks`, { method: 'POST', body: jsonBody({ name: values.name, providerType: values.providerType, channelMode: effectiveChannelMode(values), channelConfig: buildChannelConfig(templates.data ?? [], values), reply: buildReplyConfig(values), inputMappings: JSON.parse(values.inputMappings || '[]'), fixedInputs: parseFixedInputs(values.fixedInputs, activeDeployment?.inputSchema) }) })
      if (kind === 'webhookEdit' && selectedWebhook) return apiRequest<ApplicationWebhook>(`/applications/${id}/webhooks/${selectedWebhook.id}`, { method: 'PATCH', body: jsonBody({ name: values.name, status: values.status, providerType: values.providerType, channelMode: effectiveChannelMode(values), channelConfig: buildChannelConfig(templates.data ?? [], values), reply: buildReplyConfig(values), inputMappings: JSON.parse(values.inputMappings || '[]'), fixedInputs: parseFixedInputs(values.fixedInputs, activeDeployment?.inputSchema), version: selectedWebhook.version }) })
      if (kind === 'sessionUpgrade' && selectedSession) return apiRequest<ApplicationSession>(`/sessions/${selectedSession.id}/upgrade`, { method: 'POST', body: jsonBody({ workflowVersionId: values.workflowVersionId, version: selectedSession.version }) })
      const path = kind === 'scheduleEdit' && selectedSchedule ? `/applications/${id}/schedules/${selectedSchedule.id}` : `/applications/${id}/schedules`
      return apiRequest<ApplicationSchedule>(path, { method: kind === 'scheduleEdit' ? 'PATCH' : 'POST', body: jsonBody({ name: values.name, cronExpression: values.cron, timezone: values.timezone, input: JSON.parse(values.input), misfirePolicy: values.misfirePolicy, ...(kind === 'scheduleEdit' ? { status: values.status, version: selectedSchedule?.version } : {}) }) })
    },
    onSuccess: async (result, variables) => {
      if (result && typeof result === 'object' && 'secret' in result && typeof result.secret === 'string' && variables.kind !== 'webhook') setShownSecret(result.secret)
      await refresh()
      showToast(t('applications.saved'))
    },
  })
  const keyAction = useMutation({
    mutationFn: ({ keyId, action }: Exclude<KeyAction, null>) => apiRequest<ApplicationApiKey | undefined>(`/applications/${id}/api-keys/${keyId}/${action}`, { method: 'POST' }),
    onSuccess: async (value) => { if (value?.secret) setShownSecret(value.secret); setKeyConfirmation(null); await refresh(); showToast(t('applications.saved')) },
  })
  const channelToggle = useMutation({
    mutationFn: ({ webhook, status }: { webhook: ApplicationWebhook; status: string }) => apiRequest<ApplicationWebhook>(`/applications/${id}/webhooks/${webhook.id}`, { method: 'PATCH', body: jsonBody({ name: webhook.name, status, providerType: webhook.providerType, channelMode: webhook.channelMode, channelConfig: {}, reply: webhook.reply ?? null, inputMappings: webhook.inputMappings ?? [], fixedInputs: webhook.fixedInputs ?? {}, version: webhook.version }) }),
    onSuccess: async () => { await refresh(); showToast(t('applications.saved')) },
    onError: (error: Error) => showToast(error.message),
  })
  const deploymentAction = useMutation({
    mutationFn: ({ deploymentId, attemptId, action }: Exclude<DeploymentAction, null>) => {
      const path = action === 'retry'
        ? `/applications/${id}/publish-attempts/${attemptId}:retry`
        : `/applications/${id}/deployments/${deploymentId}:rollback`
      return apiRequest<PublishAttempt>(path, { method: 'POST' })
    },
    onSuccess: async () => { setDeploymentConfirmation(null); await refresh(); showToast(t('applications.publishActionAccepted')) },
    onError: (error: Error) => showToast(error.message),
  })

  if (application.isLoading) return <PageContainer><p className="text-sm text-muted-foreground">{t('common.loading')}</p></PageContainer>
  if (!application.data) return <PageContainer><EmptyState title={t('common.loadFailed')} description={String(application.error ?? '')} /></PageContainer>
  const value = application.data
  const activeDeployment = deployments.data?.find((item) => item.id === value.activeDeploymentId && item.status === 'active')
    ?? deployments.data?.find((item) => item.status === 'active')
  const selectedChannel = webhooks.data?.find((item) => item.id === selectedChannelId) ?? webhooks.data?.[0]
  const channelEndpointActive = (item: ApplicationWebhook) => Boolean(value.status === 'active' && activeDeployment && value.runtimeConfigRevision <= value.publishedRuntimeConfigRevision && item.status === 'active')
  const endpointActive = Boolean(selectedChannel && channelEndpointActive(selectedChannel))
  const schedule = selectedSchedule
  const fields: Record<Exclude<DialogKind, null>, EntityFormField[]> = {
    edit: [
      { name: 'name', label: t('common.name'), required: true, defaultValue: value.name },
      { name: 'description', label: t('common.description'), type: 'textarea', defaultValue: value.description ?? '' },
      { name: 'visibility', label: t('applications.visibility'), type: 'select', defaultValue: value.visibility, required: true, options: ['private', 'department', 'company'].map((item) => ({ value: item, label: t(`applications.${item}`) })) },
      { name: 'status', label: t('common.status'), type: 'select', defaultValue: value.status, required: true, options: ['draft', 'active', 'disabled'].map((item) => ({ value: item, label: t(`applications.${item}`) })) },
    ],
    deployment: [
      { name: 'workflowVersionId', label: t('evaluations.workflowVersion'), type: 'select', required: true, options: (versions.data ?? []).map((item) => ({ value: item.id, label: `v${item.versionNumber}` })) },
      { name: 'environmentId', label: t('applications.environment'), type: 'select', required: true, options: (environments.data ?? []).filter((item) => item.status === 'active').map((item) => ({ value: item.id, label: item.name })) },
      { name: 'sessionVersionPolicy', label: t('applications.sessionPolicy'), type: 'select', defaultValue: 'pinned', required: true, options: ['pinned', 'follow_deployment', 'manual_upgrade'].map((item) => ({ value: item, label: t(`applications.${item}`) })) },
    ],
    key: [{ name: 'name', label: t('common.name'), required: true }],
    webhook: [
      { name: 'name', label: t('common.name'), required: true },
      ...channelConfigFields(t, templates.data ?? []),
      ...replyConfigFields(t, activeDeployment?.outputSchema, null),
      { name: 'inputMappings', label: t('applications.inputMappings'), required: true, defaultValue: JSON.stringify(defaultChannelMappings(activeDeployment?.inputSchema), null, 2), render: ({ value: mappings, values, update }) => <MappingEditor mode={values.channelMode} provider={values.providerType} schema={activeDeployment?.inputSchema} templates={templates.data ?? []} value={mappings} onChange={update} /> },
      { name: 'fixedInputs', label: t('applications.fixedInputs'), defaultValue: '{}', render: ({ value: fixed, update }) => <FixedInputsEditor schema={activeDeployment?.inputSchema} value={fixed} onChange={update} /> },
    ],
    webhookEdit: [
      { name: 'name', label: t('common.name'), required: true, defaultValue: selectedWebhook?.name ?? '' },
      ...channelConfigFields(t, templates.data ?? [], selectedWebhook),
      ...replyConfigFields(t, activeDeployment?.outputSchema, selectedWebhook?.reply ?? null),
      { name: 'inputMappings', label: t('applications.inputMappings'), required: true, defaultValue: JSON.stringify(selectedWebhook?.inputMappings ?? defaultChannelMappings(activeDeployment?.inputSchema), null, 2), render: ({ value: mappings, values, update }) => <MappingEditor mode={values.channelMode} provider={values.providerType} schema={activeDeployment?.inputSchema} templates={templates.data ?? []} value={mappings} onChange={update} /> },
      { name: 'fixedInputs', label: t('applications.fixedInputs'), defaultValue: JSON.stringify(selectedWebhook?.fixedInputs ?? {}, null, 2), render: ({ value: fixed, update }) => <FixedInputsEditor schema={activeDeployment?.inputSchema} value={fixed} onChange={update} /> },
      { name: 'status', label: t('common.status'), type: 'select', defaultValue: selectedWebhook?.status ?? 'active', required: true, options: ['active', 'disabled'].map((item) => ({ value: item, label: t(`applications.${item}`) })) },
    ],
    schedule: scheduleFields(t),
    scheduleEdit: scheduleFields(t, schedule),
    sessionUpgrade: [{ name: 'workflowVersionId', label: t('evaluations.workflowVersion'), type: 'select', defaultValue: selectedSession?.workflowVersionId ?? '', required: true, options: (versions.data ?? []).map((item) => ({ value: item.id, label: `v${item.versionNumber}` })) }],
  }
  return <PageContainer>
    <PageHeader action={auth.hasPermission('application:manage') ? <Button onClick={() => setDialog('edit')} variant="secondary"><Edit3 className="size-4" />{t('common.edit')}</Button> : undefined} description={`${value.workflowName} · ${value.slug}`} title={value.name} />
    <div className="mt-5"><StatusBadge status={value.status === 'active' ? 'active' : value.status === 'disabled' ? 'inactive' : 'draft'} /></div>
    <Tabs className="mt-6" defaultValue="deployments">
      <TabsList className="border-b border-border"><TabsTrigger value="deployments">{t('applications.deployments')}</TabsTrigger><TabsTrigger value="keys">{t('applications.apiKeys')}</TabsTrigger><TabsTrigger value="channels">{t('applications.channels')}</TabsTrigger><TabsTrigger value="triggers">{t('applications.triggers')}</TabsTrigger><TabsTrigger value="sessions">{t('applications.playground.sessions')}</TabsTrigger><TabsTrigger value="deliveries">{t('applications.deliveries')}</TabsTrigger></TabsList>
      <TabsContent className="pt-5" value="deployments"><Section action={auth.hasPermission('application:manage') && <PrerequisiteAction description={t('applications.prerequisites.deploymentDescription')} loading={versions.isLoading || environments.isLoading} onReady={() => setDialog('deployment')} requirements={[{ key: 'workflow-version', label: t('applications.prerequisites.workflowVersion'), met: Boolean(versions.data?.length), href: `/workflows/${value.workflowId}`, actionLabel: t('applications.prerequisites.goWorkflows') }, { key: 'environment', label: t('applications.prerequisites.activeEnvironment'), met: Boolean(environments.data?.some((item) => item.status === 'active')) }]} size="sm"><Plus className="size-4" />{t('applications.deploy')}</PrerequisiteAction>} title={t('applications.deployments')}>{deployments.data?.map((item) => <Row action={auth.hasPermission('application:manage') ? <div className="flex gap-1">{item.status === 'rejected' && item.publishAttemptId && <Button onClick={() => setDeploymentConfirmation({ deploymentId: item.id, attemptId: item.publishAttemptId ?? undefined, action: 'retry' })} size="sm" variant="ghost">{t('applications.retryPublish')}</Button>}{item.status === 'superseded' && <Button onClick={() => setDeploymentConfirmation({ deploymentId: item.id, action: 'rollback' })} size="sm" variant="ghost">{t('applications.rollback')}</Button>}</div> : undefined} detail={<>{localizedValue(t, 'applications', item.sessionVersionPolicy)} · #{item.sequenceNumber}{item.publishErrorMessage && <span className="mt-1 block text-danger">{item.publishErrorCode ? `${item.publishErrorCode}: ` : ''}{item.publishErrorMessage}</span>}</>} key={item.id} status={<StatusBadge label={deploymentStatusLabel(t, item.status)} status={deploymentStatus(item.status)} />} title={`${item.environmentName} · v${item.workflowVersionNumber}`} />)}</Section></TabsContent>
      <TabsContent className="pt-5" value="keys"><Section action={<div className="flex gap-2"><Button onClick={() => setIntegrationDoc({ kind: 'apiKey' })} size="sm" variant="secondary"><BookOpen className="size-4" />{integrationDocsText.openApiKey}</Button>{auth.hasPermission('application:manage_key') && <Button onClick={() => setDialog('key')} size="sm"><Plus className="size-4" />{t('applications.createKey')}</Button>}</div>} title={t('applications.apiKeys')}>
        {shownSecret && <div className="my-4 rounded-lg border border-warning/30 bg-warning/10 p-4"><p className="text-xs font-semibold">{t('applications.secretOnce')}</p><div className="mt-2 flex items-center gap-2"><code className="min-w-0 flex-1 break-all text-xs">{shownSecret}</code><Button aria-label={t('common.copy')} onClick={() => void navigator.clipboard.writeText(shownSecret)} size="icon" variant="ghost"><Copy className="size-4" /></Button><Button onClick={() => setIntegrationDoc({ kind: 'apiKey' })} size="sm" variant="secondary"><BookOpen className="size-3.5" />{integrationDocsText.openApiKey}</Button></div></div>}
        {keys.data?.map((item) => <div className="flex items-center border-t border-border py-3 first:border-0" key={item.id}><KeyRound className="mr-3 size-4 text-muted-foreground" /><div><p className="text-xs font-medium">{item.name}</p><p className="mt-1 text-[11px] text-muted-foreground">{item.prefix} · {localizedValue(t, 'applications', item.status)}</p></div><div className="flex-1" />{item.status === 'active' && <><Button aria-label={t('applications.rotateKey')} onClick={() => setKeyConfirmation({ keyId: item.id, action: 'rotate' })} size="icon" variant="ghost"><RotateCw className="size-4" /></Button><Button aria-label={t('applications.revoke')} onClick={() => setKeyConfirmation({ keyId: item.id, action: 'revoke' })} size="icon" variant="ghost"><ShieldOff className="size-4" /></Button></>}</div>)}
      </Section></TabsContent>
      <TabsContent className="grid grid-cols-2 gap-5 pt-5" value="channels">
        <Section action={<div className="flex gap-2">{selectedChannel && <Button onClick={() => setIntegrationDoc({ kind: 'webhook', name: selectedChannel.name, path: endpointActive ? selectedChannel.path : undefined })} size="sm" variant="secondary"><BookOpen className="size-4" />{integrationDocsText.openWebhook}</Button>}{auth.hasPermission('application:manage') && <Button onClick={() => setDialog('webhook')} size="sm"><Plus className="size-4" />{t('applications.addChannel')}</Button>}</div>} title={t('applications.channels')}>
          {webhooks.data?.map((item) => <div className={`flex cursor-pointer items-center gap-4 border-t border-border py-4 first:border-0 ${item.id === selectedChannel?.id ? '-mx-5 bg-primary/5 px-6' : 'pl-1'}`} key={item.id} onClick={() => setSelectedChannelId(item.id)} role="button" tabIndex={0} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') setSelectedChannelId(item.id) }}><ProviderLogo className="size-6 shrink-0" provider={item.providerType} /><div className="min-w-0"><p className="text-xs font-medium">{item.name}</p><p className="mt-1 text-[11px] text-muted-foreground">{providerLabel(item.providerType)} · {modeLabel(t, item.channelMode)} · {item.mappingCount ?? item.inputMappings?.length ?? 0} {t('applications.mappingCount')} · Revision {item.configurationRevision ?? item.version}</p><p className="mt-1 text-[10px] text-muted-foreground">{item.status === 'active' ? `${t('applications.channelEnabled')} · ${channelEndpointActive(item) ? t('applications.published') : t('applications.pendingPublish')}` : `${t('applications.channelDisabled')} · ${localizedValue(t, 'applications', item.status)}`}</p></div><div className="flex-1" />{auth.hasPermission('application:manage') && <Button onClick={(event) => { event.stopPropagation(); channelToggle.mutate({ webhook: item, status: item.status === 'active' ? 'disabled' : 'active' }) }} size="sm" type="button" variant="ghost">{item.status === 'active' ? t('applications.disableChannel') : t('applications.enableChannel')}</Button>}{auth.hasPermission('application:manage') && <Button onClick={(event) => { event.stopPropagation(); setSelectedWebhook(item); setDialog('webhookEdit') }} size="sm" type="button" variant="ghost">{t('common.edit')}</Button>}{auth.hasPermission('application:delete') && <EntityDeleteButton canDelete onDeleted={async () => { await queryClient.invalidateQueries({ queryKey: ['application-webhooks', id] }); if (selectedChannelId === item.id) setSelectedChannelId(null); showToast(t('applications.deleted')) }} deletePath={`/applications/${id}/webhooks/${item.id}`} entityId={item.id} entityName={item.name} entityType="application_webhook" />}</div>)}
          {!webhooks.isLoading && webhooks.data?.length === 0 && <p className="py-8 text-center text-xs text-muted-foreground">{t('applications.noChannels')}</p>}
        </Section>
        <Section title={t('applications.channelDetails')}>
          {selectedChannel && <div className="space-y-4 py-4">{selectedChannel.channelMode === 'stream' ? <><p className="text-xs text-muted-foreground">{t('applications.connectionStatus')}</p><div className="flex items-center gap-3"><StatusBadge label={connectionStatusLabel(t, selectedChannel.connectionStatus)} status={connectionStatus(selectedChannel.connectionStatus)} />{selectedChannel.lastConnectedAt && <span className="text-[11px] text-muted-foreground">{selectedChannel.lastConnectedAt}</span>}</div>{selectedChannel.connectionError && <p className="rounded-md border border-danger/30 bg-danger/5 p-3 text-[11px] text-danger">{selectedChannel.connectionError}</p>}<div className="rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground">{t('applications.streamNoEndpoint')}</div></> : <><p className="text-xs text-muted-foreground">{t('applications.productionEndpoint')}</p>{endpointActive ? <div className="flex items-start gap-2"><code className="min-w-0 flex-1 break-all rounded-md border border-border bg-canvas p-3 text-[11px]">{runtimePublicBaseUrl().replace(/\/$/, '')}{selectedChannel.path}</code><Button aria-label={t('common.copy')} onClick={() => void navigator.clipboard.writeText(`${runtimePublicBaseUrl().replace(/\/$/, '')}${selectedChannel.path}`)} size="icon" variant="ghost"><Copy className="size-4" /></Button></div> : <p className="rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground">{t('applications.endpointInactive')}</p>}</>}<p className="text-xs text-muted-foreground">{t('applications.sourceMapping')}</p><div className="space-y-2 text-[11px]">{(selectedChannel.inputMappings ?? []).map((mapping) => <div key={`${mapping.source}:${mapping.target}`}><strong>{mapping.target}</strong> ← <code>{mapping.source}</code></div>)}{(Object.entries(selectedChannel.fixedInputs ?? {}) as Array<[string, unknown]>).map(([key, item]) => <div key={`fixed:${key}`}><strong>{key}</strong> = <code>{typeof item === 'string' ? item : JSON.stringify(item)}</code></div>)}</div><p className="text-xs text-muted-foreground">{t('applications.replySettings')}</p>{selectedChannel.reply?.enabled ? <div className="rounded-md border border-border p-3 text-[11px]"><p><strong>{t('applications.replyOutputField')}</strong>: <code>{selectedChannel.reply.outputField}</code></p>{selectedChannel.reply.template && <p className="mt-1 break-all"><strong>{t('applications.replyTemplate')}</strong>: <code>{selectedChannel.reply.template}</code></p>}</div> : <p className="rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground">{t('applications.replyDisabled')}</p>}<div className="rounded-md border-l-2 border-primary bg-primary/5 p-3 text-[11px] text-muted-foreground">{t('applications.multiConversationHint')}</div></div>}
        </Section>
      </TabsContent>
      <TabsContent className="pt-5" value="triggers">
        {value.runtimeConfigRevision > value.publishedRuntimeConfigRevision && <div className="mb-4"><StatusBadge label={t('applications.pendingPublish')} status="pending" /></div>}
        <Section action={auth.hasPermission('application:manage') && <Button onClick={() => { setSelectedSchedule(null); setDialog('schedule') }} size="sm"><Plus className="size-4" />{t('common.create')}</Button>} title={t('applications.schedules')}>{schedules.data?.map((item) => <Row action={<div className="flex gap-1">{auth.hasPermission('application:manage') && <Button aria-label={t('common.edit')} onClick={() => { setSelectedSchedule(item); setDialog('scheduleEdit') }} size="icon" variant="ghost"><Edit3 className="size-4" /></Button>}<EntityDeleteButton canDelete={auth.hasPermission('application:delete')} deletePath={`/applications/${id}/schedules/${item.id}`} entityId={item.id} entityName={item.name} entityType="application_schedule" onDeleted={async () => { await queryClient.invalidateQueries({ queryKey: ['application-schedules', id] }); showToast(t('applications.deleted')) }} /></div>} detail={`${item.cronExpression} · ${item.timezone}`} key={item.id} status={localizedValue(t, 'applications', item.status)} title={item.name} />)}</Section>
      </TabsContent>
      <TabsContent className="pt-5" value="sessions"><Section action={auth.hasPermission('application:invoke') ? <Button asChild size="sm" variant="secondary"><Link to={`/playground?applicationId=${value.id}&mode=conversation`}><ExternalLink className="size-4" />{t('applications.openPlayground')}</Link></Button> : undefined} title={t('applications.playground.sessions')}><p className="py-3 text-xs text-muted-foreground">{t('applications.sessionDescription')}</p>{sessions.data?.map((item) => <Row action={<div className="flex gap-1">{auth.hasPermission('application:invoke') && <Button asChild size="sm" variant="ghost"><Link to={`/playground?applicationId=${value.id}&mode=conversation&sessionId=${item.id}`}>{t('applications.openSession')}</Link></Button>}{item.versionPolicy === 'manual_upgrade' && auth.hasPermission('application:manage') && <Button onClick={() => { setSelectedSession(item); setDialog('sessionUpgrade') }} size="sm" variant="ghost">{t('applications.upgrade')}</Button>}</div>} detail={`${localizedValue(t, 'applications', item.versionPolicy)} · ${item.workflowVersionId ?? '—'}`} key={item.id} status={<StatusBadge label={localizedValue(t, 'applications', item.status)} status={item.status === 'active' ? 'active' : 'inactive'} />} title={item.title ?? item.id} />)}{!sessions.isLoading && sessions.data?.length === 0 && <p className="border-t border-border py-8 text-center text-xs text-muted-foreground">{t('applications.noApplicationSessions')}</p>}</Section></TabsContent>
      <TabsContent className="pt-5" value="deliveries"><Section action={<Select aria-label={t('applications.deliveryStatusFilter')} className="w-40" onValueChange={setDeliveryStatus} options={[{ value: '', label: t('applications.deliveryStatusAll') }, { value: 'pending', label: t('applications.deliveryStatus_pending') }, { value: 'delivering', label: t('applications.deliveryStatus_delivering') }, { value: 'delivered', label: t('applications.deliveryStatus_delivered') }, { value: 'failed', label: t('applications.deliveryStatus_failed') }, { value: 'dead', label: t('applications.deliveryStatus_dead') }]} value={deliveryStatus} />} title={t('applications.deliveries')}>
        {deliveries.data?.items.map((item) => <Row action={item.status === 'dead' && auth.hasPermission('application:manage') ? <Button aria-label={t('applications.deliveryRetry')} onClick={() => deliveryRetry.mutate(item.id)} size="sm" variant="ghost"><RotateCw className="size-3.5" /></Button> : undefined} detail={<>{providerLabel(item.provider)} · {item.origin}{item.lastErrorCode && <span className="mt-1 block text-danger">{item.lastErrorCode}: {item.lastErrorMessage}</span>}<span className="mt-1 block">{item.lastErrorMessage ?? item.providerMessageId ?? ''}</span></>} key={item.id} status={<StatusBadge label={deliveryStatusLabel(t, item.status)} status={deliveryStatusBadge(item.status)} />} title={`${formatDateTime(item.createdAt)} · ${item.attemptCount} ${t('applications.deliveryAttempts')}`} />)}
        {!deliveries.isLoading && (deliveries.data?.items.length ?? 0) === 0 && <p className="py-8 text-center text-xs text-muted-foreground">{t('applications.noDeliveries')}</p>}
      </Section></TabsContent>
    </Tabs>
    {dialog && <EntityFormDialog cancelLabel={t('common.cancel')} fields={fields[dialog]} onClose={() => setDialog(null)} onSubmit={(values) => save.mutateAsync({ kind: dialog, values }).then(() => undefined)} open submitLabel={t('common.save')} title={dialog === 'webhookEdit' ? t('applications.editWebhook') : t(`applications.${dialog}`)} />}
    {integrationDoc && <ApplicationIntegrationDocs activeDeployment={activeDeployment} applicationSlug={value.slug} document={integrationDoc} onOpenChange={(open) => { if (!open) setIntegrationDoc(null) }} open runtimeBaseUrl={runtimePublicBaseUrl()} />}
    <ConfirmDialog cancelLabel={t('common.cancel')} confirmLabel={keyConfirmation?.action === 'rotate' ? t('applications.rotateKey') : t('applications.revoke')} description={keyConfirmation?.action === 'rotate' ? t('applications.confirmRotate') : t('applications.confirmRevoke')} onClose={() => setKeyConfirmation(null)} onConfirm={async () => { if (keyConfirmation) await keyAction.mutateAsync(keyConfirmation) }} open={Boolean(keyConfirmation)} pending={keyAction.isPending} title={keyConfirmation?.action === 'rotate' ? t('applications.rotateKey') : t('applications.revokeKey')} />
    <ConfirmDialog cancelLabel={t('common.cancel')} confirmLabel={deploymentConfirmation?.action === 'retry' ? t('applications.retryPublish') : t('applications.rollback')} description={deploymentConfirmation?.action === 'retry' ? t('applications.confirmRetryPublish') : t('applications.confirmRollback')} onClose={() => setDeploymentConfirmation(null)} onConfirm={async () => { if (deploymentConfirmation) await deploymentAction.mutateAsync(deploymentConfirmation) }} open={Boolean(deploymentConfirmation)} pending={deploymentAction.isPending} title={deploymentConfirmation?.action === 'retry' ? t('applications.retryPublish') : t('applications.rollback')} variant="primary" />
  </PageContainer>
}

type ReplyConfig = { enabled: boolean; outputField: string; template?: string | null }

function buildReplyConfig(values: Record<string, string>) {
  if (values.replyEnabled !== 'true') return null
  const outputField = (values.replyOutputField ?? '').trim()
  if (!outputField) return null
  const template = (values.replyTemplate ?? '').trim()
  return { enabled: true, outputField, template: template || null }
}

function replyConfigFields(t: (key: string) => string, outputSchema: unknown, reply: ReplyConfig | null | undefined): EntityFormField[] {
  const properties = schemaProperties(outputSchema)
  return [
    { name: 'replyEnabled', label: t('applications.replyEnabled'), type: 'select', defaultValue: reply?.enabled ? 'true' : 'false', required: true, options: [{ value: 'false', label: t('applications.replyOff') }, { value: 'true', label: t('applications.replyOn') }] },
    { name: 'replyOutputField', label: t('applications.replyOutputField'), type: properties.length ? 'select' : 'text', defaultValue: reply?.outputField ?? '', options: properties.map((property) => ({ value: property, label: property })) },
    { name: 'replyTemplate', label: t('applications.replyTemplate'), defaultValue: reply?.template ?? '' },
  ]
}

function deliveryStatusLabel(t: (key: string) => string, status: string) {
  const labels: Record<string, string> = {
    pending: t('applications.deliveryStatus_pending'),
    delivering: t('applications.deliveryStatus_delivering'),
    delivered: t('applications.deliveryStatus_delivered'),
    failed: t('applications.deliveryStatus_failed'),
    dead: t('applications.deliveryStatus_dead'),
  }
  return labels[status] ?? status
}

function deliveryStatusBadge(status: string): 'active' | 'pending' | 'running' | 'failed' | 'inactive' {
  if (status === 'delivered') return 'active'
  if (status === 'dead') return 'failed'
  if (status === 'delivering') return 'running'
  if (status === 'pending') return 'pending'
  return 'inactive'
}

function providerLabel(provider?: string) {
  return provider === 'dingtalk' ? '钉钉' : provider === 'wecom' ? '企业微信' : provider === 'feishu' ? '飞书' : provider === 'agentx' ? 'Agentx' : provider ?? '—'
}

function modeLabel(t: (key: string) => string, mode?: string) {
  return mode === 'stream' ? t('applications.channelModeStream') : t('applications.channelModeCallback')
}

function connectionStatus(status?: string | null): 'active' | 'pending' | 'running' | 'failed' | 'inactive' {
  if (status === 'connected') return 'active'
  if (status === 'reconnecting') return 'running'
  if (status === 'error') return 'failed'
  if (status === 'disconnected') return 'inactive'
  return 'pending'
}

function connectionStatusLabel(t: (key: string) => string, status?: string | null) {
  const labels: Record<string, string> = {
    pending: t('applications.connectionPending'),
    connected: t('applications.connectionConnected'),
    reconnecting: t('applications.connectionReconnecting'),
    disconnected: t('applications.connectionDisconnected'),
    error: t('applications.connectionError'),
  }
  return status ? labels[status] ?? status : t('applications.connectionPending')
}

/// Coerces fixed input strings to the Workflow Start Input schema type so
/// number/boolean parameters can be configured from a plain text field.
function parseFixedInputs(value: string, schema: unknown) {
  try {
    const parsed = JSON.parse(value || '{}') as Record<string, unknown>
    const result: Record<string, unknown> = {}
    for (const [key, item] of Object.entries(parsed)) {
      if (!key.trim()) continue
      const type = schemaPropertyType(schema, key)
      if (typeof item === 'string' && (type === 'number' || type === 'integer')) {
        const numeric = Number(item)
        result[key] = item.trim() !== '' && Number.isFinite(numeric) ? numeric : item
      } else if (typeof item === 'string' && type === 'boolean' && (item === 'true' || item === 'false')) {
        result[key] = item === 'true'
      } else {
        result[key] = item
      }
    }
    return result
  } catch { return {} }
}

function schemaProperties(schema: unknown): string[] {
  if (!schema || typeof schema !== 'object' || Array.isArray(schema)) return []
  const properties = (schema as { properties?: unknown }).properties
  if (!properties || typeof properties !== 'object' || Array.isArray(properties)) return []
  return Object.keys(properties)
}

function schemaPropertyType(schema: unknown, key: string): string | undefined {
  if (!schema || typeof schema !== 'object' || Array.isArray(schema)) return undefined
  const properties = (schema as { properties?: Record<string, { type?: unknown }> }).properties
  const type = properties?.[key]?.type
  return typeof type === 'string' ? type : undefined
}

function defaultChannelMappings(schema: unknown) {
  const properties = schemaProperties(schema)
  const target = (preferred: string, fallback: string) => properties.includes(preferred) ? preferred : properties[0] ?? fallback
  const questionTarget = target('question', 'question')
  const conversationTarget = properties.includes('conversation_id') ? 'conversation_id' : properties.find((property) => property !== questionTarget)
  return [
    { source: 'message.text', target: questionTarget, missingPolicy: 'error' },
    ...(conversationTarget ? [{ source: 'conversation.id', target: conversationTarget, missingPolicy: 'error' }] : []),
  ]
}

type MappingValue = { source: string; target: string; missingPolicy?: string }

function parseMappings(value: string): MappingValue[] {
  try {
    const parsed = JSON.parse(value || '[]') as unknown
    return Array.isArray(parsed) ? parsed.filter((item): item is MappingValue => Boolean(item) && typeof item === 'object' && typeof (item as MappingValue).source === 'string' && typeof (item as MappingValue).target === 'string') : []
  } catch { return [] }
}

/// Standardized Trigger Context sources shared by every provider. Provider
/// payload fields beyond these are addressed as raw.<dotted.path>.
function standardSourceOptions(t: (key: string) => string): Array<{ value: string; label: string }> {
  return [
    { value: 'message.text', label: t('applications.sourceMessageText') },
    { value: 'conversation.id', label: t('applications.sourceConversationId') },
    { value: 'conversation.name', label: t('applications.sourceConversationName') },
    { value: 'conversation.type', label: t('applications.sourceConversationType') },
    { value: 'sender.id', label: t('applications.sourceSenderId') },
    { value: 'sender.name', label: t('applications.sourceSenderName') },
    { value: 'provider', label: t('applications.sourceProvider') },
    { value: 'provider_event_id', label: t('applications.sourceProviderEventId') },
  ]
}

/// Mapping rows read workflow-input-first: pick the Workflow Start Input to
/// fill, then pick where its value comes from — a standardized channel field
/// or a provider payload field from the template catalog. Anything else
/// (e.g. an agentx caller's own JSON) can be typed as a raw.* path.
function MappingEditor({ schema, templates, provider, mode, value, onChange }: { schema: unknown; templates: WebhookProviderTemplate[]; provider?: string; mode?: string; value: string; onChange: (value: string) => void }) {
  const { t } = useTranslation()
  const rows = parseMappings(value)
  const targets = schemaProperties(schema)
  const effectiveMode = effectiveChannelMode({ providerType: provider ?? '', channelMode: mode ?? 'callback' })
  const providerSources = templates.find((item) => item.provider === provider && item.mode === effectiveMode)?.mappingSources ?? []
  const options = [
    ...standardSourceOptions(t),
    ...providerSources.map((source) => ({ value: source, label: source })),
  ]
  const update = (index: number, patch: Partial<MappingValue>) => onChange(JSON.stringify(rows.map((row, current) => current === index ? { ...row, ...patch } : row)))
  return <div className="space-y-2">
    <input name="inputMappings" type="hidden" value={value} />
    {rows.map((row, index) => {
      const knownSource = options.some((option) => option.value === row.source)
      return <div className="grid grid-cols-[1fr_1fr_auto] gap-2" key={`${index}:${row.source}:${row.target}`}>
        <Select aria-label={t('applications.workflowInput')} className="min-w-0" onValueChange={(target) => update(index, { target })} options={targets.map((target) => ({ value: target, label: target }))} value={row.target} />
        {knownSource
          ? <Select aria-label={t('applications.sourceField')} className="min-w-0" onValueChange={(source) => update(index, { source })} options={options} value={row.source} />
          : <Input aria-label={t('applications.sourceField')} className="min-w-0" onChange={(event) => update(index, { source: event.target.value })} placeholder="raw.senderNick" value={row.source} />}
        <Button aria-label={t('applications.removeMapping')} onClick={() => onChange(JSON.stringify(rows.filter((_, current) => current !== index)))} size="icon" type="button" variant="ghost"><Trash2 className="size-3.5" /></Button>
      </div>
    })}
    <div className="flex gap-2">
      <Button onClick={() => onChange(JSON.stringify([...rows, { source: 'message.text', target: targets[0] ?? '', missingPolicy: 'error' }]))} size="sm" type="button" variant="secondary"><Plus className="size-3.5" />{t('applications.addMapping')}</Button>
      <Button onClick={() => onChange(JSON.stringify([...rows, { source: 'raw.', target: targets[0] ?? '', missingPolicy: 'error' }]))} size="sm" type="button" variant="secondary"><Plus className="size-3.5" />{t('applications.addCustomMapping')}</Button>
    </div>
    <p className="text-[11px] leading-5 text-muted-foreground">{t('applications.mappingHint')}</p>
  </div>
}

/// Fixed inputs are workflow-input-first as well: pick the Workflow Start
/// Input, then type the constant value (coerced to the schema type on save).
function FixedInputsEditor({ schema, value, onChange }: { schema: unknown; value: string; onChange: (value: string) => void }) {
  const { t } = useTranslation()
  const targets = schemaProperties(schema)
  let entries: Array<[string, string]> = []
  try { const parsed = JSON.parse(value || '{}') as Record<string, unknown>; entries = Object.entries(parsed).map(([key, item]) => [key, typeof item === 'string' ? item : JSON.stringify(item)]) } catch { entries = [] }
  const update = (next: Array<[string, string]>) => { const object: Record<string, string> = {}; for (const [key, item] of next) object[key] = item; onChange(JSON.stringify(object)) }
  return <div className="space-y-2">
    <input name="fixedInputs" type="hidden" value={value} />
    {entries.map(([key, item], index) => <div className="grid grid-cols-[1fr_1fr_auto] gap-2" key={`${index}:${key}`}>
      {targets.length
        ? <Select aria-label={t('applications.workflowInput')} className="min-w-0" onValueChange={(nextKey) => { const next = entries.slice(); next[index] = [nextKey, item]; update(next) }} options={targets.map((target) => ({ value: target, label: target }))} value={targets.includes(key) ? key : ''} placeholder={key || undefined} />
        : <Input aria-label={t('applications.fixedInputName')} onChange={(event) => { const next = entries.slice(); next[index] = [event.target.value, item]; update(next) }} placeholder="department" value={key} />}
      <Input aria-label={t('applications.fixedInputValue')} onChange={(event) => { const next = entries.slice(); next[index] = [key, event.target.value]; update(next) }} placeholder={typePlaceholder(schema, key)} value={item} />
      <Button aria-label={t('applications.removeFixedInput')} onClick={() => update(entries.filter((_, current) => current !== index))} size="icon" type="button" variant="ghost"><Trash2 className="size-3.5" /></Button>
    </div>)}
    <Button onClick={() => update([...entries, [targets[0] ?? '', '']])} size="sm" type="button" variant="secondary"><Plus className="size-3.5" />{t('applications.addFixedInput')}</Button>
  </div>
}

function typePlaceholder(schema: unknown, key: string) {
  const type = schemaPropertyType(schema, key)
  return type === 'number' || type === 'integer' ? '42' : type === 'boolean' ? 'true' : 'customer_service'
}

function scheduleFields(t: (key: string) => string, value?: ApplicationSchedule | null): EntityFormField[] {
  return [
    { name: 'name', label: t('common.name'), required: true, defaultValue: value?.name ?? '' },
    { name: 'cron', label: 'Cron', defaultValue: value?.cronExpression ?? '0 9 * * 1', required: true },
    { name: 'timezone', label: t('applications.timezone'), defaultValue: value?.timezone ?? 'Asia/Shanghai', required: true },
    { name: 'misfirePolicy', label: t('applications.misfirePolicy'), type: 'select', defaultValue: value?.misfirePolicy ?? 'fire_once', required: true, options: ['fire_once', 'skip'].map((item) => ({ value: item, label: t(`applications.${item}`) })) },
    { name: 'input', label: t('applications.input'), type: 'textarea', defaultValue: JSON.stringify(value?.input ?? {}, null, 2), required: true },
    ...(value ? [{ name: 'status', label: t('common.status'), type: 'select' as const, defaultValue: value.status, required: true, options: ['active', 'disabled'].map((item) => ({ value: item, label: t(`applications.${item}`) })) }] : []),
  ]
}

function Section({ title, action, children }: { title: string; action?: React.ReactNode; children?: React.ReactNode }) { return <Card className="overflow-hidden"><div className="flex min-h-14 items-center justify-between border-b border-border px-5"><h2 className="text-sm font-semibold">{title}</h2>{action}</div><div className="px-5">{children || <p className="py-8 text-center text-xs text-muted-foreground">—</p>}</div></Card> }
function Row({ title, detail, status, action }: { title: string; detail: React.ReactNode; status: React.ReactNode; action?: React.ReactNode }) { return <div aria-label={title} className="flex min-h-16 items-center border-t border-border first:border-0" role="group"><div><p className="text-xs font-medium">{title}</p><div className="mt-1 text-[11px] text-muted-foreground">{detail}</div></div><div className="flex-1" /><span className="text-[11px] text-muted-foreground">{status}</span>{action && <div className="ml-2">{action}</div>}</div> }

function deploymentStatus(status: string): 'active' | 'inactive' | 'pending' | 'running' | 'failed' {
  if (status === 'active') return 'active'
  if (status === 'rejected') return 'failed'
  if (status === 'superseded') return 'inactive'
  if (status === 'activating') return 'running'
  return 'pending'
}

function deploymentStatusLabel(t: (key: string) => string, status: string): string {
  const labels: Record<string, string> = {
    building: t('applications.building'),
    copying: t('applications.copying'),
    preparing: t('applications.preparing'),
    prepared: t('applications.prepared'),
    activating: t('applications.activating'),
    active: t('applications.active'),
    rejected: t('applications.rejected'),
    superseded: t('applications.superseded'),
  }
  return labels[status] ?? status
}
