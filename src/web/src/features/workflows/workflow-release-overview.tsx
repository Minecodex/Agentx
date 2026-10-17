import { Table, TableHead, TableCell, TableContainer } from '../../shared/ui/table'
import { Link } from 'react-router-dom'
import { History, MoreHorizontal, Rocket } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { WorkflowVersionSummary, WorkflowDeployment, WorkflowEnvironment } from '../../shared/api/types'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { localizedValue } from '../../shared/lib/localized-value'
import { Badge } from '../../shared/ui/badge'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { Dialog, DialogContent } from '../../shared/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '../../shared/ui/dropdown-menu'
type HistoryDialogProps = {
  open: boolean
  deployments: WorkflowDeployment[]
  canRollback: boolean
  rollingBack: boolean
  onClose: () => void
  onRollback: (deployment: WorkflowDeployment) => void
}

export function WorkflowReleaseHistoryDialog({ open, deployments, canRollback, rollingBack, onClose, onRollback }: HistoryDialogProps) {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  return <Dialog onOpenChange={(value) => !value && onClose()} open={open}><DialogContent title={t('workflows.releaseHistory')}>
    <div className="border-b border-border px-6 py-5"><h2 className="text-lg font-semibold">{t('workflows.releaseHistory')}</h2><p className="mt-1 text-xs text-muted-foreground">{t('workflows.releaseHistoryDescription')}</p></div>
    <div className="max-h-[56vh] divide-y divide-border overflow-y-auto px-6">{deployments.map((deployment) => <div className="flex items-center gap-3 py-4" key={deployment.id}><span className="grid size-9 shrink-0 place-items-center rounded-lg bg-muted text-xs font-semibold">{deployment.environmentName.slice(0, 1).toUpperCase()}</span><div className="min-w-0 flex-1"><div className="flex flex-wrap items-center gap-2"><strong className="text-xs">{deployment.environmentName} · v{deployment.versionNumber}</strong><Badge tone={deployment.status === 'active' ? 'success' : 'neutral'}>{localizedValue(t, 'common', deployment.status)}</Badge></div><p className="mt-1.5 text-[10px] text-muted-foreground">{localizedValue(t, 'workflows.deploymentSources', deployment.source)} · #{deployment.sequenceNumber} · {formatDateTime(deployment.createdAt)}</p></div>{canRollback && deployment.status !== 'active' && <Button disabled={rollingBack} onClick={() => onRollback(deployment)} size="sm" variant="ghost">{t('workflows.rollbackToVersion', { version: deployment.versionNumber })}</Button>}</div>)}{deployments.length === 0 && <Empty />}</div>
    <div className="flex justify-end border-t border-border bg-muted/30 px-6 py-4"><Button onClick={onClose} variant="secondary">{t('common.close')}</Button></div>
  </DialogContent></Dialog>
}

function Empty() {
  const { t } = useTranslation()
  return <p className="py-5 text-xs text-muted-foreground">{t('workflows.noData')}</p>
}

type VersionListProps = {
  versions: WorkflowVersionSummary[]; deployments: WorkflowDeployment[]; canEdit: boolean; canPublish: boolean; canArchive: boolean
  workflowId: string; onPublish: (version: WorkflowVersionSummary) => void; onArchive: (version: WorkflowVersionSummary) => void
}
export function WorkflowVersionsCard({ versions, deployments, canEdit, canPublish, canArchive, workflowId, onPublish, onArchive }: VersionListProps) {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  return <Card className="overflow-hidden"><div className="border-b border-border px-5 py-4"><h2 className="text-sm font-semibold">{t('workflows.versions')}</h2></div>
    <TableContainer><Table><thead className="border-b border-border bg-muted/30 text-muted-foreground"><tr>{['version','state','draft','base','updated','actions'].map(key => <TableHead className="font-medium" key={key}>{t(`workflows.versionCanvas.columns.${key}`)}</TableHead>)}</tr></thead><tbody className="divide-y divide-border">{versions.map(version => <tr key={version.id} data-testid={`workflow-version-${version.versionNumber}`}>
      <TableCell className="px-5 py-4 font-semibold">v{version.versionNumber}<div className="mt-1 flex flex-wrap gap-1">{deployments.filter(item => item.status === 'active' && item.workflowVersionId === version.id).map(item => <Badge key={item.environmentId} tone="success">{item.environmentName}</Badge>)}</div></TableCell>
      <TableCell className="px-5 py-4"><div className="flex gap-1"><Badge tone={version.publicationStatus === 'published' ? 'success' : 'neutral'}>{t(`workflows.versionCanvas.${version.publicationStatus}`)}</Badge>{version.archived && <Badge>{t('workflows.archived')}</Badge>}</div></TableCell>
      <TableCell className="px-5 py-4">{version.publicationStatus === 'unpublished' ? '—' : <Badge tone={version.hasDraft ? 'warning' : 'neutral'}>{t(version.hasDraft ? 'workflows.versionCanvas.hasDraft' : 'workflows.versionCanvas.noDraft')}</Badge>}</TableCell>
      <TableCell className="px-5 py-4">{version.baseVersionId ? <Link className="text-primary hover:underline" to={`/workflows/${workflowId}/versions/${version.baseVersionId}`}>v{version.baseVersionNumber}</Link> : '—'}</TableCell>
      <TableCell className="px-5 py-4"><p>{formatDateTime(version.updatedAt)}</p><p className="mt-1 text-muted-foreground">{version.updatedByName}</p></TableCell>
      <TableCell className="px-5 py-4"><div className="flex items-center gap-1 whitespace-nowrap"><Button asChild size="sm" variant="ghost"><Link to={`/workflows/${workflowId}/versions/${version.id}`}>{t('workflows.details')}</Link></Button>{canEdit && (version.archived ? <Button size="sm" variant="ghost" disabled title={t('workflows.versionCanvas.archivedHint')}>{t('common.edit')}</Button> : <Button asChild size="sm" variant="ghost"><Link to={`/workflows/${workflowId}/versions/${version.id}/editor`}>{t('common.edit')}</Link></Button>)}{canPublish && <Button size="sm" variant="ghost" onClick={() => onPublish(version)}>{t('workflows.publish')}</Button>}
        <DropdownMenu><DropdownMenuTrigger asChild><Button size="icon" variant="ghost" aria-label={t('workflows.versionCanvas.more', { version: version.versionNumber })}><MoreHorizontal className="size-4" /></Button></DropdownMenuTrigger><DropdownMenuContent align="end"><DropdownMenuItem asChild><Link to={`/executions?workflowIds=${workflowId}&workflowVersionIds=${version.id}`}>{t('workflows.versionCanvas.executions')}</Link></DropdownMenuItem>{canArchive && <DropdownMenuItem onSelect={() => onArchive(version)}>{t(version.archived ? 'workflows.versionCanvas.unarchive' : 'workflows.archive')}</DropdownMenuItem>}</DropdownMenuContent></DropdownMenu>
      </div></TableCell>
    </tr>)}</tbody></Table></TableContainer>{versions.length === 0 && <div className="px-5"><Empty /></div>}
  </Card>
}

export function WorkflowEnvironmentsCard({ environments, deployments, onHistory }: { environments: WorkflowEnvironment[]; deployments: WorkflowDeployment[]; onHistory: () => void }) {
  const { t } = useTranslation()
  return <Card className="overflow-hidden"><div className="flex items-center gap-2 border-b border-border px-5 py-4"><Rocket className="size-4 text-primary" /><h2 className="flex-1 text-sm font-semibold">{t('workflows.environmentStatus')}</h2><Button size="sm" variant="ghost" onClick={onHistory}><History className="size-4" />{t('workflows.viewReleaseHistory')}</Button></div><div className="grid grid-cols-3 gap-4 p-5">{environments.map(environment => {
    const current = deployments.find(item => item.environmentId === environment.id && item.status === 'active')
    return <div key={environment.id} className="rounded-lg border border-border px-4 py-3"><p className="text-xs text-muted-foreground">{environment.name}</p><div className="mt-2 text-sm font-semibold">{current ? <Link className="text-primary hover:underline" to={`/workflows/${current.workflowId}/versions/${current.workflowVersionId}`}>v{current.versionNumber}</Link> : t('workflows.versionBadges.notDeployed')}</div></div>
  })}</div>{environments.length === 0 && <div className="px-5"><Empty /></div>}</Card>
}
