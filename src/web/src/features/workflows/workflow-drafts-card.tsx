import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { apiRequest, jsonBody } from '../../shared/api/client'
import type { WorkflowDraft, WorkflowVersion } from '../../shared/api/types'
import { EntityFormDialog } from '../../shared/components/entity-form-dialog'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { Select } from '../../shared/ui/select'
import { useToast } from '../../shared/ui/toast'

export function WorkflowDraftsCard({ workflowId, drafts, versions, canEdit, selectedId, onSelect }: { workflowId: string; drafts: WorkflowDraft[]; versions: WorkflowVersion[]; canEdit: boolean; selectedId: string; onSelect: (id: string) => void }) {
  const { t } = useTranslation()
  const client = useQueryClient()
  const { showToast } = useToast()
  const [open, setOpen] = useState(false)
  const refresh = () => Promise.all([client.invalidateQueries({queryKey: ['workflow-drafts', workflowId]}), client.invalidateQueries({queryKey: ['workflow', workflowId]})])
  const update = useMutation({
    mutationFn: (draft: WorkflowDraft) => apiRequest(`/workflows/${workflowId}/drafts/${draft.id}/metadata`, {method:'PATCH', body: jsonBody({name:draft.name,status:draft.status === 'active' ? 'archived' : 'active',expectedRevision:draft.revision})}),
    onSuccess: async () => { await refresh() }, onError: (error: Error) => showToast(error.message),
  })
  return <Card className="overflow-hidden">
    <div className="flex items-center gap-3 border-b border-border p-4"><h2 className="flex-1 text-sm font-semibold">{t('workflows.drafts')}</h2>{canEdit && <Button onClick={() => setOpen(true)} size="sm" variant="secondary">{t('workflows.newDraft')}</Button>}</div>
    <div className="space-y-3 p-4">
      <Select aria-label={t('workflows.selectedDraft')} value={selectedId} onValueChange={onSelect} options={[{value:'',label:t('workflows.selectDraft')},...drafts.filter(d => d.status === 'active').map(d => ({value:d.id,label:`${d.name} · r${d.revision}`}))]} />
      {drafts.map(draft => <div className="flex items-center gap-2 border-t border-border pt-3 text-xs" key={draft.id}>
        <div className="min-w-0 flex-1"><strong>{draft.name}</strong><p className="mt-1 text-muted-foreground">r{draft.revision} · {draft.baseVersionId ? t('workflows.fromVersion', {version: versions.find(v => v.id === draft.baseVersionId)?.versionNumber ?? draft.baseVersionId}) : t('workflows.emptyDraft')} · {draft.status === 'archived' ? t('workflows.archived') : t('common.active')}</p></div>
        <Button asChild size="sm" variant="ghost"><Link to={`/workflows/${workflowId}/drafts/${draft.id}/editor`}>{t('workflows.openEditor')}</Link></Button>
        {canEdit && <Button disabled={update.isPending} onClick={() => update.mutate(draft)} size="sm" variant="ghost">{t(draft.status === 'active' ? 'workflows.archiveDraft' : 'workflows.restoreDraft')}</Button>}
      </div>)}
    </div>
    {open && <EntityFormDialog open onClose={() => setOpen(false)} title={t('workflows.newDraft')} cancelLabel={t('common.cancel')} submitLabel={t('common.create')} fields={[{name:'name',label:t('workflows.draftName'),required:true},{name:'baseVersionId',label:t('workflows.baseVersion'),type:'select',options:[{value:'',label:t('workflows.emptyDraft')},...versions.map(v => ({value:v.id,label:`v${v.versionNumber}`}))]}]} onSubmit={async values => {const draft = await apiRequest<WorkflowDraft>(`/workflows/${workflowId}/drafts`, {method:'POST',body:jsonBody({name:values.name,baseVersionId:values.baseVersionId || null})});await refresh();onSelect(draft.id)}} />}
  </Card>
}
