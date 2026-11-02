import { useTranslation } from 'react-i18next'
import type { components } from '../../shared/api/generated'
import { Card } from '../../shared/ui/card'
import { Badge } from '../../shared/ui/badge'
type Subject = components['schemas']['ApprovalSubjectV1']

export function ToolEffectCard({ subject }: { subject: Subject }) {
  const { t } = useTranslation()
  if (subject.kind !== 'agent_tool_effect') return null
  return <Card className="mt-5 space-y-3 border-primary/25 p-5" data-testid="tool-effect-approval">
    <div className="flex items-center gap-3">
      <Badge tone="warning">{t('applicationMcp.toolOperation')}</Badge>
      <h2 className="text-sm font-semibold">{subject.toolName}</h2>
    </div>
    <details>
      <summary className="cursor-pointer text-xs font-medium">{t('applicationMcp.auditDetails')}</summary>
      <dl className="grid grid-cols-[160px_minmax(0,1fr)] gap-x-4 gap-y-2 pt-3 text-xs">
        <dt className="text-muted-foreground">{t('applicationMcp.toolVersion')}</dt>
        <dd className="break-all font-mono">{subject.toolVersion}</dd>
        <dt className="text-muted-foreground">{t('applicationMcp.effectId')}</dt>
        <dd className="break-all font-mono">{subject.effectId}</dd>
        <dt className="text-muted-foreground">{t('applicationMcp.argumentsHash')}</dt>
        <dd className="break-all font-mono">{subject.argumentsHash}</dd>
        <dt className="text-muted-foreground">{t('applicationMcp.policyRevision')}</dt>
        <dd>#{subject.policyRevision}</dd>
      </dl>
    </details>
  </Card>
}
