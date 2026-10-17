import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { SchemaForm, schemaDefaults, validateSchemaValues } from '../../shared/components/schema-form'
import type { ArtifactReference, JsonSchema } from '../../shared/components/schema-form'
import { Button } from '../../shared/ui/button'
import { Dialog, DialogContent } from '../../shared/ui/dialog'

export function SessionInputDialog({ open, schema, pending, onClose, onCreate, onUpload }: {
  open: boolean; schema: JsonSchema; pending: boolean; onClose: () => void
  onCreate: (input: Record<string, unknown>) => void; onUpload: (file: File) => Promise<ArtifactReference>
}) {
  const { t } = useTranslation()
  const [input, setInput] = useState<Record<string, unknown>>({})
  const [errors, setErrors] = useState<Record<string, string>>({})
  useEffect(() => { if (open) { setInput(schemaDefaults(schema)); setErrors({}) } }, [open, schema])
  const submit = () => {
    const nextErrors = validateSchemaValues(schema, input)
    setErrors(nextErrors)
    if (!Object.keys(nextErrors).length) onCreate(input)
  }
  return <Dialog open={open} onOpenChange={value => { if (!value) onClose() }}><DialogContent title={t('applications.playground.sessionInputs')} description={t('applications.playground.sessionInputsDescription')} className="max-w-2xl"><div className="border-b border-border p-5"><h2 className="text-sm font-semibold">{t('applications.playground.sessionInputs')}</h2><p className="mt-2 text-xs text-muted-foreground">{t('applications.playground.sessionInputsDescription')}</p></div><div className="p-5"><SchemaForm schema={schema} values={input} onChange={setInput} onUpload={onUpload} disabled={pending} errors={errors} /></div><div className="flex justify-end gap-2 border-t border-border p-5"><Button onClick={onClose} variant="ghost">{t('common.cancel')}</Button><Button disabled={pending} onClick={submit}>{t('applications.playground.newSession')}</Button></div></DialogContent></Dialog>
}
