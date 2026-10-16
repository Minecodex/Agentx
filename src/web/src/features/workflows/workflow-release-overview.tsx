import { Link } from "react-router-dom"
import { AppWindow, Check, ChevronRight, CircleDot, GitCommitHorizontal, History, Play, Plus, Rocket } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import type { Workflow, WorkflowDeployment, WorkflowEnvironment, WorkflowVersion } from '../../shared/api/types'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { localizedValue } from '../../shared/lib/localized-value'
import { Badge } from '../../shared/ui/badge'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { Dialog, DialogContent } from '../../shared/ui/dialog'

type ReleaseOverviewProps = {
  workflow: Workflow
  versions: WorkflowVersion[]
  deployments: WorkflowDeployment[]
  primaryEnvironment?: WorkflowEnvironment
  primaryDeployment?: WorkflowDeployment
}

export function WorkflowReleaseOverview({ workflow, versions, deployments, primaryEnvironment, primaryDeployment }: ReleaseOverviewProps) {
  const { t } = useTranslation()
  const latest = versions[0]
  const dirty = !latest
  const steps = [
    { icon: Check, label: t('workflows.deliveryPath.draft'), detail: t('workflows.draftCount', { count: workflow.draftCount }), state: 'done' },
    { icon: GitCommitHorizontal, label: t('workflows.deliveryPath.version'), detail: latest ? t('workflows.deliveryPath.latestVersion', { version: latest.versionNumber }) : t('workflows.deliveryPath.noVersion'), state: dirty ? 'current' : 'done' },
    { icon: Rocket, label: t('workflows.deliveryPath.environment'), detail: primaryDeployment ? t('workflows.deliveryPath.environmentActive', { environment: primaryDeployment.environmentName, version: primaryDeployment.versionNumber }) : t('workflows.deliveryPath.notDeployed'), state: dirty ? 'pending' : primaryDeployment ? 'current' : 'current' },
    { icon: AppWindow, label: t('workflows.deliveryPath.application'), detail: t('workflows.deliveryPath.applicationContract'), state: 'pending' },
  ]
  return <>
    <Card className="mt-6 grid gap-2 p-3 sm:grid-cols-2 xl:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto_minmax(0,1fr)_auto_minmax(0,1fr)] xl:items-center">
      {steps.map((step, index) => <div className="contents" key={step.label}>
        <div className="flex min-w-0 items-center gap-3 rounded-lg px-2 py-2">
          <span className={`grid size-8 shrink-0 place-items-center rounded-full ${step.state === 'done' ? 'bg-success/10 text-success' : step.state === 'current' ? 'bg-primary text-primary-foreground' : 'bg-muted text-muted-foreground'}`}><step.icon className="size-3.5" /></span>
          <span className="min-w-0"><strong className="block text-xs font-semibold">{step.label}</strong><span className="mt-1 block truncate text-[10px] text-muted-foreground">{step.detail}</span></span>
        </div>
        {index < steps.length - 1 && <ChevronRight className="hidden size-3.5 text-muted-foreground/60 xl:block" />}
      </div>)}
    </Card>
    <div className="mt-4 grid gap-4 md:grid-cols-3">
      <StateCard detail={t('workflows.selectDraft')} label={t('workflows.releaseState.currentDraft')} tone={dirty ? 'warning' : 'success'} value={String(workflow.draftCount)} />
      <StateCard detail={latest ? t('workflows.releaseState.fromDraft', { revision: latest.sourceRevision }) : t('workflows.releaseState.candidateDescription')} label={t('workflows.releaseState.latestImmutableVersion')} tone={latest && deployments.some((item) => item.workflowVersionId === latest.id) ? 'success' : 'warning'} value={latest ? `v${latest.versionNumber}` : '—'} />
      <StateCard detail={primaryDeployment ? t('workflows.releaseState.activeDeployment', { sequence: primaryDeployment.sequenceNumber }) : t('workflows.releaseState.notDeployed')} label={primaryEnvironment ? t('workflows.releaseState.environmentCurrent', { environment: primaryEnvironment.name }) : t('workflows.releaseState.primaryEnvironment')} tone={primaryDeployment ? 'success' : 'neutral'} value={primaryDeployment ? `v${primaryDeployment.versionNumber}` : '—'} />
    </div>
  </>
}

function StateCard({ detail, label, tone, value }: { detail: string; label: string; tone: 'success' | 'warning' | 'neutral'; value: string }) {
  return <Card className="p-4"><p className="text-[10px] text-muted-foreground">{label}</p><div className="mt-2 flex items-center gap-2"><CircleDot className={`size-3.5 ${tone === 'success' ? 'text-success' : tone === 'warning' ? 'text-warning' : 'text-muted-foreground'}`} /><strong className="text-base">{value}</strong></div><p className="mt-2 text-[10px] text-muted-foreground">{detail}</p></Card>
}

type VersionsCardProps = {
  versions: WorkflowVersion[]
  deployments: WorkflowDeployment[]
  canCreate: boolean
  draftDirty: boolean
  canRun: boolean
  creating: boolean
  running: boolean
  onCreate: () => void
  onPublish: (versionId: string) => void
  onRun: (versionId: string) => void
}

export function WorkflowVersionsCard({ versions, deployments, canCreate, canRun, creating, draftDirty, running, onCreate, onPublish, onRun }: VersionsCardProps) {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  return <Card className="overflow-hidden">
    <div className="flex min-h-15 items-center gap-2 border-b border-border px-5"><GitCommitHorizontal className="size-4 text-primary" /><h2 className="text-sm font-semibold">{t('workflows.versions')}</h2><span className="flex-1" />{canCreate && <Button disabled={creating || !draftDirty} onClick={onCreate} size="sm" variant="secondary"><Plus className="size-3.5" />{t('workflows.saveAsVersion')}</Button>}</div>
    <div className="divide-y divide-border px-5">{versions.map((version, index) => {
      const versionDeployments = deployments.filter((item) => item.workflowVersionId === version.id)
      const activeDeployments = versionDeployments.filter((item) => item.status === 'active')
      return <div className="flex flex-wrap items-center gap-3 py-4" key={version.id}>
        <div className="min-w-0 flex-1"><div className="flex flex-wrap items-center gap-2"><strong className="text-xs">v{version.versionNumber}</strong>{index === 0 && <Badge tone="primary">{t('workflows.versionBadges.latest')}</Badge>}{activeDeployments.map((item) => <Badge key={item.environmentId} tone="success">{t('workflows.versionBadges.environmentCurrent', { environment: item.environmentName })}</Badge>)}{versionDeployments.length === 0 && <Badge tone="warning">{t('workflows.versionBadges.unreleased')}</Badge>}{versionDeployments.length > 0 && activeDeployments.length === 0 && <Badge tone="neutral">{t('workflows.versionBadges.historical')}</Badge>}</div><p className="mt-1.5 truncate text-[10px] text-muted-foreground">{t('workflows.sourceRevision', { revision: version.sourceRevision })} · {version.contentHash.slice(0, 20)}… · {formatDateTime(version.createdAt)}</p></div>
        <div className="flex items-center gap-1"><Button asChild size="sm" variant="ghost"><Link to={`/workflows/${version.workflowId}/versions/${version.id}/editor`}>{t("workflows.viewCanvas")}</Link></Button>{canRun && <Button disabled={running} onClick={() => onRun(version.id)} size="sm" variant="ghost"><Play className="size-3.5" />{t('workflows.testRun')}</Button>} {canCreate && <Button onClick={() => onPublish(version.id)} size="sm" variant="secondary"><Rocket className="size-3.5" />{t('workflows.publishTo')}</Button>}</div>
      </div>
    })}{versions.length === 0 && <Empty />}</div>
  </Card>
}

type EnvironmentsCardProps = {
  environments: WorkflowEnvironment[]
  deployments: WorkflowDeployment[]
  canPublish: boolean
  onHistory: () => void
  onPublish: (environmentId: string) => void
}

export function WorkflowEnvironmentsCard({ environments, deployments, canPublish, onHistory, onPublish }: EnvironmentsCardProps) {
  const { t } = useTranslation()
  return <Card className="overflow-hidden">
    <div className="flex min-h-15 items-center gap-2 border-b border-border px-5"><Rocket className="size-4 text-primary" /><h2 className="text-sm font-semibold">{t('workflows.environmentStatus')}</h2><span className="flex-1" />{deployments.length > 0 && <Button onClick={onHistory} size="sm" variant="ghost"><History className="size-3.5" />{t('workflows.viewReleaseHistory')}</Button>}</div>
    <div className="divide-y divide-border px-5">{environments.map((environment) => {
      const active = deployments.find((item) => item.environmentId === environment.id && item.status === 'active')
      return <div className="flex items-center gap-3 py-4" key={environment.id}><span className={`grid size-9 shrink-0 place-items-center rounded-lg text-xs font-semibold ${active ? 'bg-primary/10 text-primary' : 'bg-muted text-muted-foreground'}`}>{environment.code.slice(0, 1).toUpperCase()}</span><div className="min-w-0 flex-1"><div className="flex flex-wrap items-center gap-2"><strong className="text-xs">{environment.name}</strong>{active ? <Badge tone="success">{t('workflows.versionBadges.running')}</Badge> : <Badge tone="neutral">{t('workflows.versionBadges.notDeployed')}</Badge>}</div><p className="mt-1.5 text-[10px] text-muted-foreground">{active ? t('workflows.environmentActiveDetail', { version: active.versionNumber, sequence: active.sequenceNumber }) : t('workflows.noActiveDeployment')}</p></div>{canPublish && <Button onClick={() => onPublish(environment.id)} size="sm" variant="ghost">{active ? t('workflows.updateEnvironment') : t('workflows.releaseEnvironment')}</Button>}</div>
    })}{environments.length === 0 && <Empty />}</div>
  </Card>
}

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
