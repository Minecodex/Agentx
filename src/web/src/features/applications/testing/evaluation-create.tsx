import { useMutation, useQuery } from '@tanstack/react-query'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import { apiRequest, jsonBody } from '../../../shared/api/client'
import { Button } from '../../../shared/ui/button'
import { Dialog, DialogContent } from '../../../shared/ui/dialog'
import { Input } from '../../../shared/ui/input'
import { Select } from '../../../shared/ui/select'
import { testingPath, type ContractVersion, type Dataset, type Profile } from './types'

type Selection = { id: string; datasetId: string; workflowVersionIds: string[] }

export function EvaluationCreate({ applicationId, onClose, onCreated }: { applicationId: string; onClose: () => void; onCreated: (id: string) => Promise<void> }) {
  const { t } = useTranslation()
  const path = testingPath(applicationId)
  const datasets = useQuery({ queryKey: ['application-datasets', applicationId], queryFn: () => apiRequest<Dataset[]>(`${path}/datasets`) })
  const versions = useQuery({ queryKey: ['application-test-contracts', applicationId], queryFn: () => apiRequest<ContractVersion[]>(`${path}/test-contracts`) })
  const profiles = useQuery({ queryKey: ['application-profiles', applicationId], queryFn: () => apiRequest<Profile[]>(`${path}/evaluation-profiles`) })
  const model = useQuery({ queryKey: ['platform-ai-model-setting'], queryFn: () => apiRequest<{ modelAlias: string | null; status: string }>('/platform-ai/model-setting') })
  const [name, setName] = useState('')
  const [profileId, setProfileId] = useState('')
  const [selections, setSelections] = useState<Selection[]>([{ id: 'first', datasetId: '', workflowVersionIds: [] }])
  const submitted = useRef<{ body: string; key: string }>()
  const chosenProfile = profileId || profiles.data?.[0]?.versionId || ''
  const compatibleVersions = (dataset: Dataset) => (versions.data ?? []).filter(version => dataset.compatibleVersions.includes(version.id)).sort((a, b) => a.versionNumber - b.versionNumber)
  const readyDatasets = (datasets.data ?? []).filter(value => value.status === 'active' && value.caseCount > 0 && value.incompleteCount === 0 && compatibleVersions(value).length > 0)
  const start = useMutation({ mutationFn: () => {
    const body = jsonBody({ name: name.trim(), profileVersionId: chosenProfile, selections: selections.map(({ datasetId, workflowVersionIds }) => ({ datasetId, expectedRevision: datasets.data!.find(value => value.id === datasetId)!.revision, workflowVersionIds })) })
    if (submitted.current?.body !== body) submitted.current = { body, key: crypto.randomUUID() }
    return apiRequest<{ id: string }>(`${path}/evaluations`, { method: 'POST', headers: { 'Idempotency-Key': submitted.current!.key }, body })
  }, onSuccess: async value => { await onCreated(value.id); onClose() } })
  const ready = Boolean(name.trim() && profiles.data?.some(value => value.versionId === chosenProfile) && model.data?.status === 'ready' && selections.length > 0 && selections.every(row => {
    const dataset = readyDatasets.find(value => value.id === row.datasetId)
    return dataset && row.workflowVersionIds.length > 0 && row.workflowVersionIds.every(id => compatibleVersions(dataset).some(version => version.id === id))
  }))
  const loading = datasets.isLoading || versions.isLoading || profiles.isLoading || model.isLoading
  const queryError = datasets.error ?? versions.error ?? profiles.error ?? model.error
  const error = queryError ?? start.error
  const updateRow = (id: string, update: Partial<Selection>) => setSelections(current => current.map(value => value.id === id ? { ...value, ...update } : value))

  return <Dialog open onOpenChange={open => { if (!open && !start.isPending) onClose() }}><DialogContent title={t('testing.createEvaluation')} description={t('testing.startHint')} className="w-[min(960px,calc(100vw-48px))]"><form onSubmit={event => { event.preventDefault(); if (ready && !queryError) start.mutate() }}>
    <div className="border-b border-border p-6"><h2 className="text-base font-semibold">{t('testing.createEvaluation')}</h2><p className="mt-2 text-xs text-muted-foreground">{t('testing.startHint')}</p></div>
    <div className="space-y-5 p-6"><label className="block space-y-2 text-xs"><span>{t('testing.evaluationName')}</span><Input aria-label={t('testing.evaluationName')} required value={name} onChange={event => setName(event.target.value)} disabled={start.isPending} /></label>
      <div className="grid grid-cols-2 gap-4"><label className="block space-y-2 text-xs"><span>{t('testing.profile')}</span><Select className="w-full" aria-label={t('testing.profile')} value={chosenProfile} onValueChange={setProfileId} options={(profiles.data ?? []).map(value => ({ value: value.versionId, label: `${value.name} · v${value.versionNumber}` }))} disabled={start.isPending} /></label><div className="space-y-2 text-xs"><span>{t('testing.judgeModel')}</span><p className="rounded-lg border border-border px-3 py-2.5">{model.data?.modelAlias ?? t('testing.modelUnavailable')}</p>{!loading && model.data?.status !== 'ready' && <Link className="text-primary" to="/models" target="_blank">{t('testing.configureModel')}</Link>}</div></div>
      {!profiles.isLoading && profiles.data?.length === 0 && <p className="text-xs text-warning">{t('testing.noProfiles')}</p>}
      {loading ? <p>{t('common.loading')}</p> : <section className="space-y-3"><h3 className="text-sm font-semibold">{t('testing.datasetVersions')}</h3>
        {selections.map((row, index) => {
          const dataset = readyDatasets.find(value => value.id === row.datasetId)
          const label = t('testing.evaluationDataset', { index: index + 1 })
          return <fieldset key={row.id} aria-label={label} className="space-y-3 rounded-lg border border-border p-4"><div className="flex items-end gap-3"><label className="block flex-1 space-y-2 text-xs"><span>{label}</span><Select className="w-full" aria-label={label} value={row.datasetId} placeholder={t('testing.chooseDataset')} onValueChange={datasetId => updateRow(row.id, { datasetId, workflowVersionIds: [] })} options={readyDatasets.filter(value => value.id === row.datasetId || !selections.some(other => other.datasetId === value.id)).map(value => ({ value: value.id, label: value.name }))} disabled={start.isPending} /></label>{selections.length > 1 && <Button type="button" variant="ghost" aria-label={t('testing.removeDataset', { index: index + 1 })} disabled={start.isPending} onClick={() => setSelections(current => current.filter(value => value.id !== row.id))}>{t('testing.removeDataset', { index: index + 1 })}</Button>}</div>
            {dataset && <div className="space-y-2"><p className="text-xs text-muted-foreground">{t('testing.chooseCompatibleVersions')}</p><div className="flex flex-wrap gap-4">{compatibleVersions(dataset).map(version => <label className="flex items-center gap-2 text-xs" key={version.id}><input type="checkbox" aria-label={`${dataset.name} · v${version.versionNumber}`} checked={row.workflowVersionIds.includes(version.id)} disabled={start.isPending} onChange={event => updateRow(row.id, { workflowVersionIds: event.target.checked ? [...row.workflowVersionIds, version.id] : row.workflowVersionIds.filter(id => id !== version.id) })} />v{version.versionNumber}</label>)}</div></div>}
          </fieldset>
        })}
        <Button type="button" variant="secondary" disabled={start.isPending || selections.length >= readyDatasets.length} onClick={() => setSelections(current => [...current, { id: crypto.randomUUID(), datasetId: '', workflowVersionIds: [] }])}>{t('testing.addDataset')}</Button>
        {readyDatasets.length === 0 && <p className="text-xs text-warning">{datasets.data?.length ? t('testing.datasetNotReady') : t('testing.noDatasets')}</p>}
      </section>}
      {error && <p role="alert" className="text-xs text-danger">{error.message}</p>}
    </div><div className="flex justify-end gap-2 border-t border-border p-4"><Button type="button" variant="ghost" disabled={start.isPending} onClick={onClose}>{t('common.cancel')}</Button><Button type="submit" disabled={!ready || loading || Boolean(queryError) || start.isPending}>{t('testing.start')}</Button></div>
  </form></DialogContent></Dialog>
}
