import { approvalPolicyFields } from './mcp-approval-fields'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Activity, Bug, Pencil, RefreshCw, Settings2, TerminalSquare } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useParams } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { HealthCheck, McpDebugResult, McpDiscovery, McpServer, McpTool } from '../../shared/api/types'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'
import { JsonSchemaViewer } from '../../shared/components/json-schema-viewer'
import { ResourceDetailLayout } from '../../shared/components/resource-detail-layout'
import { StatusBadge } from '../../shared/components/status-badge'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { localizedValue } from '../../shared/lib/localized-value'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { useToast } from '../../shared/ui/toast'
import { McpCredentialPicker, McpEnvironmentCredentials, McpRuntimeSandboxPicker } from './mcp-transport-fields'
import { buildMcpTransport, transportArgs, transportCredential, transportEndpoint, transportEnvironment, transportKind, transportSandbox } from './mcp-transport'

const sideEffectOptions = [
  { value: 'unknown', labelKey: 'mcp.sideEffects.unknown' },
  { value: 'none', labelKey: 'mcp.sideEffects.none' },
  { value: 'read_only', labelKey: 'mcp.sideEffects.read_only' },
  { value: 'idempotent', labelKey: 'mcp.sideEffects.idempotent' },
  { value: 'non_idempotent', labelKey: 'mcp.sideEffects.non_idempotent' },
  { value: 'irreversible', labelKey: 'mcp.sideEffects.irreversible' },
] as const

export function McpServerDetailPage() {
  const { id = '' } = useParams()
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  const auth = useAuth()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [editOpen, setEditOpen] = useState(false)
  const [policyOpen, setPolicyOpen] = useState(false)
  const [debugOpen, setDebugOpen] = useState(false)
  const [selectedToolId, setSelectedToolId] = useState('')
  const [health, setHealth] = useState<HealthCheck>()
  const [debugResult, setDebugResult] = useState<McpDebugResult>()
  const server = useQuery({ queryKey: ['mcp-server', id], queryFn: () => apiRequest<McpServer>(`/mcp/servers/${id}`) })
  const tools = useQuery({ queryKey: ['mcp-tools', id], queryFn: () => apiRequest<McpTool[]>(`/mcp/servers/${id}/tools`) })
  const selected = tools.data?.find((tool) => tool.id === selectedToolId)
  useEffect(() => { if (!selectedToolId && tools.data?.[0]) setSelectedToolId(tools.data[0].id) }, [selectedToolId, tools.data])
  const invalidate = async () => Promise.all([queryClient.invalidateQueries({ queryKey: ['mcp-server', id] }), queryClient.invalidateQueries({ queryKey: ['mcp-servers'] }), queryClient.invalidateQueries({ queryKey: ['mcp-tools', id] })])
  const test = useMutation({ mutationFn: () => apiRequest<HealthCheck>(`/mcp/servers/${id}/test-connection`, { method: 'POST' }), onSuccess: (value) => { setHealth(value); showToast(localizedValue(t, 'mcp', value.status)) }, onError: (error: Error) => showToast(error.message) })
  const discover = useMutation({ mutationFn: () => apiRequest<McpDiscovery>(`/mcp/servers/${id}/discover`, { method: 'POST' }), onSuccess: async (value) => { await invalidate(); showToast(t('mcp.toolsDiscovered', { count: value.discoveredCount })) }, onError: (error: Error) => showToast(error.message) })
  const edit = async (values: Record<string, string>) => {
    let configuration: unknown
    try { configuration = JSON.parse(values.configuration || '{}') as unknown } catch { throw new Error(t('mcp.invalidJson')) }
    await apiRequest(`/mcp/servers/${id}`, { method: 'PATCH', body: jsonBody({ name: values.name, description: values.description || null, transport: buildMcpTransport(values), configuration, status: values.status, version: server.data?.version }) })
    await invalidate()
  }
  const updatePolicy = async (values: Record<string, string>) => {
    if (!selected) return
    await apiRequest(`/mcp/tools/${selected.id}/policy`, { method: 'PATCH', body: jsonBody({ enabled: values.enabled === 'true', debugEnabled: values.approvalMode === 'required' ? false : values.debugEnabled === 'true', approvalRule: values.approvalMode === 'required' ? {userIds:JSON.parse(values.approval_user||'[]') as string[],roleIds:JSON.parse(values.approval_role||'[]') as string[],timeoutSeconds:Number(values.approvalTimeout)} : null, timeoutSeconds: Number(values.timeout), sideEffect: values.sideEffect, version: selected.version }) })
    await invalidate()
  }
  const invoke = async (values: Record<string, string>) => {
    if (!selected) return
    let args: unknown
    try { args = JSON.parse(values.arguments || '{}') as unknown } catch { throw new Error(t('mcp.invalidJson')) }
    const result = await apiRequest<McpDebugResult>(`/mcp/tools/${selected.id}/debug-invoke`, { method: 'POST', body: jsonBody({ arguments: args, expectedToolVersionId: selected.currentVersionId, confirmed: true, confirmationText: values.confirmation || null }) })
    setDebugResult(result)
    showToast(t('mcp.debugSucceeded'))
  }
  const value = server.data
  const editFields: EntityFormField[] = value ? [
    { name: 'name', label: t('common.name'), defaultValue: value.name, required: true }, { name: 'description', label: t('common.description'), defaultValue: value.description ?? '' },
    { name: 'transport', label: t('mcp.transport'), type: 'select', defaultValue: transportKind(value.transport), required: true, options: [{ value: 'streamable_http', label: t('mcp.streamableHttp') }, { value: 'sse', label: t('mcp.legacySse') }, { value: 'stdio', label: t('mcp.stdio') }] },
    { name: 'endpoint', label: t('mcp.endpoint'), defaultValue: value.transport.kind === 'stdio' ? '' : value.transport.endpoint, visible: (values) => values.transport !== 'stdio' },
    { name: 'credential', label: t('mcp.credential'), defaultValue: transportCredential(value.transport), visible: (values) => values.transport !== 'stdio', render: ({ value: credential, update }) => <McpCredentialPicker departmentId={value.ownerDepartmentId} onChange={update} value={credential} /> },
    { name: 'command', label: t('mcp.command'), defaultValue: value.transport.kind === 'stdio' ? value.transport.command : '', required: true, visible: (values) => values.transport === 'stdio' },
    { name: 'args', label: t('mcp.args'), type: 'textarea', defaultValue: transportArgs(value.transport), description: t('mcp.argsDescription'), visible: (values) => values.transport === 'stdio' },
    { name: 'environmentCredentials', label: t('mcp.environmentCredentials'), defaultValue: transportEnvironment(value.transport), description: t('mcp.environmentCredentialsDescription'), visible: (values) => values.transport === 'stdio', render: ({ value: environment, update }) => <McpEnvironmentCredentials departmentId={value.ownerDepartmentId} onChange={update} value={environment} /> },
    { name: 'runtimeSandbox', label: t('mcp.runtimeSandbox'), defaultValue: transportSandbox(value.transport), required: true, description: t('mcp.runtimeSandboxDescription'), visible: (values) => values.transport === 'stdio', render: ({ value: sandbox, update }) => <McpRuntimeSandboxPicker departmentId={value.ownerDepartmentId} onChange={update} value={sandbox} /> },
    { name: 'status', label: t('common.status'), type: 'select', defaultValue: value.status, required: true, options: [{ value: 'active', label: t('common.active') }, { value: 'disabled', label: t('common.inactive') }] },
    { name: 'configuration', label: t('mcp.configuration'), type: 'textarea', defaultValue: '{}' },
  ] : []
  const policyFields: EntityFormField[] = selected ? [
    ...approvalPolicyFields(t,selected),
    { name: 'enabled', label: t('mcp.toolEnabled'), type: 'select', defaultValue: String(selected.enabled), required: true, options: booleanOptions(t) },
    { name: 'debugEnabled', label: t('mcp.debugEnabled'), type: 'select', defaultValue: String(selected.debugEnabled), required: true, options: booleanOptions(t) },
    { name: 'timeout', label: t('mcp.timeoutSeconds'), type: 'number', defaultValue: String(selected.timeoutSeconds), required: true },
    { name: 'sideEffect', label: t('mcp.sideEffect'), type: 'select', defaultValue: selected.sideEffect, required: true, options: sideEffectOptions.map(({ value, labelKey }) => ({ value, label: t(labelKey) })) },
  ] : []
  const debugFields: EntityFormField[] = selected ? [
    { name: 'arguments', label: t('mcp.debugArguments'), type: 'textarea', defaultValue: sampleArguments(selected.inputSchema), required: true },
    { name: 'confirmation', label: t('mcp.confirmToolName'), defaultValue: requiresName(selected.sideEffect) ? '' : selected.name, placeholder: selected.name, required: requiresName(selected.sideEffect) },
  ] : []
  const actions = <div className="flex gap-2">
    {auth.hasPermission('mcp:manage') && <Button onClick={() => setEditOpen(true)} variant="secondary"><Pencil className="size-4" />{t('common.edit')}</Button>}
    {auth.hasPermission('mcp:manage') && <Button disabled={test.isPending} onClick={() => test.mutate()} variant="secondary"><Activity className="size-4" />{t('mcp.connectionTest')}</Button>}
    {auth.hasPermission('mcp:discover') && <Button disabled={discover.isPending} onClick={() => discover.mutate()}><RefreshCw className="size-4" />{t('mcp.discoverTools')}</Button>}
  </div>
  return <>
    <ResourceDetailLayout actions={actions} description={value?.description ?? t('mcp.description')} details={value ? [
      { label: t('mcp.transport'), value: transportKind(value.transport) }, { label: t('mcp.endpoint'), value: transportEndpoint(value.transport) },
      { label: t('mcp.discoveredTools'), value: value.toolCount }, { label: t('mcp.lastDiscovered'), value: value.lastDiscoveredAt ? formatDateTime(value.lastDiscoveredAt) : undefined },
      { label: t('mcp.serverVersion'), value: `v${value.currentVersionNumber}` }, { label: t('mcp.department'), value: value.ownerDepartmentId },
    ] : []} error={server.error} loading={server.isLoading} name={value?.name} status={value?.status}>
      {health && <Card className="flex items-center gap-3 p-4 text-xs"><Activity className="size-4 text-primary" /><StatusBadge label={localizedValue(t, 'mcp', health.status)} status={health.status === 'healthy' ? 'active' : 'inactive'} /><span>{health.latencyMs ?? 0} ms</span>{health.errorMessage && <span className="text-danger">{health.errorMessage}</span>}</Card>}
      <div className="grid min-h-[480px] grid-cols-[240px_minmax(0,1fr)] overflow-hidden rounded-lg border border-border bg-surface">
        <div className="border-r border-border p-3"><div className="mb-3 flex items-center gap-2 px-2 text-xs font-semibold"><TerminalSquare className="size-4 text-primary" />{t('mcp.discoveredTools')}</div>{tools.data?.map((tool) => <button className={`mb-1 w-full rounded-md px-3 py-2 text-left text-xs ${tool.id === selectedToolId ? 'bg-primary/10 text-primary' : 'hover:bg-muted'}`} key={tool.id} onClick={() => { setSelectedToolId(tool.id); setDebugResult(undefined) }} type="button"><span className="block truncate font-medium">{tool.title ?? tool.name}</span><span className="mt-1 block truncate text-[10px] text-muted-foreground">{tool.name} · v{tool.currentVersionNumber}</span></button>)}{tools.data?.length === 0 && <p className="px-2 py-6 text-xs text-muted-foreground">{t('mcp.noDiscoveredTools')}</p>}</div>
        <div className="min-w-0 p-5">{selected ? <><div className="flex items-start gap-3"><div><h2 className="text-sm font-semibold">{selected.title ?? selected.name}</h2><p className="mt-1 text-xs text-muted-foreground">{selected.description ?? selected.name}</p></div><div className="flex-1" />{auth.hasPermission('mcp:manage') && <Button onClick={() => setPolicyOpen(true)} size="sm" variant="secondary"><Settings2 className="size-3.5" />{t('mcp.toolPolicy')}</Button>}{auth.hasPermission('mcp:debug') && <Button disabled={!selected.debugEnabled} onClick={() => setDebugOpen(true)} size="sm"><Bug className="size-3.5" />{t('mcp.debugInvoke')}</Button>}</div><div className="mt-5 grid grid-cols-3 gap-3 text-xs"><div><span className="text-muted-foreground">{t('common.status')}</span><p className="mt-1">{selected.availability}</p></div><div><span className="text-muted-foreground">{t('mcp.sideEffect')}</span><p className="mt-1">{selected.sideEffect}</p></div><div><span className="text-muted-foreground">{t('mcp.timeoutSeconds')}</span><p className="mt-1">{selected.timeoutSeconds}s</p></div></div><div className="mt-5 space-y-5"><SchemaPanel label={t('mcp.inputSchema')} value={selected.inputSchema} /><SchemaPanel label={t('mcp.outputSchema')} value={selected.outputSchema} /></div>{debugResult && <div className="mt-5"><h3 className="text-xs font-semibold">{t('mcp.debugResult')} · {debugResult.durationMs} ms</h3><pre className="mt-2 max-h-64 overflow-auto rounded-md bg-canvas p-3 text-[11px]">{JSON.stringify(debugResult.result, null, 2)}</pre></div>}</> : <p className="text-xs text-muted-foreground">{t('mcp.selectTool')}</p>}</div>
      </div>
    </ResourceDetailLayout>
    {editOpen && <EntityFormDialog cancelLabel={t('common.cancel')} fields={editFields} onClose={() => setEditOpen(false)} onSubmit={edit} open submitLabel={t('common.save')} title={t('mcp.editMcp')} />}
    {policyOpen && <EntityFormDialog cancelLabel={t('common.cancel')} fields={policyFields} onClose={() => setPolicyOpen(false)} onSubmit={updatePolicy} open submitLabel={t('common.save')} title={t('mcp.toolPolicy')} />}
    {debugOpen && <EntityFormDialog cancelLabel={t('common.cancel')} fields={debugFields} onClose={() => setDebugOpen(false)} onSubmit={invoke} open submitLabel={t('mcp.confirmInvoke')} title={t('mcp.debugInvoke')} />}
  </>
}

function SchemaPanel({ label, value }: { label: string; value: unknown }) {
  return <section className="min-w-0"><h3 className="mb-2 text-xs font-semibold">{label}</h3><JsonSchemaViewer schema={value} /></section>
}

function booleanOptions(t: (key: string) => string) { return [{ value: 'true', label: t('mcp.yes') }, { value: 'false', label: t('mcp.no') }] }
function requiresName(value: string) { return ['unknown', 'non_idempotent', 'irreversible'].includes(value) }
function sampleArguments(schema: unknown) {
  const value = schema as { properties?: Record<string, { type?: string }> }
  return JSON.stringify(Object.fromEntries(Object.entries(value.properties ?? {}).map(([key, property]) => [key, property.type === 'string' ? '' : null])), null, 2)
}
