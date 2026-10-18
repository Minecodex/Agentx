import { useTranslation } from 'react-i18next'
import { StatusBadge } from '../../../shared/components/status-badge'
import { useLocaleFormat } from '../../../shared/lib/locale-format'
import { Button } from '../../../shared/ui/button'
import { Table, TableCell, TableContainer, TableHead } from '../../../shared/ui/table'
import type { Batch, Run } from './types'

export function EvaluationList({ batches, onSelect }: { batches: Batch[]; onSelect: (id: string) => void }) {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  return <TableContainer><Table><thead><tr><TableHead>{t('common.name')}</TableHead><TableHead>{t('testing.profile')}</TableHead><TableHead>{t('testing.datasets')}</TableHead><TableHead>{t('testing.workflowVersions')}</TableHead><TableHead>{t('testing.tasks')}</TableHead><TableHead>{t('testing.averageScore')}</TableHead><TableHead>{t('common.status')}</TableHead><TableHead>{t('testing.createdAt')}</TableHead><TableHead /></tr></thead><tbody>{batches.map(value => {
    const groups = new Map<string, Run[]>()
    for (const run of value.runs) groups.set(run.datasetId, [...(groups.get(run.datasetId) ?? []), run])
    const scored = value.runs.reduce((sum, run) => sum + run.scoredCount, 0)
    const score = value.runs.reduce((sum, run) => sum + (run.averageScore ?? 0) * run.scoredCount, 0)
    return <tr key={value.id} className="border-t border-border"><TableCell>{value.name}</TableCell><TableCell>{value.profileName}</TableCell>
      <TableCell>{[...groups].map(([id, runs]) => <p className="max-w-48 truncate leading-6" title={runs[0].datasetName} key={id}>{runs[0].datasetName}</p>)}</TableCell>
      <TableCell>{[...groups].map(([id, runs]) => <p className="whitespace-nowrap leading-6" key={id}>{[...new Set(runs.map(run => run.workflowVersionNumber))].sort((a, b) => a - b).map(number => `v${number}`).join(', ')}</p>)}</TableCell>
      <TableCell>{value.runs.filter(run => !['created', 'queued', 'running'].includes(run.status)).length}/{value.runs.length}</TableCell><TableCell>{scored ? (score / scored * 100).toFixed(1) : '—'}</TableCell>
      <TableCell className="whitespace-nowrap"><StatusBadge status={value.status === 'queued' ? 'pending' : value.status === 'cancelled' ? 'inactive' : value.status as 'running' | 'completed' | 'failed'} label={t(`common.${value.status}`)} /></TableCell><TableCell className="whitespace-nowrap">{formatDateTime(value.createdAt)}</TableCell><TableCell><Button className="whitespace-nowrap" size="sm" variant="ghost" onClick={() => onSelect(value.id)}>{t('testing.report')}</Button></TableCell>
    </tr>
  })}</tbody></Table></TableContainer>
}
