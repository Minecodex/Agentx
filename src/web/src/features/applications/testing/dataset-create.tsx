import { useEffect, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { apiFieldErrors, apiRequest, jsonBody } from '../../../shared/api/client'
import { Button } from '../../../shared/ui/button'
import { Dialog, DialogContent } from '../../../shared/ui/dialog'
import { Input } from '../../../shared/ui/input'
import { Select } from '../../../shared/ui/select'
import { CaseValidationMessages } from './case-form'
import { testingPath, type Dataset, type ContractVersion, type CaseFieldError } from './types'

export function DatasetCreate({ applicationId, versions, initialVersion, source, onClose, onCreated }: { applicationId: string; versions: ContractVersion[]; initialVersion?: string; source?: Dataset; onClose: () => void; onCreated: (value: Dataset) => Promise<void> }) {
  const { t } = useTranslation()
  const [name, setName] = useState(source ? `${source.name} ${t('testing.copySuffix')}` : '')
  const [version, setVersion] = useState(initialVersion ?? versions[0]?.id ?? '')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({})
  const nameInput = useRef<HTMLInputElement>(null)
  const target = versions.find(value => value.id === version)
  const preview = useQuery({ queryKey: ['test-copy-preview', applicationId, source?.id, source?.revision, version], enabled: Boolean(source && version), queryFn: () => apiRequest<{ caseCount: number; incompleteCount: number; contractChanged: boolean; cases: { id: string; name: string; validationErrors: CaseFieldError[] }[] }>(`${testingPath(applicationId)}/datasets/${source!.id}/copy-preview`, { method: 'POST', body: jsonBody({ name, description: source!.description, workflowVersionId: version, expectedRevision: source!.revision }) }) })
  useEffect(() => { setError('') }, [version])
  useEffect(() => { if (!pending && fieldErrors.name) nameInput.current?.focus() }, [pending, fieldErrors.name])
  const save = async () => {
    if (!name.trim() || !version) return
    setPending(true); setError(''); setFieldErrors({})
    try {
      const value = await apiRequest<Dataset>(`${testingPath(applicationId)}/datasets${source ? `/${source.id}/copies` : ''}`, { method: 'POST', body: jsonBody({ name: name.trim(), description: source?.description ?? null, workflowVersionId: version, ...(source ? { expectedRevision: source.revision } : {}) }) })
      await onCreated(value); onClose()
    } catch (cause) { setFieldErrors(apiFieldErrors(cause)); setError(cause instanceof Error ? cause.message : String(cause)) } finally { setPending(false) }
  }
  return <Dialog open onOpenChange={open => { if (!open && !pending) onClose() }}><DialogContent title={source ? t('testing.copyDataset') : t('testing.createDataset')} description={t('testing.fixedContractHint')}><form onSubmit={event => { event.preventDefault(); void save() }}>
    <div className="border-b border-border p-6"><h2 className="text-base font-semibold">{source ? t('testing.copyDataset') : t('testing.createDataset')}</h2><p className="mt-2 text-xs text-muted-foreground">{t('testing.fixedContractHint')}</p></div>
    <div className="space-y-5 p-6"><label className="block space-y-2 text-xs"><span>{t('testing.contractVersion')}</span><Select aria-label={t('testing.contractVersion')} value={version} onValueChange={setVersion} options={versions.map(value => ({ value: value.id, label: `v${value.versionNumber}` }))} disabled={pending} /></label>
      <label className="block space-y-2 text-xs"><span>{t('testing.datasetName')}</span><Input ref={nameInput} aria-invalid={Boolean(fieldErrors.name)} aria-label={t('testing.datasetName')} required value={name} disabled={pending} onChange={event => setName(event.target.value)} />{fieldErrors.name && <span className="block text-danger">{fieldErrors.name}</span>}</label>
      {versions.length === 0 && <p className="text-xs text-warning">{t('testing.noVersions')}</p>}
      {source && <div className="rounded-lg bg-muted/40 p-4 text-xs">{preview.isFetching ? t('common.loading') : preview.error ? <span role="alert">{preview.error.message}</span> : preview.data && target ? <div><p>{t('testing.copyPreview', preview.data)}</p>{preview.data.contractChanged && <p className="mt-2 text-warning">{t('testing.copyContractChanged', { count: preview.data.caseCount })}</p>}<ul className="mt-3 max-h-48 space-y-3 overflow-auto">{preview.data.cases.map(value => ({ ...value, validationErrors: value.validationErrors.filter(error => error.code !== 'REFERENCE_UNCONFIRMED') })).filter(value => value.validationErrors.length).map(value => <li key={value.id}><strong className="mb-1 block">{value.name}</strong><CaseValidationMessages contract={target.contract} errors={value.validationErrors} /></li>)}</ul></div> : null}</div>}
      {error && <p role="alert" className="text-xs text-danger">{error}</p>}
    </div><div className="flex justify-end gap-2 border-t border-border p-4"><Button type="button" variant="ghost" disabled={pending} onClick={onClose}>{t('common.cancel')}</Button><Button type="submit" disabled={pending || !name.trim() || !version || Boolean(source && (!preview.data || preview.isFetching || preview.error))}>{t('common.create')}</Button></div>
  </form></DialogContent></Dialog>
}
