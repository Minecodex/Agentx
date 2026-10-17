import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Rocket } from 'lucide-react'
import { Link } from 'react-router-dom'
import type { WorkflowDeployment, WorkflowEnvironment, WorkflowVersionSummary } from '../../shared/api/types'
import { Dialog, DialogContent } from '../../shared/ui/dialog'
import { Button } from '../../shared/ui/button'
import { Select } from '../../shared/ui/select'

export type WorkflowPublishInput = { environmentId: string; workflowVersionId: string; contentSource: 'version' | 'draft'; expectedRevision?: number }
export function WorkflowPublishDialog({ version, environments, environmentsLoading, environmentsError, onRetryEnvironments, deployments, onClose, onSubmit }: {
  version: WorkflowVersionSummary; environments: WorkflowEnvironment[]; deployments: WorkflowDeployment[]; onClose: () => void
  environmentsLoading: boolean; environmentsError?: string; onRetryEnvironments: () => void
  onSubmit: (input: WorkflowPublishInput, key: string) => Promise<void>
}) {
  const { t } = useTranslation()
  const [selectedEnvironmentId, setEnvironmentId] = useState<string>()
  const environmentId = selectedEnvironmentId ?? environments[0]?.id ?? ''
  const targetEnvironment = environments.find(item => item.id === environmentId)
  const environmentsReady = !environmentsLoading && !environmentsError && environments.length > 0
  const [contentSource, setContentSource] = useState<'version' | 'draft'>(version.hasDraft ? 'draft' : 'version')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const operation = useRef({ payload: '', key: '' })
  const active = deployments.find(item => item.environmentId === environmentId && item.status === 'active')
  const noOp = version.publicationStatus === 'published' && contentSource === 'version' && active?.workflowVersionId === version.id
  const submit = async () => {
    if (!environmentsReady || !targetEnvironment || pending || noOp) return
    const input: WorkflowPublishInput = { environmentId, workflowVersionId: version.id, contentSource, ...((version.publicationStatus === 'unpublished' || contentSource === 'draft') ? { expectedRevision: version.draftRevision ?? undefined } : {}) }
    const payload = JSON.stringify(input)
    if (operation.current.payload !== payload) operation.current = { payload, key: crypto.randomUUID() }
    setPending(true); setError('')
    try { await onSubmit(input, operation.current.key); onClose() }
    catch (failure) { setError((failure as Error).message) }
    finally { setPending(false) }
  }
  return <Dialog open onOpenChange={open => { if (!open && !pending) onClose() }}><DialogContent title={t('workflows.publishToEnvironment')}>
    <div className="border-b border-border px-6 py-5"><h2 className="text-lg font-semibold">{t('workflows.publishToEnvironment')}</h2><p className="mt-1 text-xs text-muted-foreground">{t('workflows.versionCanvas.publishDescription')}</p></div>
    <div className="space-y-4 p-6">
      <label className="block text-xs"><span className="mb-2 block font-medium">{t('workflows.publishDialog.targetEnvironment')}</span><Select aria-label={t('workflows.publishDialog.targetEnvironment')} className="w-full" disabled={pending || !environmentsReady} value={targetEnvironment ? environmentId : ''} placeholder={t(environmentsLoading ? 'workflows.publishDialog.environmentsLoading' : 'workflows.publishDialog.noEnvironments')} onValueChange={setEnvironmentId} options={environments.map(item => ({ value: item.id, label: item.name }))} /></label>
      {environmentsLoading ? <p role="status" className="text-xs text-muted-foreground">{t('workflows.publishDialog.environmentsLoading')}</p> : environmentsError ? <div role="alert" className="space-y-2 text-xs text-danger"><p>{t('workflows.publishDialog.environmentLoadFailed')}</p><p>{environmentsError}</p><Button size="sm" variant="secondary" onClick={onRetryEnvironments}>{t('workflows.publishDialog.retryEnvironments')}</Button></div> : environments.length === 0 && <div role="status" className="space-y-3 rounded-lg border border-border bg-muted/30 p-4 text-xs text-muted-foreground"><p>{t('workflows.publishDialog.noEnvironmentsHint')}</p><div className="flex items-center gap-2"><Button asChild size="sm" variant="secondary"><Link to="/environments" target="_blank" rel="noreferrer">{t('workflows.manageEnvironments')}</Link></Button><Button size="sm" variant="ghost" onClick={onRetryEnvironments}>{t('workflows.publishDialog.retryEnvironments')}</Button></div></div>}
      <label className="block text-xs"><span className="mb-2 block font-medium">{t('workflows.publishDialog.releaseContent')}</span><Select aria-label={t('workflows.publishDialog.releaseContent')} className="w-full" disabled={pending} value={contentSource} onValueChange={value => setContentSource(value as 'version' | 'draft')} options={[
        { value: 'version', label: t(version.publicationStatus === 'published' ? 'workflows.versionCanvas.originalContent' : 'workflows.versionCanvas.unpublishedContent', { version: version.versionNumber, revision: version.publicationStatus === 'published' ? version.sourceRevision : version.draftRevision }) },
        ...(version.hasDraft ? [{ value: 'draft', label: t('workflows.versionCanvas.draftContent', { version: version.versionNumber, revision: version.draftRevision }) }] : []),
      ]} /></label>
      {targetEnvironment && <div className="rounded-lg border border-border bg-muted/30 p-4 text-xs leading-6"><p>{t('workflows.versionCanvas.environmentFrom', { version: active ? `v${active.versionNumber}` : t('workflows.versionBadges.notDeployed') })}</p><p>{t(contentSource === 'draft' ? 'workflows.versionCanvas.derivePreview' : version.publicationStatus === 'unpublished' ? 'workflows.versionCanvas.freezePreview' : 'workflows.versionCanvas.deployPreview', { version: version.versionNumber })}</p></div>}
      {noOp && <p role="status" className="text-xs text-warning">{t('workflows.publishDialog.noOp', { version: version.versionNumber, environment: environments.find(item => item.id === environmentId)?.name })}</p>}
      {error && <p role="alert" className="text-xs text-danger">{error}</p>}
    </div>
    <div className="flex justify-end gap-2 border-t border-border px-6 py-4"><Button variant="ghost" disabled={pending} onClick={onClose}>{t('common.cancel')}</Button><Button disabled={pending || noOp || !environmentsReady || !targetEnvironment} onClick={() => void submit()}><Rocket className="size-4" />{t(pending ? 'workflows.publishDialog.publishing' : 'workflows.publishToEnvironment')}</Button></div>
  </DialogContent></Dialog>
}
