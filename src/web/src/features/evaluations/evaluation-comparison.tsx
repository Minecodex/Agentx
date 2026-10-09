import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { localizedValue } from '../../shared/lib/localized-value'
import { apiRequest } from '../../shared/api/client'
import type { EvaluationRun } from '../../shared/api/types'
import { EmptyState } from '../../shared/components/empty-state'
import { SearchableMultiSelect } from '../../shared/components/searchable-multi-select'
import { Card } from '../../shared/ui/card'
import { Table, TableCell, TableContainer, TableHead } from '../../shared/ui/table'

type Comparison = {
  runs: Array<{ runId: string; name: string; metrics: Record<string, number> }>
  alignedCaseCount: number
  totalCaseCount: number
  caseDeltas: Array<{ caseKey: string; statusByRun: Array<string | null>; scoreByRun: Array<number | null>; durationMsByRun: Array<number | null>; costMicrosByRun: Array<number | null>; targetExecutionIdByRun: Array<string | null> }>
  ruleAggregates: Array<{ ruleKey: string; resultsByRun: Array<{ passRate: number; averageScore: number | null; evaluatorType: string; evaluatorExecutionIds: string[] } | null> }>
}

export function EvaluationComparison({ runIds, onChange }: { runIds: string[]; onChange: (ids: string[]) => void }) {
  const { t } = useTranslation()
  const runs = useQuery({ queryKey: ['evaluations'], queryFn: () => apiRequest<EvaluationRun[]>('/evaluations') })
  const comparison = useQuery({ queryKey: ['evaluation-comparison', runIds], enabled: runIds.length >= 2 && runIds.length <= 5, queryFn: () => apiRequest<Comparison>(`/evaluations/compare?${new URLSearchParams({ runIds: runIds.join(',') })}`), retry: false })
  const value = comparison.data
  return <div className="mt-5 space-y-5">
    <Card className="flex items-center gap-4 p-4"><SearchableMultiSelect label={t('evaluations.compareSelect')} queryKey="evaluation-comparison" value={runIds} onChange={(ids) => { if (ids.length <= 5) onChange(ids) }} options={(runs.data ?? []).map((run) => ({ value: run.id, label: run.name }))} /><span className="text-xs text-muted-foreground">{t('evaluations.compareHint')}</span></Card>
    {!value ? <EmptyState title={comparison.isError ? t('common.loadFailed') : comparison.isLoading ? t('common.loading') : t('evaluations.compareSelect')} description={comparison.error?.message ?? t('evaluations.compareHint')} /> : <>
      <p className="text-xs text-muted-foreground">{t('evaluations.compareCoverage', { aligned: value.alignedCaseCount, total: value.totalCaseCount })}</p>
      <Card><TableContainer><Table><thead><tr><TableHead>{t('evaluations.compareMetrics')}</TableHead>{value.runs.map((run) => <TableHead key={run.runId}><Link className="text-primary hover:underline" to={`/evaluations/${run.runId}`}>{run.name}</Link></TableHead>)}</tr></thead><tbody>{Object.keys(value.runs[0]?.metrics ?? {}).map((key) => <tr className="border-t border-border" key={key}><TableCell>{localizedValue(t, 'evaluations.metricLabels', key)}</TableCell>{value.runs.map((run, index) => <TableCell key={run.runId}>{format(run.metrics[key])}{index > 0 && <span className="ml-2 text-[10px]">({signed(run.metrics[key] - value.runs[0].metrics[key])})</span>}</TableCell>)}</tr>)}</tbody></Table></TableContainer></Card>
      <Card><h2 className="border-b border-border px-5 py-4 text-sm font-semibold">{t('evaluations.caseResults')}</h2><TableContainer><Table><thead><tr><TableHead>{t('evaluations.caseResults')}</TableHead>{value.runs.map((run) => <TableHead key={run.runId}>{run.name}</TableHead>)}</tr></thead><tbody>{value.caseDeltas.map((item) => <tr className="border-t border-border" key={item.caseKey}><TableCell>{item.caseKey}</TableCell>{value.runs.map((run, index) => <TableCell key={run.runId}>{item.statusByRun[index] === null ? t('evaluations.compareAbsent') : <><p>{t(`common.${item.statusByRun[index]}`, { defaultValue: item.statusByRun[index] ?? '' })} · {format(item.scoreByRun[index])}</p><p className="mt-1 text-[10px]">{format(item.durationMsByRun[index])} ms · {format(item.costMicrosByRun[index])}</p>{item.targetExecutionIdByRun[index] && <Link className="text-primary hover:underline" to={`/executions/${item.targetExecutionIdByRun[index]}`}>Trace</Link>}</>}</TableCell>)}</tr>)}</tbody></Table></TableContainer></Card>
      <Card><h2 className="border-b border-border px-5 py-4 text-sm font-semibold">{t('evaluations.scoringRules')}</h2><TableContainer><Table><thead><tr><TableHead>{t('evaluations.ruleName')}</TableHead>{value.runs.map((run) => <TableHead key={run.runId}>{run.name}</TableHead>)}</tr></thead><tbody>{value.ruleAggregates.map((rule) => <tr className="border-t border-border" key={rule.ruleKey}><TableCell>{rule.ruleKey}</TableCell>{rule.resultsByRun.map((result, index) => <TableCell key={value.runs[index].runId}>{result ? <><p>{(result.passRate * 100).toFixed(1)}% · {format(result.averageScore)}</p><p className="text-[10px]">{result.evaluatorType}</p>{result.evaluatorExecutionIds.map((id) => <Link className="mr-2 text-primary hover:underline" key={id} to={`/executions/${id}`}>{t('evaluations.evaluatorTrace')}</Link>)}</> : t('evaluations.compareAbsent')}</TableCell>)}</tr>)}</tbody></Table></TableContainer></Card>
    </>}
  </div>
}

function format(value?: number | null) { return value == null ? '—' : Number.isInteger(value) ? String(value) : value.toFixed(3) }
function signed(value: number) { return Number.isFinite(value) ? `${value >= 0 ? '+' : ''}${format(value)}` : '—' }
