import { WaitInputsEditor } from '../../../shared/components/wait-inputs-editor'
import { useQuery } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import type { TFunction } from 'i18next'
import { useTranslation } from 'react-i18next'
import { apiRequest, gatewayRequest, jsonBody } from '../../../shared/api/client'
import { SchemaFormWorkspace, type ArtifactReference, type JsonSchema } from '../../../shared/components/schema-form'
import { Button } from '../../../shared/ui/button'
import { Input } from '../../../shared/ui/input'
import { testingPath, type CaseDraft, type CaseFieldError, type Dataset } from './types'

export async function uploadTestArtifact(file: File): Promise<ArtifactReference> {
  const body = new FormData(); body.append('file', file)
  const result = await gatewayRequest<ArtifactReference>('/artifacts', { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body })
  return { ...result, fileName: file.name }
}

export function caseName(draft: Pick<CaseDraft, 'name' | 'input'>, fallback: string) {
  return draft.name.trim() || Object.values(draft.input).find(value => typeof value === 'string' && value.trim())?.toString().slice(0, 100) || fallback
}

export function useCaseValidation(applicationId: string, workflowVersionId: string, draft: CaseDraft) {
  const body = jsonBody({ ...draft, workflowVersionId })
  const [request, setRequest] = useState(body)
  useEffect(() => { const timer = setTimeout(() => setRequest(body), 180); return () => clearTimeout(timer) }, [body])
  const query = useQuery({
    queryKey: ['test-case-validation', applicationId, request], retry: false,
    queryFn: ({ signal }) => apiRequest<{ validationErrors: CaseFieldError[] }>(`${testingPath(applicationId)}/test-case-validation`, { method: 'POST', body: request, signal }),
    staleTime: 0,
  })
  const current = body === request
  return { errors: current ? query.data?.validationErrors ?? [] : [], checking: !current || query.isFetching || query.isPending, error: current ? query.error : null }
}

const errorKeys = {
  REQUIRED: 'testing.validation.required', INVALID_VALUE: 'testing.validation.invalidValue',
  INVALID_CONTRACT: 'testing.validation.invalidContract', PENDING_FIELD: 'testing.validation.pendingField',
  REFERENCE_UNCONFIRMED: 'testing.validation.referenceUnconfirmed', ARTIFACT_UNAVAILABLE: 'testing.validation.artifactUnavailable',
} as const

export function caseFieldLabel(contract: Dataset['contract'], field: string, t: TFunction) {
  const [root, ...segments] = field.split('/')
  let schema = (root === 'input' ? contract.inputSchema : contract.outputSchema) as JsonSchema
  const labels = [root === 'input' ? t('testing.input') : t('testing.expectedOutput')]
  for (const segment of segments) {
    const key = segment.replaceAll('~1', '/').replaceAll('~0', '~')
    schema = schema.properties?.[key] ?? schema.items ?? {}
    labels.push(schema.title || key)
  }
  return labels.join(' · ')
}

export function CaseValidationMessages({ contract, errors }: { contract: Dataset['contract']; errors: CaseFieldError[] }) {
  const { t } = useTranslation()
  if (!errors.length) return null
  return <div aria-live="polite" className="space-y-1 rounded-lg bg-warning/10 p-3 text-xs">{errors.map((error, index) => <p key={`${error.field}-${index}`}>{caseFieldLabel(contract, error.field, t)}：{t(errorKeys[error.code])}</p>)}</div>
}

export function CaseFields({ contract, draft, onChange, disabled, errors, onValidityChange }: { contract: Dataset['contract']; draft: CaseDraft; onChange: (value: CaseDraft) => void; disabled: boolean; errors: CaseFieldError[]; onValidityChange?: (valid: boolean) => void }) {
  const { t } = useTranslation()
  const pending = new Set(errors.filter(error => error.code === 'PENDING_FIELD').map(error => error.field))
  return <>
    <label className="block space-y-2 text-xs"><span>{t('testing.name')}</span><Input aria-label={t('testing.name')} value={draft.name} disabled={disabled} onChange={event => onChange({ ...draft, name: event.target.value })} placeholder={t('testing.automaticName')} /></label>
    <section><h3 className="mb-3 text-sm font-semibold">{t('testing.input')}</h3><SchemaFormWorkspace schema={contract.inputSchema as JsonSchema} value={draft.input} onChange={input => onChange({ ...draft, input })} onUpload={uploadTestArtifact} disabled={disabled} /></section>
    <WaitInputsEditor onValidityChange={onValidityChange} nodes={contract.waitInputNodes ?? []} value={draft.waitInputs ?? {}} onChange={waitInputs => onChange({ ...draft, waitInputs })} disabled={disabled} />
    <section><h3 className="mb-3 text-sm font-semibold">{t('testing.expectedOutput')}</h3><SchemaFormWorkspace schema={contract.outputSchema as JsonSchema} value={draft.expectedOutput} onChange={expectedOutput => onChange({ ...draft, expectedOutput })} onUpload={uploadTestArtifact} disabled={disabled} /></section>
    <label className="block space-y-2 text-xs"><span>{t('testing.tags')}</span><Input aria-label={t('testing.tags')} value={draft.tags.join(', ')} disabled={disabled} onChange={event => onChange({ ...draft, tags: event.target.value.split(',').map(value => value.trim()).filter(Boolean) })} /></label>
    {pending.size > 0 && <div className="space-y-2 rounded-lg bg-warning/10 p-3 text-xs"><p>{t('testing.pendingHint')}</p>{[...pending].map(field => <div key={field} className="flex items-center justify-between gap-3"><span>{caseFieldLabel(contract, field, t)}</span><Button size="sm" variant="ghost" disabled={disabled} onClick={() => onChange({ ...draft, pendingFields: draft.pendingFields?.filter(value => value !== field) })}>{t('testing.confirmUnused')}</Button></div>)}</div>}
    <CaseValidationMessages contract={contract} errors={errors.filter(error => error.code !== 'PENDING_FIELD')} />
  </>
}
