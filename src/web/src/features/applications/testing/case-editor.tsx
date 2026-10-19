import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { apiRequest, jsonBody } from '../../../shared/api/client'
import { schemaDefaults, type JsonSchema } from '../../../shared/components/schema-form'
import { Button } from '../../../shared/ui/button'
import { Dialog, DialogContent } from '../../../shared/ui/dialog'
import { CaseFields, caseName, useCaseValidation } from './case-form'
import { testingPath, type CaseDraft, type Dataset, type TestCase } from './types'

export function CaseEditor({ applicationId, dataset, selected, onClose, onSaved }: { applicationId: string; dataset: Dataset; selected?: TestCase; onClose: () => void; onSaved: () => Promise<void> }) {
  const { t } = useTranslation()
  const [repliesValid, setRepliesValid] = useState(true)
  const [draft, setDraft] = useState<CaseDraft>(() => selected ? { waitInputs: selected.waitInputs, name: selected.name, input: selected.input as Record<string, unknown>, expectedOutput: selected.expectedOutput as Record<string, unknown>, tags: selected.tags, pendingFields: selected.pendingFields, sourceInvocationId: selected.sourceInvocationId, referenceConfirmed: selected.referenceConfirmed } : { name: '', input: schemaDefaults(dataset.contract.inputSchema as JsonSchema), expectedOutput: schemaDefaults(dataset.contract.outputSchema as JsonSchema), tags: [], referenceConfirmed: true })
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const validation = useCaseValidation(applicationId, dataset.sourceWorkflowVersionId, draft)
  const incomplete = validation.errors.length > 0
  const save = async () => {
    if (!repliesValid || validation.checking || validation.error) return
    setPending(true); setError('')
    try {
      await apiRequest(`${testingPath(applicationId)}/datasets/${dataset.id}/cases${selected ? `/${selected.id}` : ''}`, { method: selected ? 'PATCH' : 'POST', body: jsonBody({ ...draft, name: caseName(draft, t('testing.newCase')), referenceConfirmed: !incomplete, expectedRevision: dataset.revision, version: selected?.version ?? null }) })
      await onSaved(); onClose()
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) } finally { setPending(false) }
  }
  return <Dialog open onOpenChange={open => { if (!open && !pending) onClose() }}><DialogContent title={selected ? t('testing.editCase') : t('testing.addCase')} description={t('testing.draftHint')} className="left-auto right-0 top-0 h-dvh w-[720px] translate-x-0 translate-y-0 rounded-none">
    <div className="border-b border-border p-6"><h2 className="text-base font-semibold">{selected ? t('testing.editCase') : t('testing.addCase')}</h2><p className="mt-1 text-xs text-muted-foreground">{t('testing.draftHint')}</p></div>
    <div className="space-y-6 p-6"><CaseFields onValidityChange={setRepliesValid} contract={dataset.contract} draft={draft} onChange={setDraft} disabled={pending} errors={validation.errors} />
      {!draft.referenceConfirmed && <p className="text-xs text-muted-foreground">{t('testing.referenceSaveHint')}</p>}
      {validation.checking && <p role="status" className="text-xs text-muted-foreground">{t('testing.validating')}</p>}
      {(error || validation.error) && <p role="alert" className="text-xs text-danger">{error || validation.error?.message}</p>}
    </div><div className="sticky bottom-0 flex justify-end gap-2 border-t border-border bg-surface p-4"><Button variant="ghost" disabled={pending} onClick={onClose}>{t('common.cancel')}</Button><Button disabled={!repliesValid || pending || validation.checking || Boolean(validation.error)} onClick={() => void save()}>{incomplete ? t('testing.saveDraft') : !draft.referenceConfirmed ? t('testing.confirmSave') : t('common.save')}</Button></div>
  </DialogContent></Dialog>
}
