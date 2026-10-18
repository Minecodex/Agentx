import { useTranslation } from 'react-i18next'
import { Badge } from '../../../shared/ui/badge'
import { Button } from '../../../shared/ui/button'
import { Card } from '../../../shared/ui/card'
import { Table, TableCell, TableContainer, TableHead } from '../../../shared/ui/table'
import { summary, type ContractVersion, type Dataset, type TestCase } from './types'

export function DatasetList({ datasets, versions, canManage, canDelete, onSelect, onEdit, onCopy, onDelete }: { datasets: Dataset[]; versions: ContractVersion[]; canManage: boolean; canDelete: boolean; onSelect: (id: string) => void; onEdit: (value: Dataset) => void; onCopy: (value: Dataset) => void; onDelete: (value: Dataset) => void }) {
  const { t } = useTranslation()
  return <Card><TableContainer><Table><thead><tr>{['datasetName', 'contractVersion', 'matchingVersions', 'caseCount', 'needsAttention'].map(key => <TableHead key={key}>{t(`testing.${key}`)}</TableHead>)}<TableHead /></tr></thead><tbody>{datasets.map(value => <tr key={value.id} className="border-t border-border">
    <TableCell><Button size="sm" variant="ghost" onClick={() => onSelect(value.id)}>{value.name}</Button></TableCell>
    <TableCell>v{versions.find(version => version.id === value.sourceWorkflowVersionId)?.versionNumber ?? '—'}</TableCell>
    <TableCell>{versions.filter(version => value.compatibleVersions.includes(version.id)).map(version => `v${version.versionNumber}`).join(', ')}</TableCell>
    <TableCell>{value.caseCount}</TableCell><TableCell>{value.incompleteCount}</TableCell>
    <TableCell><div className="flex justify-end gap-1">{canManage && <><Button size="sm" variant="ghost" onClick={() => onEdit(value)}>{t('common.edit')}</Button><Button size="sm" variant="ghost" onClick={() => onCopy(value)}>{t('testing.copyDataset')}</Button></>}{canDelete && <Button size="sm" variant="ghost" onClick={() => onDelete(value)}>{t('common.delete')}</Button>}</div></TableCell>
  </tr>)}</tbody></Table></TableContainer>{datasets.length === 0 && <p className="p-8 text-center text-sm text-muted-foreground">{t('testing.noDatasets')}</p>}</Card>
}

export function DatasetCases({ dataset, versions, cases, selectedIds, onSelect, canManage, onEdit, onDelete }: { dataset: Dataset; versions: ContractVersion[]; cases: TestCase[]; selectedIds: string[]; onSelect: (ids: string[]) => void; canManage: boolean; onEdit: (value: TestCase) => void; onDelete: (value: TestCase) => void }) {
  const { t } = useTranslation()
  const selected = new Set(selectedIds)
  const count = cases.filter(value => selected.has(value.id)).length
  return <Card><p className="border-b border-border px-5 py-3 text-xs text-muted-foreground">{t('testing.matchingVersions')}: {versions.filter(value => dataset.compatibleVersions.includes(value.id)).map(value => `v${value.versionNumber}`).join(', ') || '—'}</p>
    <TableContainer><Table><thead><tr><TableHead className="w-12"><input type="checkbox" aria-label={t('testing.selectAllCases')} checked={cases.length > 0 && count === cases.length} disabled={cases.length === 0} ref={node => { if (node) node.indeterminate = count > 0 && count < cases.length }} onChange={event => onSelect(event.target.checked ? cases.map(value => value.id) : [])} /></TableHead>{['name', 'input', 'expectedOutput', 'readiness'].map(key => <TableHead key={key}>{t(`testing.${key}`)}</TableHead>)}<TableHead /></tr></thead>
      <tbody>{cases.map(value => <tr key={value.id} className="border-t border-border"><TableCell><input type="checkbox" aria-label={t('testing.selectCase', { name: value.name })} checked={selected.has(value.id)} onChange={event => onSelect(event.target.checked ? [...selectedIds, value.id] : selectedIds.filter(id => id !== value.id))} /></TableCell>
        <TableCell><strong>{value.name}</strong>{value.tags.length > 0 && <p className="mt-1 text-[11px]">{value.tags.join(', ')}</p>}</TableCell>
        <TableCell><span className="block max-w-64 truncate">{summary(value.input)}</span></TableCell><TableCell><span className="block max-w-64 truncate">{summary(value.expectedOutput)}</span></TableCell>
        <TableCell><Badge tone={value.validationErrors.length ? 'warning' : 'success'}>{value.validationErrors.length ? t('testing.draft') : t('testing.ready')}</Badge>{value.validationErrors.length > 0 && <p className="mt-1 text-[11px] text-muted-foreground">{value.validationErrors.some(error => error.code !== 'REFERENCE_UNCONFIRMED') ? t('testing.incomplete') : t('testing.awaitingConfirmation')}</p>}</TableCell>
        <TableCell>{canManage && <div className="flex justify-end gap-1"><Button size="sm" variant="ghost" onClick={() => onEdit(value)}>{t('common.edit')}</Button><Button size="sm" variant="ghost" onClick={() => onDelete(value)}>{t('common.delete')}</Button></div>}</TableCell>
      </tr>)}{cases.length === 0 && <tr><TableCell colSpan={6}>{t('testing.noCases')}</TableCell></tr>}</tbody>
    </Table></TableContainer>
  </Card>
}
