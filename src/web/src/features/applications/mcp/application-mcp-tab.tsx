import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { History, Pencil, Radio } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest, jsonBody } from '../../../shared/api/client'
import type { components } from '../../../shared/api/generated'
import type { ApplicationDeployment } from '../../../shared/api/types'
import { EntityFormDialog, type EntityFormField } from '../../../shared/components/entity-form-dialog'
import { Button } from '../../../shared/ui/button'
import { Card } from '../../../shared/ui/card'
import { McpDocumentation } from './mcp-documentation'
import { McpPublicationHistory } from './mcp-publication-history'

export type McpRevision = components['schemas']['ApplicationMcpRevision']

export function ApplicationMcpTab({ applicationId, deployments, defaultDescription }: {
  applicationId: string
  deployments: ApplicationDeployment[]
  defaultDescription: string
}) {
  const { t } = useTranslation()
  const query = useQueryClient()
  const [editing, setEditing] = useState(false)
  const [history, setHistory] = useState(false)
  const [selectedId, setSelectedId] = useState<string>()
  const path = `/applications/${applicationId}/mcp/publications`
  const versions = useQuery({
    queryKey: ['application-mcp', applicationId],
    queryFn: () => apiRequest<McpRevision[]>(path),
    refetchInterval: query => query.state.data?.some(item => item.status !== 'published' || (item.deleted && item.deletionStatus !== 'published')) ? 1500 : false,
  })
  const rows = versions.data ?? []
  const retained = rows.filter(item => !item.deleted && item.status === 'published')
  const active = retained.find(item => item.publication.publicationId === selectedId) ?? retained[0]
  const pending = rows.some(item => item.status !== 'published' || (item.deleted && item.deletionStatus !== 'published'))
  const refresh = () => query.invalidateQueries({ queryKey: ['application-mcp', applicationId] })
  const currentDeployments = deployments.filter(item => item.status === 'active')
  const previousDeployment = deployments.find(item => item.id === active?.publication.deploymentId)
  const businessDeployment = active ? currentDeployments.find(item => item.environmentId === previousDeployment?.environmentId) : currentDeployments[0]
  const remove = useMutation({ mutationFn: (id: string) => apiRequest(`${path}/${id}`, { method: 'DELETE' }), onSuccess: refresh })
  const fields: EntityFormField[] = [
    { name: 'toolName', label: t('applicationMcp.toolName'), required: true, maxLength: 64, defaultValue: active?.publication.toolName ?? 'run_application', description: t('applicationMcp.toolNameHint') },
    { name: 'slug', label: t('applicationMcp.slug'), required: true, maxLength: 80, defaultValue: active?.publication.slug ?? `app-${applicationId.slice(-12)}`, description: t('applicationMcp.slugHint') },
    ...(active ? [{ name: 'description', label: t('applicationMcp.purpose'), type: 'textarea' as const, required: true, maxLength: 4096, defaultValue: active.publication.description, description: t('applicationMcp.changeHint') }] : []),
    ...(!active && currentDeployments.length > 1 ? [{ name: 'deploymentId', label: t('applicationMcp.deployment'), type: 'select' as const, required: true, defaultValue: currentDeployments[0].id, options: currentDeployments.map(item => ({ value: item.id, label: `${item.environmentName} · v${item.workflowVersionNumber}` })) }] : []),
  ]
  return <div className="space-y-5" data-testid="application-mcp-tab">
    {versions.isLoading ? <p className="text-sm text-muted-foreground">{t('common.loading')}</p> : active ? <>
      <div className="flex items-start justify-between gap-6">
        <div><h2 className="text-base font-semibold">{t('applicationMcp.documentation')}</h2><p className="mt-1 text-xs text-muted-foreground">{t('applicationMcp.description')}</p></div>
        <div className="flex shrink-0 items-center gap-2">
          <Button size="sm" variant="secondary" onClick={() => setHistory(true)}><History className="mr-2 size-3.5" />{t('applicationMcp.history')}</Button>
          <Button size="sm" disabled={pending || !businessDeployment} title={!businessDeployment ? t('applicationMcp.deploymentRequired') : undefined} onClick={() => setEditing(true)}><Pencil className="mr-2 size-3.5" />{t('applicationMcp.change')}</Button>
        </div>
      </div>
      <McpDocumentation applicationId={applicationId} revision={active} />
    </> : !versions.error && <Card className="flex min-h-72 flex-col items-center justify-center gap-4 p-8">
      <Radio className="size-9 text-muted-foreground" aria-hidden="true" />
      <Button disabled={pending || !currentDeployments.length} title={!currentDeployments.length ? t('applicationMcp.deploymentRequired') : undefined} onClick={() => setEditing(true)}>{t('applicationMcp.publish')}</Button>
      {pending && <p className="text-xs text-muted-foreground" role="status">{t('applicationMcp.pending')}</p>}
      {rows.some(item => !item.deleted || item.deletionStatus !== 'published') && <Button size="sm" variant="ghost" onClick={() => setHistory(true)}>{t('applicationMcp.history')}</Button>}
    </Card>}
    {active && pending && <p className="text-xs text-muted-foreground" role="status">{t('applicationMcp.pending')}</p>}
    {versions.error && <p role="alert" className="text-sm text-danger">{versions.error.message}</p>}
    {history && <McpPublicationHistory rows={rows} removing={remove.isPending} error={remove.error?.message} onClose={() => setHistory(false)} onSelect={id => { setSelectedId(id); setHistory(false) }} onRemove={async id => { await remove.mutateAsync(id) }} />}
    {editing && <EntityFormDialog open title={t(active ? 'applicationMcp.change' : 'applicationMcp.publish')} submitLabel={t('applicationMcp.publish')} cancelLabel={t('common.cancel')} fields={fields} onClose={() => setEditing(false)} onSubmit={async values => {
      const result = await apiRequest<McpRevision>(path, { method: 'POST', body: jsonBody({
        deploymentId: active ? businessDeployment?.id : values.deploymentId ?? businessDeployment?.id,
        expectedRevision: rows[0]?.publication.revision ?? 0,
        toolName: values.toolName.trim(), slug: values.slug.trim(),
        description: active ? values.description.trim() : defaultDescription.trim(), enabled: true,
      }) })
      setSelectedId(result.publication.publicationId)
      await refresh()
    }} />}
  </div>
}
