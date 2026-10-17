import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useParams } from 'react-router-dom'
import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest } from '../../shared/api/client'
import type { Department, ExternalConnection, Knowledge, PageResponse } from '../../shared/api/types'
import { EntityDeleteButton } from '../../shared/components/entity-delete-button'
import { ResourceDetailLayout } from '../../shared/components/resource-detail-layout'
import { ResourceGrantDialog } from '../../shared/components/resource-grant-panel'
import { StatusBadge } from '../../shared/components/status-badge'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { ConnectionForm } from './connection-form'
import { QueryForm } from './query-form'
import { scopeLabel } from './scope'
import { QueryInspectDialog } from './query-inspect-dialog'

export function KnowledgeDetailPage() {
  const { id = '' } = useParams(); const { t } = useTranslation(); const auth = useAuth(); const client = useQueryClient()
  const [editConnection, setEditConnection] = useState(false)
  const [editing, setEditing] = useState<Knowledge | 'new'>(); const [grant, setGrant] = useState<Knowledge>()
  const [inspect, setInspect] = useState<{ query: Knowledge; mode: 'test' | 'documents' }>(); const [page, setPage] = useState(1)
  const connection = useQuery({ queryKey: ['knowledge-connection', id], queryFn: () => apiRequest<ExternalConnection>(`/knowledge/connections/${id}`) })
  const queries = useQuery({ queryKey: ['knowledge-queries', id, page], queryFn: () => apiRequest<PageResponse<Knowledge>>(`/knowledge/connections/${id}/queries?page=${page}&pageSize=20`) })
  const refresh = async () => { await Promise.all([client.invalidateQueries({ queryKey: ['knowledge-queries', id] }), client.invalidateQueries({ queryKey: ['studio-resource-options'] })]) }
  const departments = useQuery({ queryKey: ['departments'], queryFn: () => apiRequest<Department[]>('/departments') })
  const departmentName = (id: string) => departments.data?.find(item => item.id === id)?.name ?? id
  const value = connection.data
  return <ResourceDetailLayout loading={connection.isLoading} error={connection.error} name={value?.name} status={value?.status} description={t('knowledge.connectionDescription')} details={value ? [{ label: t('knowledge.provider'), value: value.provider }, { label: t('knowledge.endpoint'), value: value.endpoint }, { label: t('knowledge.department'), value: departmentName(value.ownerDepartmentId) }] : []} actions={<div className="flex gap-2"><Button asChild variant="ghost"><Link to="/knowledge">{t('knowledge.back')}</Link></Button>{value && auth.hasPermission('knowledge:manage') && <Button variant="secondary" onClick={() => setEditConnection(true)}>{t('knowledge.editConnection')}</Button>}</div>}>
    <Card className="overflow-hidden">
      <div className="flex min-h-16 items-center justify-between border-b border-border px-5"><h2 className="text-sm font-semibold">{t('knowledge.queries')}</h2>{auth.hasPermission('knowledge:manage') && <Button size="sm" disabled={value?.status !== 'active'} onClick={() => setEditing('new')}><Plus className="size-4" />{t('knowledge.addQuery')}</Button>}</div>
      {queries.isLoading && <p className="p-5 text-xs">{t('common.loading')}</p>}{queries.error && <p role="alert" className="p-5 text-xs text-danger">{queries.error.message}</p>}
      <div className="divide-y divide-border">{queries.data?.items.map(query => <div className="flex items-start gap-4 p-5" key={query.id}>
        <div className="min-w-0 flex-1"><div className="flex items-center gap-3"><h3 className="text-sm font-medium">{query.name}</h3><StatusBadge status={query.status === 'active' ? 'active' : 'inactive'} /></div><p className="mt-2 text-xs text-muted-foreground">{query.description}</p><p className="mt-2 break-all text-[11px] text-muted-foreground">{t('knowledge.scope')}: {scopeLabel(query.scope, t('knowledge.defaultWorkspace'))}</p><p className="mt-1 text-[11px] text-muted-foreground">{t('knowledge.department')}: {departmentName(query.ownerDepartmentId)} · {t('knowledge.grantCount', { count: query.grantCount })}</p></div>
        <div className="flex max-w-[500px] flex-wrap justify-end gap-1">
          <Button size="sm" variant="ghost" disabled={query.status !== 'active' || value?.status !== 'active'} onClick={() => setInspect({ query, mode: 'documents' })}>{t('knowledge.documents')}</Button>
          {auth.hasPermission('knowledge:manage') && <><Button size="sm" variant="ghost" disabled={query.status !== 'active' || value?.status !== 'active'} onClick={() => setInspect({ query, mode: 'test' })}>{t('knowledge.retrievalTest')}</Button><Button size="sm" variant="ghost" onClick={() => setEditing(query)}>{t('common.edit')}</Button></>}
          <Button size="sm" variant="ghost" onClick={() => setGrant(query)}>{t('knowledge.authorization')}</Button>
          <EntityDeleteButton canDelete={auth.hasPermission('knowledge:delete')} entityId={query.id} entityName={query.name} entityType="knowledge" deletePath={`/knowledge/queries/${query.id}`} onDeleted={refresh} />
        </div>
      </div>)}</div>
      {queries.data?.items.length === 0 && <p className="p-8 text-center text-xs text-muted-foreground">{t('knowledge.noQueries')}</p>}
      <div className="flex justify-end gap-2 border-t border-border p-3"><Button size="sm" variant="ghost" disabled={page === 1} onClick={() => setPage(page - 1)}>{t('knowledge.previous')}</Button><Button size="sm" variant="ghost" disabled={!queries.data || page * 20 >= queries.data.total} onClick={() => setPage(page + 1)}>{t('knowledge.next')}</Button></div>
    </Card>
    {value && editConnection && <ConnectionForm connection={value} onClose={() => setEditConnection(false)} onSaved={async () => { await client.invalidateQueries({ queryKey: ['knowledge-connection', id] }); await client.invalidateQueries({ queryKey: ['knowledge-connections'] }) }} />}
    {value && editing && <QueryForm connection={value} query={editing === 'new' ? undefined : editing} onClose={() => setEditing(undefined)} onSaved={async (query, configureGrants) => { await refresh(); if (configureGrants) setGrant(query) }} />}
    {grant && <ResourceGrantDialog open resource={{ id: grant.id, name: grant.name, resourceType: 'rag' }} onClose={() => { setGrant(undefined); void refresh() }} />}
    {inspect && <QueryInspectDialog key={`${inspect.query.id}-${inspect.mode}`} {...inspect} onClose={() => setInspect(undefined)} />}
  </ResourceDetailLayout>
}
