import { useQuery } from '@tanstack/react-query'
import { useEffect, useMemo } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest } from '../../shared/api/client'
import type { WorkflowVersion } from '../../shared/api/types'
import { Select } from '../../shared/ui/select'
import { channelSchemas } from './channel-inputs'

type Props = { workflowId: string; versionId: string; value: string; onChange: (value: string) => void }

export function DeploymentUserInput({ workflowId, versionId, value, onChange }: Props) {
  const { t } = useTranslation()
  const version = useQuery({
    queryKey: ['workflow-version', workflowId, versionId],
    enabled: Boolean(versionId),
    queryFn: () => apiRequest<WorkflowVersion>(`/workflows/${workflowId}/versions/${versionId}`),
  })
  const fields = useMemo(() => {
    const schema = channelSchemas(undefined, version.data ? [version.data] : undefined)?.input as { properties?: Record<string, { type?: unknown }> } | undefined
    return Object.entries(schema?.properties ?? {}).filter(([, schema]) => schema.type === 'string').map(([name]) => name)
  }, [version.data])
  useEffect(() => {
    if (version.data && value && !fields.includes(value)) onChange('')
  }, [version.data, value, fields, onChange])
  return <details className="rounded-lg border border-border p-3 text-xs">
    <summary className="cursor-pointer font-medium">{t('applications.advancedConfiguration')}</summary>
    <div className="mt-4 space-y-3">
      <span className="block font-medium">{t('applications.apiUserIdInput')}</span>
      <Select aria-label={t('applications.apiUserIdInput')} disabled={version.isLoading} name="apiUserIdInput" onValueChange={selected => onChange(selected === 'none' ? '' : selected.slice(6))} options={[{ value: 'none', label: t('applications.userIdNotConfigured') }, ...fields.map(name => ({ value: `input:${name}`, label: name }))]} value={value ? `input:${value}` : 'none'} />
      <p className="whitespace-pre-line text-[11px] leading-5 text-muted-foreground">{t('applications.userIdExplanation')}</p>
      {version.isError && <p className="text-danger" role="alert">{t('common.loadFailed')}</p>}
    </div>
  </details>
}
