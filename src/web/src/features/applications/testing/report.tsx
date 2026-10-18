import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import { Badge } from '../../../shared/ui/badge'
import { Button } from '../../../shared/ui/button'
import { Card } from '../../../shared/ui/card'
import { Dialog, DialogContent } from '../../../shared/ui/dialog'
import { Select } from '../../../shared/ui/select'
import { Table, TableCell, TableContainer, TableHead } from '../../../shared/ui/table'
import { formatCost } from '../../../shared/lib/cost-format'
import { type Batch, type CaseResult, type Run } from './types'

function currencies(values: Record<string, number>[]) {
  const total: Record<string, number> = {}
  for (const value of values) for (const [currency, cost] of Object.entries(value)) total[currency] = (total[currency] ?? 0) + cost
  return Object.entries(total).map(([currency, cost]) => formatCost(cost, currency)).join(' + ') || '0'
}
function score(value?: number | null) { return value == null ? '—' : (value * 100).toFixed(1) }
function Output({ value }: { value: unknown }) { return <pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-canvas p-4 text-xs">{typeof value === 'string' ? value : JSON.stringify(value, null, 2) ?? '—'}</pre> }
function CaseStatus({ value }: { value: string }) { const { t } = useTranslation(); return <Badge tone={value === 'passed' ? 'success' : ['failed', 'error'].includes(value) ? 'danger' : 'neutral'}>{t(`testing.status.${value}`)}</Badge> }

export function EvaluationReport({ report }: { report: Batch }) {
  const { t } = useTranslation()
  const [filter, setFilter] = useState('issues')
  const [selected, setSelected] = useState<{ value: CaseResult; run: Run }>()
  const [baselines, setBaselines] = useState<Record<string, string>>({})
  const [comparison, setComparison] = useState(false)
  const groups = useMemo(() => {
    const result = new Map<string, Run[]>()
    for (const run of report.runs) result.set(run.datasetId, [...(result.get(run.datasetId) ?? []), run])
    return [...result.values()].map(runs => runs.sort((a, b) => a.workflowVersionNumber - b.workflowVersionNumber))
  }, [report.runs])
  const totals = report.runs.reduce((value, run) => ({ total: value.total + run.totalCases, scored: value.scored + run.scoredCount, passed: value.passed + run.passedCount, failed: value.failed + run.failedCount, errors: value.errors + run.errorCount, score: value.score + (run.averageScore ?? 0) * run.scoredCount, business: value.business + run.businessCostMicros, judge: value.judge + run.judgeCostMicros }), { total: 0, scored: 0, passed: 0, failed: 0, errors: 0, score: 0, business: 0, judge: 0 })
  const metrics = [
    [t('testing.progress'), `${report.runs.reduce((sum, run) => sum + run.cases.filter(value => ['passed', 'failed', 'error', 'cancelled'].includes(value.status)).length, 0)}/${totals.total}`],
    [t('testing.passRate'), totals.scored ? `${(totals.passed / totals.scored * 100).toFixed(1)}%` : '—'],
    [t('testing.averageScore'), totals.scored ? (totals.score / totals.scored * 100).toFixed(1) : '—'],
    [t('testing.qualityFailed'), String(totals.failed)],
    [t('testing.errors'), String(totals.errors)],
    [t('testing.totalDuration'), `${(report.durationMs / 1000).toFixed(1)} s`],
    [t('testing.cost'), `${currencies(report.runs.map(run => run.businessCostByCurrency))} / ${currencies(report.runs.map(run => run.judgeCostByCurrency))}`],
  ]
  return <div className="space-y-5"><Card className="p-4"><div className="flex items-center justify-between"><div className="space-y-1 text-xs"><p>{report.profileName} · {t('testing.passThreshold')} {score(report.passThreshold)}</p><p className="text-muted-foreground">{t('testing.modelVersion')}: {report.modelName}<span className="ml-2 text-[10px]">{report.modelVersion}</span></p></div><CaseStatus value={report.status} /></div><details className="mt-3 text-xs"><summary className="cursor-pointer text-muted-foreground">{t('testing.criteria')}</summary><p className="mt-2 whitespace-pre-wrap">{report.criteria}</p></details></Card>
    <div className="grid grid-cols-3 gap-3">{metrics.map(([label, value]) => <Card className="p-4" key={label}><p className="text-xs text-muted-foreground">{label}</p><p className="mt-2 text-xl font-semibold tabular-nums">{value}</p></Card>)}</div>
    <div className="flex items-center justify-between gap-4"><Select aria-label={t('testing.resultFilter')} value={filter} onValueChange={setFilter} options={['issues', 'all', 'passed', 'failed', 'error'].map(value => ({ value, label: t(`testing.filters.${value}`) }))} /><Button variant="secondary" onClick={() => setComparison(value => !value)}>{comparison ? t('testing.caseResults') : t('testing.compare')}</Button></div>
    {comparison ? <Comparison runs={report.runs} onSelect={(value, run) => setSelected({ value, run })} /> : groups.map(runs => {
      const baseline = runs.find(run => run.id === baselines[runs[0].datasetId]) ?? runs[0]
      return <section className="space-y-3" key={runs[0].datasetId}><div className="flex items-center justify-between"><h3 className="text-sm font-semibold">{runs[0].datasetName} · {t('testing.revision')} {runs[0].sourceRevision}</h3>{runs.length > 1 && <Select aria-label={t('testing.baseline', { name: runs[0].datasetName })} value={baseline.id} onValueChange={value => setBaselines(current => ({ ...current, [runs[0].datasetId]: value }))} options={runs.map(run => ({ value: run.id, label: `${t('testing.baselineLabel')} v${run.workflowVersionNumber}` }))} />}</div>
        {runs.map(run => {
          const displayed = run.cases.filter(value => filter === 'all' || (filter === 'issues' ? ['failed', 'error'].includes(value.status) : value.status === filter))
          const sameData = run.datasetVersionId === baseline.datasetVersionId
          const changes = run.id !== baseline.id && sameData ? run.cases.reduce((value, result) => { const previous = baseline.cases.find(base => base.id === result.id); if (previous?.score != null && result.score != null) { if (result.score < previous.score) value.regressions++; if (result.score > previous.score) value.improvements++ } return value }, { regressions: 0, improvements: 0 }) : undefined
          return <Card key={run.id} className="overflow-hidden"><div className="flex flex-wrap items-center gap-4 border-b border-border px-5 py-4 text-xs"><strong>v{run.workflowVersionNumber}</strong><CaseStatus value={run.status} /><span>{t('testing.averageScore')} {score(run.averageScore)}</span>{changes && <span>{t('testing.changes', changes)}</span>}{run.id !== baseline.id && !sameData && <span className="text-warning">{t('testing.dataChanged')}</span>}</div>
            {run.dispatchError != null && <p role="alert" className="p-4 text-xs text-danger">{JSON.stringify(run.dispatchError)}</p>}
            <TableContainer><Table><thead><tr><TableHead>{t('common.name')}</TableHead><TableHead>{t('common.status')}</TableHead><TableHead>{t('testing.score')}</TableHead><TableHead>{t('testing.reason')}</TableHead><TableHead>{t('testing.duration')}</TableHead><TableHead /></tr></thead><tbody>{displayed.map(value => <tr key={value.id} className="border-t border-border"><TableCell>{value.name}</TableCell><TableCell><CaseStatus value={value.status} /></TableCell><TableCell>{score(value.score)}</TableCell><TableCell><span className="line-clamp-2 max-w-96">{value.reason ?? value.errorMessage ?? '—'}</span></TableCell><TableCell>{value.durationMs == null ? '—' : `${value.durationMs} ms`}</TableCell><TableCell><Button size="sm" variant="ghost" onClick={() => setSelected({ value, run })}>{t('testing.inspect')}</Button></TableCell></tr>)}</tbody></Table></TableContainer>{displayed.length === 0 && <p className="p-6 text-center text-xs text-muted-foreground">{t('testing.noResults')}</p>}
          </Card>
        })}
      </section>
    })}
    {selected && <Dialog open onOpenChange={open => { if (!open) setSelected(undefined) }}><DialogContent title={selected.value.name} className="left-auto right-0 top-0 h-dvh w-[900px] translate-x-0 translate-y-0 rounded-none"><div className="flex items-center justify-between border-b border-border p-5"><div><h2 className="text-base font-semibold">{selected.value.name}</h2><p className="mt-1 text-xs text-muted-foreground">{selected.run.datasetName} · v{selected.run.workflowVersionNumber}</p></div><Button variant="ghost" onClick={() => setSelected(undefined)}>{t('common.close')}</Button></div><div className="space-y-5 p-5"><div className="flex items-center gap-4"><CaseStatus value={selected.value.status} /><strong>{score(selected.value.score)}</strong></div><section><h3 className="mb-2 text-sm font-semibold">{t('testing.input')}</h3><Output value={selected.value.input} /></section><div className="grid grid-cols-2 gap-4"><section><h3 className="mb-2 text-sm font-semibold">{t('testing.expectedOutput')}</h3><Output value={selected.value.expectedOutput} /></section><section><h3 className="mb-2 text-sm font-semibold">{t('testing.actualOutput')}</h3><Output value={selected.value.actualOutput} /></section></div><section><h3 className="mb-2 text-sm font-semibold">{t('testing.reason')}</h3><p className="whitespace-pre-wrap text-sm">{selected.value.reason ?? selected.value.errorMessage ?? '—'}</p>{selected.value.error != null && <Output value={selected.value.error} />}</section><p className="text-xs">{t('testing.cost')}: {currencies([selected.value.businessCostByCurrency])} / {currencies([selected.value.judgeCostByCurrency])}</p><div className="flex gap-3">{selected.value.executionId && <Button asChild variant="secondary" size="sm"><Link to={`/executions/${selected.value.executionId}`}>{t('testing.executionTrace')}</Link></Button>}{selected.value.judgeExecutionId && <Button asChild variant="secondary" size="sm"><Link to={`/executions/${selected.value.judgeExecutionId}`}>{t('testing.judgeTrace')}</Link></Button>}</div></div></DialogContent></Dialog>}
  </div>
}

function Comparison({ runs, onSelect }: { runs: Run[]; onSelect: (value: CaseResult, run: Run) => void }) {
  const { t } = useTranslation()
  const rows = new Map<string, Map<string, CaseResult>>()
  for (const run of runs) for (const value of run.cases) { if (!rows.has(value.lineageId)) rows.set(value.lineageId, new Map()); rows.get(value.lineageId)!.set(run.id, value) }
  return <Card><p className="border-b border-border p-4 text-xs text-muted-foreground">{t('testing.crossDatasetHint')}</p><TableContainer><Table><thead><tr><TableHead>{t('common.name')}</TableHead>{runs.map(run => <TableHead key={run.id}>{run.datasetName}<span className="mt-1 block text-muted-foreground">v{run.workflowVersionNumber}</span></TableHead>)}</tr></thead><tbody>{[...rows.entries()].map(([lineage, values]) => { const first = [...values.values()][0]; return <tr key={lineage} className="border-t border-border"><TableCell>{first.name}</TableCell>{runs.map(run => { const value = values.get(run.id); const changed = value && (run.contractFingerprint !== runs.find(source => source.id === [...values.keys()][0])?.contractFingerprint || JSON.stringify(value.input) !== JSON.stringify(first.input) || JSON.stringify(value.expectedOutput) !== JSON.stringify(first.expectedOutput)); return <TableCell key={run.id}>{value ? <div className="space-y-2"><div className="flex items-center gap-2"><CaseStatus value={value.status} /><span>{score(value.score)}</span></div>{changed && <p className="text-warning">{t('testing.dataChanged')}</p>}<p className="line-clamp-3 max-w-72 text-xs">{JSON.stringify(value.actualOutput)}</p><Button size="sm" variant="ghost" onClick={() => onSelect(value, run)}>{t('testing.inspect')}</Button></div> : t('testing.absent')}</TableCell> })}</tr> })}</tbody></Table></TableContainer></Card>
}
