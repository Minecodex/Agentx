import { useTranslation } from 'react-i18next'

import { Badge } from '../ui/badge'

export type StatusValue = 'active' | 'inactive' | 'invited' | 'published' | 'draft' | 'running' | 'success' | 'failed' | 'pending' | 'waiting' | 'completed' | 'synced' | 'syncing' | 'untested' | 'healthy' | 'unhealthy'

const tones = {
  active: 'success', published: 'success', success: 'success', completed: 'success', synced: 'success', healthy: 'success',
  running: 'primary', syncing: 'primary',
  pending: 'warning', waiting: 'warning', invited: 'warning', draft: 'neutral', inactive: 'neutral', untested: 'neutral', failed: 'danger', unhealthy: 'danger',
} as const

export function StatusBadge({ status, label }: { status: StatusValue; label?: string }) {
  const { t } = useTranslation()
  return <Badge className="gap-1.5 whitespace-nowrap" tone={tones[status]}><span className="size-1.5 shrink-0 rounded-full bg-current" />{label ?? t(`common.${status}`)}</Badge>
}
