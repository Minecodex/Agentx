import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useSearchParams } from 'react-router-dom'
import { useAuth } from '../../../app/providers/auth-provider'
import { apiRequest, apiRequestText, jsonBody } from '../../../shared/api/client'
import { ConfirmDialog } from '../../../shared/components/confirm-dialog'
import { EntityFormDialog } from '../../../shared/components/entity-form-dialog'
import { Button } from '../../../shared/ui/button'
import { useToast } from '../../../shared/ui/toast'
import { DatasetCases, DatasetList } from './dataset-tables'
import { CaseEditor } from './case-editor'
import { DatasetCreate } from './dataset-create'
import { InvocationImport } from './invocation-import'
import { testingPath, type ContractVersion, type Dataset, type InvocationSample, type TestCase } from './types'

export function ApplicationDatasets({ applicationId, activeVersionNumber }: { applicationId: string; activeVersionNumber?: number | null }) {
  const { t } = useTranslation()
  const auth = useAuth()
  const client = useQueryClient()
  const { showToast } = useToast()
  const [params, setParams] = useSearchParams()
  const datasetId = params.get('datasetId')
  const invocationId = params.get('invocationId')
  const path = `${testingPath(applicationId)}/datasets`
  const datasets = useQuery({ queryKey: ['application-datasets', applicationId], queryFn: () => apiRequest<Dataset[]>(path) })
  const versions = useQuery({ queryKey: ['application-test-contracts', applicationId], queryFn: () => apiRequest<ContractVersion[]>(`${testingPath(applicationId)}/test-contracts`) })
  const sample = useQuery({ queryKey: ['application-test-sample', applicationId, invocationId], enabled: Boolean(invocationId), queryFn: () => apiRequest<InvocationSample>(`${testingPath(applicationId)}/invocations/${invocationId}/test-sample`) })
  const selected = datasets.data?.find(value => value.id === datasetId)
  const cases = useQuery({ queryKey: ['application-test-cases', applicationId, datasetId, selected?.revision], enabled: Boolean(selected), queryFn: () => apiRequest<TestCase[]>(`${path}/${datasetId}/cases`) })
  const [create, setCreate] = useState<Dataset | 'new'>()
  const [editor, setEditor] = useState<TestCase | 'new'>()
  const [removing, setRemoving] = useState<Dataset | TestCase>()
  const [selection, setSelection] = useState<string[]>([])
  const [exporting, setExporting] = useState(false)
  useEffect(() => setSelection([]), [datasetId])
  const [metadata, setMetadata] = useState<Dataset>()
  const file = useRef<HTMLInputElement>(null)
  const refresh = async () => { await Promise.all([client.invalidateQueries({ queryKey: ['application-datasets', applicationId] }), client.invalidateQueries({ queryKey: ['application-test-cases', applicationId] })]) }
  const select = (id?: string) => setParams(current => { const next = new URLSearchParams(current); if (id) next.set('datasetId', id); else next.delete('datasetId'); return next })
  const dismissSample = () => setParams(current => { const next = new URLSearchParams(current); next.delete('invocationId'); return next })
  const remove = useMutation({ mutationFn: async () => {
    if (!removing) return
    if ('contract' in removing) await apiRequest(`${path}/${removing.id}?expectedVersion=${removing.version}`, { method: 'DELETE' })
    else await apiRequest(`${path}/${selected!.id}/cases/${removing.id}`, { method: 'DELETE', body: jsonBody({ expectedRevision: selected!.revision }) })
  }, onSuccess: async () => { if (removing && 'contract' in removing && selected?.id === removing.id) select(); setRemoving(undefined); await refresh() }, onError: (error: Error) => showToast(error.message) })
  const importFile = async (value: File) => {
    try { await apiRequest(`${path}/${selected!.id}/import`, { method: 'POST', body: jsonBody({ expectedRevision: selected!.revision, content: await value.text(), format: value.name.endsWith('.csv') ? 'csv' : 'jsonl' }) }); await refresh(); showToast(t('testing.imported')) } catch (error) { showToast(error instanceof Error ? error.message : String(error)) }
  }
  const selectedIds = selection.filter(id => cases.data?.some(value => value.id === id))
  const exportFile = async () => {
    if (!selected || selectedIds.length === 0 || exporting) return
    setExporting(true)
    try { const value = await apiRequestText(`${path}/${selected.id}/export`, { method: 'POST', body: jsonBody({ expectedRevision: selected.revision, caseIds: selectedIds }) }); const url = URL.createObjectURL(new Blob([value], { type: 'application/x-ndjson' })); const anchor = document.createElement('a'); anchor.href = url; anchor.download = `${selected.name}.jsonl`; anchor.click(); URL.revokeObjectURL(url) } catch (error) { showToast(error instanceof Error ? error.message : String(error)) } finally { setExporting(false) }
  }

  return <div className="space-y-4">
    {sample.isLoading && <p>{t('common.loading')}</p>}{sample.error && <p role="alert" className="text-danger">{sample.error.message}</p>}
    <div className="flex items-center justify-between gap-4"><div className="flex items-center gap-3">{selected && <Button size="sm" variant="ghost" onClick={() => select()}>{t('testing.backDatasets')}</Button>}<div><h3 className="text-sm font-semibold">{selected?.name ?? t('testing.datasets')}</h3><p className="mt-1 text-xs text-muted-foreground">{selected ? t('testing.completeness', { count: selected.caseCount, incomplete: selected.incompleteCount }) : t('testing.fixedContractHint')}</p></div></div><div role="toolbar" aria-label={selected ? t('testing.caseActions') : t('testing.datasetActions')} className="flex items-center gap-3">{selected ? <>
      {selectedIds.length > 0 && <span className="text-xs text-muted-foreground">{t('testing.selectedCases', { count: selectedIds.length })}</span>}
      <div className="flex gap-2">{auth.hasPermission('dataset:manage') && <Button size="sm" variant="secondary" onClick={() => file.current?.click()}>{t('testing.import')}</Button>}<Button size="sm" variant="secondary" disabled={selectedIds.length === 0 || exporting} onClick={() => void exportFile()}>{t('testing.export')}</Button></div>
      {auth.hasPermission('dataset:manage') && <Button size="sm" onClick={() => setEditor('new')}>{t('testing.addCase')}</Button>}
    </> : auth.hasPermission('dataset:manage') && <Button onClick={() => setCreate('new')}>{t('testing.createDataset')}</Button>}</div></div>
    {datasets.isLoading && <p>{t('common.loading')}</p>}{datasets.error && <p role="alert" className="text-danger">{datasets.error.message}</p>}
    {selected ? <>{cases.error && <p role="alert" className="text-danger">{cases.error.message}</p>}<DatasetCases dataset={selected} versions={versions.data ?? []} cases={cases.data ?? []} selectedIds={selectedIds} onSelect={setSelection} canManage={auth.hasPermission('dataset:manage')} onEdit={setEditor} onDelete={setRemoving} /></> : <DatasetList datasets={datasets.data ?? []} versions={versions.data ?? []} canManage={auth.hasPermission('dataset:manage')} canDelete={auth.hasPermission('dataset:delete')} onSelect={select} onEdit={setMetadata} onCopy={setCreate} onDelete={setRemoving} />}
    <input ref={file} type="file" accept=".csv,.jsonl" className="hidden" onChange={event => { const value = event.target.files?.[0]; if (value) void importFile(value); event.target.value = '' }} />
    {sample.data && datasets.data && <InvocationImport key={sample.data.invocationId} applicationId={applicationId} sample={sample.data} datasets={datasets.data} selectedId={datasetId} onClose={dismissSample} onSaved={async id => { await refresh(); setParams(current => { const next = new URLSearchParams(current); next.set('datasetId', id); next.delete('invocationId'); return next }) }} />}
    {create && <DatasetCreate applicationId={applicationId} versions={versions.data ?? []} source={create === 'new' ? undefined : create} initialVersion={versions.data?.find(value => value.versionNumber === activeVersionNumber)?.id} onClose={() => setCreate(undefined)} onCreated={async value => { await refresh(); select(value.id) }} />}
    {editor && selected && <CaseEditor applicationId={applicationId} dataset={selected} selected={editor === 'new' ? undefined : editor} onClose={() => setEditor(undefined)} onSaved={refresh} />}
    {metadata && <EntityFormDialog open title={t('common.edit')} cancelLabel={t('common.cancel')} submitLabel={t('common.save')} fields={[{ name: 'name', label: t('common.name'), required: true, defaultValue: metadata.name }, { name: 'description', label: t('common.description'), type: 'textarea', defaultValue: metadata.description ?? '' }, { name: 'status', label: t('common.status'), type: 'select', defaultValue: metadata.status, options: [{ value: 'active', label: t('common.active') }, { value: 'disabled', label: t('common.inactive') }] }]} onClose={() => setMetadata(undefined)} onSubmit={async values => { await apiRequest(`${path}/${metadata.id}`, { method: 'PATCH', body: jsonBody({ ...values, version: metadata.version }) }); await refresh() }} />}
    <ConfirmDialog open={Boolean(removing)} title={t('common.delete')} description={t('testing.deleteConfirm', { name: removing?.name })} confirmLabel={t('common.delete')} cancelLabel={t('common.cancel')} onClose={() => setRemoving(undefined)} onConfirm={() => remove.mutateAsync().then(() => undefined)} pending={remove.isPending} />
  </div>
}
