import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { apiFieldErrors, apiRequest, jsonBody } from '../../../shared/api/client'
import { Button } from '../../../shared/ui/button'
import { Dialog, DialogContent } from '../../../shared/ui/dialog'
import { Input } from '../../../shared/ui/input'
import { Select } from '../../../shared/ui/select'
import { CaseFields, caseName, useCaseValidation } from './case-form'
import { testingPath, type CaseDraft, type Dataset, type InvocationSample } from './types'

export function InvocationImport({ applicationId, sample, datasets, selectedId, onClose, onSaved }: { applicationId: string; sample: InvocationSample; datasets: Dataset[]; selectedId?: string | null; onClose: () => void; onSaved: (datasetId: string) => Promise<void> }) {
  const { t } = useTranslation()
  const matching = datasets.filter(value => value.status === 'active' && value.contract.fingerprint === sample.contract.fingerprint)
  const [repliesValid, setRepliesValid] = useState(true)
  const [target, setTarget] = useState(matching.find(value => value.id === selectedId)?.id ?? matching[0]?.id ?? 'new')
  const [name, setName] = useState(() => {
    const base = t('testing.invocationDatasetName')
    let next = base
    for (let suffix = 2; datasets.some(value => value.name === next); suffix++) next = `${base} ${suffix}`
    return next
  })
  const [draft, setDraft] = useState<CaseDraft>(() => {
    const input = sample.input && typeof sample.input === 'object' && !Array.isArray(sample.input) ? sample.input as Record<string, unknown> : {}
    return { waitInputs: sample.waitInputs, name: caseName({ name: '', input }, t('testing.newCase')), input, expectedOutput: sample.expectedOutput as Record<string, unknown> ?? {}, tags: [], sourceInvocationId: sample.invocationId, referenceConfirmed: false }
  })
  const [editing, setEditing] = useState(sample.validationErrors.length > 0)
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const [nameError, setNameError] = useState('')
  const validation = useCaseValidation(applicationId, sample.workflowVersionId, draft)
  useEffect(() => { if (validation.errors.length) setEditing(true) }, [validation.errors.length])
  const dataset = matching.find(value => value.id === target)
  const incomplete = validation.errors.length > 0
  const save = async () => {
    if (pending || validation.checking || validation.error || (target === 'new' && !name.trim())) return
    setPending(true); setError(''); setNameError('')
    try {
      const value = { ...draft, name: caseName(draft, t('testing.newCase')), referenceConfirmed: !incomplete }
      let id = dataset?.id
      if (target === 'new') {
        const created = await apiRequest<Dataset>(`${testingPath(applicationId)}/datasets`, { method: 'POST', body: jsonBody({ name: name.trim(), workflowVersionId: sample.workflowVersionId, initialCase: value }) })
        id = created.id
      } else if (dataset) {
        await apiRequest(`${testingPath(applicationId)}/datasets/${dataset.id}/cases`, { method: 'POST', body: jsonBody({ ...value, expectedRevision: dataset.revision }) })
      }
      if (id) await onSaved(id)
    } catch (cause) { setNameError(apiFieldErrors(cause).name ?? ''); setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setPending(false) }
  }
  return <Dialog open onOpenChange={open => { if (!open && !pending) onClose() }}><DialogContent title={t('testing.addFromInvocation')} description={t('testing.referenceImportHint')} className={editing ? 'left-auto right-0 top-0 h-dvh w-[720px] translate-x-0 translate-y-0 rounded-none' : 'w-[680px]'}>
    <div className="border-b border-border p-6"><h2 className="text-base font-semibold">{t('testing.addFromInvocation')}</h2><p className="mt-2 text-xs text-muted-foreground">{t('testing.referenceImportHint')}</p></div>
    <div className="space-y-5 p-6">
      <label className="block space-y-2 text-xs"><span>{t('testing.targetDataset')}</span><Select aria-label={t('testing.targetDataset')} value={target} onValueChange={setTarget} disabled={pending} options={[...matching.map(value => ({ value: value.id, label: value.name })), { value: 'new', label: t('testing.createDataset') }]} /></label>
      {target === 'new' && <label className="block space-y-2 text-xs"><span>{t('testing.datasetName')}</span><Input aria-label={t('testing.datasetName')} aria-invalid={Boolean(nameError)} disabled={pending} value={name} onChange={event => { setName(event.target.value); setNameError('') }} />{nameError && <span className="block text-danger">{nameError}</span>}</label>}
      {editing ? <CaseFields onValidityChange={setRepliesValid} contract={sample.contract} draft={draft} onChange={setDraft} disabled={pending} errors={validation.errors} /> : <>
        <div className="flex items-center justify-between gap-3"><p className="text-xs"><span className="text-muted-foreground">{t('testing.name')}：</span>{draft.name}</p><Button size="sm" variant="ghost" disabled={pending} onClick={() => setEditing(true)}>{t('testing.editCase')}</Button></div>
        <div className="grid grid-cols-2 gap-4">{[['input', draft.input], ['expectedOutput', draft.expectedOutput]].map(([key, value]) => <section key={key as string} className="min-w-0 rounded-lg bg-muted/30 p-3"><h3 className="mb-2 text-xs font-semibold">{key === 'input' ? t('testing.input') : t('testing.expectedOutput')}</h3><pre className="max-h-48 overflow-auto whitespace-pre-wrap break-words text-xs">{JSON.stringify(value, null, 2)}</pre></section>)}</div>
      </>}
      {validation.checking ? <p role="status" className="text-xs text-muted-foreground">{t('testing.validating')}</p> : !validation.error && !incomplete && <p role="status" className="text-xs text-success">{t('testing.validationPassed')}</p>}
      {!editing && incomplete && <Button size="sm" variant="secondary" onClick={() => setEditing(true)}>{t('testing.completeFields')}</Button>}
      {(error || validation.error) && <p role="alert" className="text-xs text-danger">{error || validation.error?.message}</p>}
    </div><div className="sticky bottom-0 flex justify-end gap-2 border-t border-border bg-surface p-4"><Button variant="ghost" disabled={pending} onClick={onClose}>{t('common.cancel')}</Button><Button disabled={!repliesValid || pending || validation.checking || Boolean(validation.error) || (target === 'new' ? !name.trim() : !dataset)} onClick={() => void save()}>{incomplete ? t('testing.saveDraft') : t('testing.confirmAdd')}</Button></div>
  </DialogContent></Dialog>
}
