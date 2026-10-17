import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { Department, ExternalConnection, Knowledge } from '../../shared/api/types'
import { EntityFormDialog, type EntityFormField } from '../../shared/components/entity-form-dialog'
import { Button } from '../../shared/ui/button'
import { Select } from '../../shared/ui/select'

type Scope = Knowledge['scope']
export type CatalogItem = { id: string; name: string; scope: Scope }
export function QueryForm({ connection, query, onClose, onSaved }: { connection: ExternalConnection; query?: Knowledge; onClose: () => void; onSaved: (query: Knowledge, grants: boolean) => Promise<void> }) {
  const { t } = useTranslation()
  const catalog = useQuery({ queryKey: ['knowledge-catalog', connection.id], queryFn: () => apiRequest<CatalogItem[]>(`/knowledge/connections/${connection.id}/catalog`), staleTime: 0 })
  const departments = useQuery({ queryKey: ['departments'], queryFn: () => apiRequest<Department[]>('/departments') })
  const fields: EntityFormField[] = [
    { name: 'name', label: t('knowledge.queryName'), required: true, defaultValue: query?.name, maxLength: 160 },
    { name: 'description', label: t('knowledge.purpose'), type: 'textarea', maxLength: 2000, defaultValue: query?.description },
    { name: 'scope', label: t(connection.provider === 'ragflow' ? 'knowledge.datasets' : 'knowledge.workspace'), defaultValue: query ? JSON.stringify(query.scope) : '', required: true, render: ({ value, update }) => {
      const selected: Scope | undefined = value ? JSON.parse(value) : undefined
      return <div className="space-y-2">
        {catalog.isLoading && <p>{t('common.loading')}</p>}
        {catalog.error && <p role="alert" className="text-danger">{catalog.error.message}</p>}
        {connection.provider === 'ragflow' ? <div className="max-h-48 space-y-2 overflow-auto rounded-lg border border-border p-3">{catalog.data?.map(item => <label className="flex items-center gap-2" key={item.id}>
          <input type="checkbox" className="size-4 accent-primary" checked={selected?.kind === 'ragflow' && selected.datasetIds.includes(item.id)} onChange={e => { const ids = selected?.kind === 'ragflow' ? selected.datasetIds : []; update(JSON.stringify({ kind: 'ragflow', datasetIds: e.target.checked ? [...ids, item.id] : ids.filter(id => id !== item.id) })) }} />
          <span>{item.name}<span className="ml-2 text-muted-foreground">{item.id}</span></span>
        </label>)}</div> : <Select aria-label={t('knowledge.workspace')} value={value} onValueChange={update} options={(catalog.data ?? []).map(item => ({ value: JSON.stringify(item.scope), label: item.name || t('knowledge.defaultWorkspace') }))} />}
        <p className="text-muted-foreground">{t(connection.provider === 'ragflow' ? 'knowledge.datasetsHint' : 'knowledge.workspaceHint')}</p>
        <Button size="sm" variant="ghost" type="button" disabled={catalog.isFetching} onClick={() => void catalog.refetch()}>{t('knowledge.refreshScopes')}</Button>
      </div>
    } },
    { name: 'ownerDepartmentId', label: t('knowledge.department'), type: 'select', required: true, defaultValue: query?.ownerDepartmentId ?? connection.ownerDepartmentId, options: (departments.data ?? []).map(item => ({ value: item.id, label: item.name })) },
    ...(query ? [{ name: 'status', label: t('common.status'), type: 'select' as const, defaultValue: query.status, options: [{ value: 'active', label: t('common.active') }, { value: 'disabled', label: t('common.inactive') }] }] : []),
    { name: 'configureGrants', label: t('knowledge.configureGrants'), type: 'checkbox', defaultValue: 'true', description: t('knowledge.grantsHint') },
  ]
  return <EntityFormDialog open fields={fields} title={t(query ? 'knowledge.editQuery' : 'knowledge.addQuery')} onClose={onClose} cancelLabel={t('common.cancel')} submitLabel={t('common.save')} onSubmit={async values => {
    const { configureGrants, scope: rawScope, ...rest } = values
    const scope: Scope = JSON.parse(rawScope)
    if ((scope.kind === 'ragflow' && !scope.datasetIds.length) || !catalog.data || catalog.error) throw new Error(t('knowledge.selectScope'))
    const saved = await apiRequest<Knowledge>(query ? `/knowledge/queries/${query.id}` : `/knowledge/connections/${connection.id}/queries`, { method: query ? 'PATCH' : 'POST', body: jsonBody({ ...rest, scope, ...(query ? { version: query.version } : {}) }) })
    await onSaved(saved, configureGrants === 'true')
  }} />
}
