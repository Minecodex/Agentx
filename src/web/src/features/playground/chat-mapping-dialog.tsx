import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import type { JsonSchema } from '../../shared/components/schema-form'
import { Button } from '../../shared/ui/button'
import { Dialog, DialogContent } from '../../shared/ui/dialog'
import { Select } from '../../shared/ui/select'
import type { ChatMapping, PlaygroundConfig } from './playground-types'

export function ChatMappingDialog({ open, config, inputSchema, outputSchema, saving, canManage, onOpenChange, onSave, onClear }: {
  open: boolean
  config?: PlaygroundConfig
  inputSchema: JsonSchema
  outputSchema: JsonSchema
  saving: boolean
  canManage: boolean
  onOpenChange: (open: boolean) => void
  onSave: (mapping: ChatMapping) => void
  onClear: () => void
}) {
  const { t } = useTranslation()
  const choices = useMemo(() => mappingChoices(inputSchema, outputSchema), [inputSchema, outputSchema])
  const [draft, setDraft] = useState<ChatMapping>(() => config?.mapping ?? emptyMapping(choices))
  useEffect(() => {
    if (open) setDraft(config?.mapping ?? emptyMapping(choices))
  }, [open, config?.version, config?.mapping, choices])
  const valid = Boolean(draft.questionInput && draft.answerOutput)
  return <Dialog onOpenChange={onOpenChange} open={open}><DialogContent className="w-[min(620px,calc(100vw-32px))]" description={t('applications.playground.mappingDialog.description')} title={t('applications.playground.mappingDialog.title')}><div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('applications.playground.mappingDialog.title')}</h2><p className="mt-1 text-xs text-muted-foreground">{t('applications.playground.mappingDialog.sharedHint')}</p></div><div className="grid gap-4 p-5 sm:grid-cols-2"><MappingSelect label={t('applications.playground.mappingDialog.questionInput')} options={choices.question} required value={draft.questionInput} onChange={(questionInput) => setDraft((value) => ({ ...value, questionInput }))} /><MappingSelect label={t('applications.playground.mappingDialog.fileInput')} options={choices.files} value={draft.fileInput ?? ''} onChange={(fileInput) => setDraft((value) => ({ ...value, fileInput: fileInput || null }))} /><MappingSelect label={t('applications.playground.mappingDialog.answerOutput')} options={choices.answer} required value={draft.answerOutput} onChange={(answerOutput) => setDraft((value) => ({ ...value, answerOutput }))} /><MappingSelect label={t('applications.playground.mappingDialog.answerFilesOutput')} options={choices.answerFiles} value={draft.answerFilesOutput ?? ''} onChange={(answerFilesOutput) => setDraft((value) => ({ ...value, answerFilesOutput: answerFilesOutput || null }))} />{config?.publishStatus === 'failed' && <p className="sm:col-span-2 rounded-md bg-danger/10 p-3 text-xs text-danger">{config.errorCode}: {config.errorMessage}</p>}{!canManage && <p className="sm:col-span-2 rounded-md bg-warning/10 p-3 text-xs text-warning">{t('applications.playground.mappingDialog.permissionHint')}</p>}</div><div className="flex justify-between border-t border-border px-5 py-4"><Button disabled={!canManage || saving || !config?.mapping} onClick={onClear} variant="ghost">{t('applications.playground.mappingDialog.clear')}</Button><div className="flex gap-2"><Button onClick={() => onOpenChange(false)} variant="ghost">{t('common.cancel')}</Button><Button disabled={!canManage || saving || !valid} onClick={() => onSave(draft)}>{saving ? t('applications.playground.mappingDialog.publishing') : t('applications.playground.mappingDialog.save')}</Button></div></div></DialogContent></Dialog>
}

function MappingSelect({ label, options, value, onChange, required = false }: { label: string; options: Array<{ value: string; label: string }>; value: string; onChange: (value: string) => void; required?: boolean }) {
  const { t } = useTranslation()
  return <label className="text-xs"><span className="mb-1.5 block font-medium">{label}{required && <span className="ml-1 text-danger">*</span>}</span><Select className="w-full" onValueChange={(next) => onChange(next === '__none__' ? '' : next)} options={required ? options : [{ value: '__none__', label: t('applications.playground.mappingDialog.noMapping') }, ...options]} placeholder={t('applications.playground.mappingDialog.selectCompatible')} value={value || (required ? '' : '__none__')} /></label>
}

function mappingChoices(input: JsonSchema, output: JsonSchema) {
  const entries = (schema: JsonSchema) => Object.entries(schema.properties ?? {}).map(([value, field]) => ({ value, label: `${field.title?.trim() || value} · ${field['x-agentx-artifact-array'] ? 'artifact[]' : field['x-agentx-artifact'] ? 'artifact' : field.type ?? 'unknown'}`, field }))
  const inputs = entries(input)
  const outputs = entries(output)
  return {
    question: inputs.filter(({ field }) => field.type === 'string' && !field['x-agentx-artifact']).map(stripField),
    files: inputs.filter(({ field }) => field['x-agentx-artifact']).map(stripField),
    answer: outputs.filter(({ field }) => field.type === 'string' && !field['x-agentx-sensitive']).map(stripField),
    answerFiles: outputs.filter(({ field }) => field['x-agentx-artifact']).map(stripField),
  }
}

function stripField({ value, label }: { value: string; label: string }) { return { value, label } }
function emptyMapping(choices: ReturnType<typeof mappingChoices>): ChatMapping { return { questionInput: choices.question[0]?.value ?? '', fileInput: null, answerOutput: choices.answer[0]?.value ?? '', answerFilesOutput: null } }
