import { useTranslation } from 'react-i18next'
import type { components } from '../api/generated'
import { Select } from '../ui/select'

export type EndUser = Pick<components['schemas']['ApplicationUserIdentity'], 'source' | 'externalUserId' | 'displayName'>

export function EndUserLabel({ user }: { user?: EndUser | null }) {
  const { t } = useTranslation()
  if (!user) return <span title={t('applications.noEndUser')}>—</span>
  const sourceLabel = user.source === 'platform' ? t('applications.userSources.platform')
    : user.source === 'channel' ? t('applications.userSources.channel')
      : t('applications.userSources.api_parameter')
  const name = user.source === 'platform' ? user.displayName || t('applications.unknownUser') : user.externalUserId
  return <span className="inline-flex min-w-0 w-36 max-w-full flex-col align-middle text-[11px]" title={`${name}\n${user.externalUserId}`}>
    <span className="truncate text-foreground">{name}</span>
    <span className="truncate text-[10px] text-muted-foreground">{sourceLabel}</span>
  </span>
}

export function UserSourceFilter({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const { t } = useTranslation()
  return <Select aria-label={t('applications.userSource')} value={value || 'all'} onValueChange={value => onChange(value === 'all' ? '' : value)} options={[
    { value: 'all', label: t('applications.userSource') },
    { value: 'platform', label: t('applications.userSources.platform') },
    { value: 'channel', label: t('applications.userSources.channel') },
    { value: 'api_parameter', label: t('applications.userSources.api_parameter') },
  ]} />
}
