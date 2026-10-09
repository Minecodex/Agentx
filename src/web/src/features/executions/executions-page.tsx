import * as Popover from '@radix-ui/react-popover'
import { useQuery } from '@tanstack/react-query'
import { flexRender, getCoreRowModel, useReactTable, type ColumnDef } from '@tanstack/react-table'
import { Activity, ChevronDown, Filter, X } from 'lucide-react'
import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigationType, useSearchParams } from 'react-router-dom'

import { ApiClientError, apiRequest } from '../../shared/api/client'
import type { Application, Department, McpTool, PageResponse, User, Workflow } from '../../shared/api/types'
import { EmptyState } from '../../shared/components/empty-state'
import { EntityCell } from '../../shared/components/entity-cell'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { SearchableMultiSelect, type MultiSelectOption } from '../../shared/components/searchable-multi-select'
import { Card } from '../../shared/ui/card'
import { Button } from '../../shared/ui/button'
import { DateTimePicker } from '../../shared/ui/date-time-picker'
import { Input } from '../../shared/ui/input'
import { SearchField } from '../../shared/ui/search-field'
import { StatusBadge } from '../../shared/components/status-badge'
import { Table, TableCell, TableContainer, TableHead } from '../../shared/ui/table'
import { useToast } from '../../shared/ui/toast'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { localizedValue } from '../../shared/lib/localized-value'
import { executionStatus, formatCost } from './execution-format'

type ExecutionRow = {
  id: string
  applicationId?: string | null
  applicationName?: string | null
  workflowId: string
  workflowName: string
  workflowVersionNumber?: number | null
  triggerType: string
  triggerName?: string | null
  triggerSourceId?: string | null
  initiatorUserId?: string | null
  initiatorUserName?: string | null
  initiatorDepartmentId?: string | null
  initiatorDepartmentName?: string | null
  status: string
  startedAt: string
  durationMs?: number | null
  costMicros: number
  costCurrency?: string | null
}

type ExecutionPage = { items: ExecutionRow[]; limit: number; total: number; nextCursor?: string | null }
type FilterKey = 'applicationIds' | 'workflowIds' | 'toolIds' | 'initiatorUserIds' | 'initiatorDepartmentIds' | 'triggerTypes' | 'statuses' | 'errorCodes'
const filterKeys: FilterKey[] = ['applicationIds', 'workflowIds', 'toolIds', 'initiatorUserIds', 'initiatorDepartmentIds', 'triggerTypes', 'statuses', 'errorCodes']
const triggerTypes = ['user', 'api_key', 'webhook', 'schedule', 'poll', 'lifecycle', 'debug', 'evaluation', 'fork', 'composite']
const statuses = ['created', 'queued', 'running', 'waiting', 'waiting_approval', 'suspended', 'succeeded', 'failed', 'cancelled', 'timed_out']

export function ExecutionsPage() {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  const { showToast } = useToast()
  const [params, setParams] = useSearchParams()
  const navigationType = useNavigationType()
  const [search, setSearch] = useState(params.get('search') ?? '')
  const [triggerName, setTriggerName] = useState(params.get('triggerName') ?? '')
  const [cursorStack, setCursorStack] = useState<string[]>([''])
  const [cursorIndex, setCursorIndex] = useState(0)
  const [moreOpen, setMoreOpen] = useState(false)
  const [labels, setLabels] = useState<Record<string, string>>({})
  const filters = useMemo(() => Object.fromEntries(filterKeys.map((key) => [key, csv(params.get(key))])) as Record<FilterKey, string[]>, [params])
  const canonicalFilters = canonicalFilterString(params)
  const [cursorFilterKey, setCursorFilterKey] = useState(canonicalFilters)
  const cursor = cursorFilterKey === canonicalFilters ? cursorStack[cursorIndex] ?? '' : ''

  const resetCursor = () => { setCursorStack(['']); setCursorIndex(0) }
  const updateParam = (key: string, value?: string) => {
    setParams((current) => {
      const next = new URLSearchParams(current)
      if (value) next.set(key, value)
      else next.delete(key)
      return next
    }, { replace: true })
    resetCursor()
  }
  const updateMulti = (key: FilterKey, values: string[], selected?: MultiSelectOption) => {
    if (selected) setLabels((current) => ({ ...current, [selected.value]: selected.label }))
    updateParam(key, [...values].sort().join(','))
  }
  useEffect(() => {
    const timeout = window.setTimeout(() => {
      if ((params.get('search') ?? '') !== search.trim()) updateParam('search', search.trim())
    }, 300)
    return () => window.clearTimeout(timeout)
    // params changes are intentionally observed through the current value only.
  }, [search]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    const timeout = window.setTimeout(() => {
      if ((params.get('triggerName') ?? '') !== triggerName.trim()) updateParam('triggerName', triggerName.trim())
    }, 300)
    return () => window.clearTimeout(timeout)
  }, [triggerName]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    setCursorStack([''])
    setCursorIndex(0)
    setCursorFilterKey(canonicalFilters)
    if (navigationType === 'POP') {
      setSearch(params.get('search') ?? '')
      setTriggerName(params.get('triggerName') ?? '')
    }
  }, [canonicalFilters, navigationType]) // eslint-disable-line react-hooks/exhaustive-deps

  const requestParams = new URLSearchParams(canonicalFilters)
  requestParams.set('limit', '8')
  if (cursor) requestParams.set('cursor', cursor)
  const executions = useQuery({
    queryKey: ['executions', canonicalFilters, cursor],
    queryFn: () => apiRequest<ExecutionPage>(`/executions?${requestParams.toString()}`),
    retry: false,
  })
  useEffect(() => {
    if (executions.error instanceof ApiClientError && executions.error.detail.code === 'QUERY_CURSOR_EXPIRED') {
      resetCursor()
      showToast(t('executions.resultsRefreshed'))
    }
  }, [executions.error, showToast, t])

  const columns = useMemo<Array<ColumnDef<ExecutionRow>>>(() => [
    { accessorKey: 'id', header: t('executions.executionIdLabel'), cell: ({ row }) => <EntityCell detail={row.original.id} icon={Activity} name={row.original.workflowName} /> },
    { id: 'source', header: t('executions.source'), cell: ({ row }) => <div><p className="text-foreground">{row.original.applicationName ?? localizedValue(t, 'executions.triggerTypes', row.original.triggerType)}</p><p className="mt-0.5 text-[10px]">{row.original.applicationId ?? '—'}</p></div> },
    { accessorKey: 'workflowVersionNumber', header: t('common.version'), cell: ({ row }) => row.original.workflowVersionNumber == null ? '—' : `v${row.original.workflowVersionNumber}` },
    { accessorKey: 'triggerType', header: t('executions.trigger'), cell: ({ row }) => <div><p className="text-foreground">{localizedValue(t, 'executions.triggerTypes', row.original.triggerType)}</p>{row.original.triggerName && <p className="mt-0.5 text-[10px]">{row.original.triggerName}</p>}</div> },
    { id: 'initiator', header: t('executions.initiator'), cell: ({ row }) => row.original.initiatorUserName ? <div><p className="text-foreground">{row.original.initiatorUserName}</p><p className="mt-0.5 text-[10px]">{row.original.initiatorDepartmentName ?? '—'}</p></div> : '—' },
    { accessorKey: 'status', header: t('common.status'), cell: ({ row }) => <StatusBadge status={executionStatus(row.original.status)} /> },
    { accessorKey: 'startedAt', header: t('executions.startedAt'), cell: ({ row }) => formatDateTime(row.original.startedAt) },
    { accessorKey: 'durationMs', header: t('executions.duration'), cell: ({ row }) => row.original.durationMs == null ? '—' : `${row.original.durationMs} ms` },
    { accessorKey: 'costMicros', header: t('executions.cost'), cell: ({ row }) => formatCost(row.original.costMicros, row.original.costCurrency) },
    { id: 'actions', header: '', cell: ({ row }) => <Button asChild size="sm" variant="ghost"><Link to={`/executions/${row.original.id}`}>{t('executions.trace')}</Link></Button> },
  ], [formatDateTime, t])
  const table = useReactTable({ columns, data: executions.data?.items ?? [], getCoreRowModel: getCoreRowModel() })
  const chips = filterKeys.flatMap((key) => filters[key].map((value) => ({ key, value, label: labels[value] ?? (key === 'triggerTypes' ? localizedValue(t, 'executions.triggerTypes', value) : key === 'statuses' ? localizedValue(t, 'common', executionStatus(value)) : value) })))
  if (params.get('triggerName')) chips.push({ key: 'triggerTypes', value: `triggerName:${params.get('triggerName')}`, label: `${t('executions.triggerName')}: ${params.get('triggerName')}` })

  const loadPage = <T,>(path: string, map: (item: T) => MultiSelectOption) => async (query: string) => {
    const separator = path.includes('?') ? '&' : '?'
    const page = await apiRequest<PageResponse<T>>(`${path}${separator}pageSize=20${query ? `&search=${encodeURIComponent(query)}` : ''}`)
    return page.items.map(map)
  }
  const clearAll = () => { setParams(new URLSearchParams(), { replace: true }); setSearch(''); setTriggerName(''); setLabels({}); resetCursor() }
  const clearTriggerName = () => { setTriggerName(''); updateParam('triggerName') }
  const dateTimePickerMessages = {
    time: t('executions.dateTimePicker.time'),
    now: t('executions.dateTimePicker.now'),
    clear: t('executions.dateTimePicker.clear'),
    confirm: t('common.confirm'),
    invalidTime: t('executions.dateTimePicker.invalidTime'),
    outOfRange: t('executions.dateTimePicker.outOfRange'),
  }

  return <PageContainer>
    <PageHeader description={t('executions.description')} title={t('executions.title')} />
    <Card className="mt-6 overflow-hidden">
      <div className="flex min-h-16 flex-wrap items-center gap-2 border-b border-border px-5 py-3">
        <SearchField onChange={(event) => setSearch(event.target.value)} placeholder={t('executions.search')} value={search} />
        <SearchableMultiSelect label={t('common.status')} onChange={(value, selected) => updateMulti('statuses', value, selected)} options={statuses.map((value) => ({ value, label: localizedValue(t, 'common', executionStatus(value)) }))} queryKey="execution-status" value={filters.statuses} />
        <DateTimePicker label={t('executions.createdAfter')} max={isoDate(params.get('createdBefore'))} messages={dateTimePickerMessages} onChange={(value) => updateParam('createdAfter', value?.toISOString())} value={isoDate(params.get('createdAfter'))} />
        <DateTimePicker label={t('executions.createdBefore')} messages={dateTimePickerMessages} min={isoDate(params.get('createdAfter'))} onChange={(value) => updateParam('createdBefore', value?.toISOString())} value={isoDate(params.get('createdBefore'))} />
        <Popover.Root onOpenChange={setMoreOpen} open={moreOpen}><Popover.Trigger asChild><Button size="sm" variant="secondary"><Filter className="size-3.5" />{t('executions.moreFilters')}<ChevronDown className="size-3.5" /></Button></Popover.Trigger><Popover.Portal><Popover.Content align="end" className="z-[110] w-[620px] max-w-[calc(100vw-32px)] rounded-xl border border-border bg-surface p-4 shadow-xl" sideOffset={6}>
          <div className="grid grid-cols-2 gap-3">
            <FilterField label={t('executions.application')}><SearchableMultiSelect label={t('executions.selectApplication')} loadOptions={loadPage<Application>('/applications', (item) => ({ value: item.id, label: item.name }))} onChange={(value, selected) => updateMulti('applicationIds', value, selected)} queryKey="execution-applications" value={filters.applicationIds} /></FilterField>
            <FilterField label={t('executions.workflow')}><SearchableMultiSelect label={t('executions.selectWorkflow')} loadOptions={loadPage<Workflow>('/workflows', (item) => ({ value: item.id, label: item.name }))} onChange={(value, selected) => updateMulti('workflowIds', value, selected)} queryKey="execution-workflows" value={filters.workflowIds} /></FilterField>
            <FilterField label={t('executions.tool')}><SearchableMultiSelect label={t('executions.selectTool')} loadOptions={loadPage<McpTool>('/mcp/tools', (item) => ({ value: item.id, label: item.title ?? item.name, detail: item.name }))} onChange={(value, selected) => updateMulti('toolIds', value, selected)} queryKey="execution-tools" value={filters.toolIds} /></FilterField>
            <FilterField label={t('executions.user')}><SearchableMultiSelect label={t('executions.selectUser')} loadOptions={loadPage<User>('/users', (item) => ({ value: item.id, label: item.displayName, detail: item.username }))} onChange={(value, selected) => updateMulti('initiatorUserIds', value, selected)} queryKey="execution-users" value={filters.initiatorUserIds} /></FilterField>
            <FilterField label={t('executions.department')}><SearchableMultiSelect label={t('executions.selectDepartment')} loadOptions={loadPage<Department>('/departments/search', (item) => ({ value: item.id, label: item.name }))} onChange={(value, selected) => updateMulti('initiatorDepartmentIds', value, selected)} queryKey="execution-departments" value={filters.initiatorDepartmentIds} /></FilterField>
            <FilterField label={t('executions.trigger')}><SearchableMultiSelect label={t('executions.selectTriggerType')} onChange={(value, selected) => updateMulti('triggerTypes', value, selected)} options={triggerTypes.map((value) => ({ value, label: localizedValue(t, 'executions.triggerTypes', value) }))} queryKey="execution-trigger-types" value={filters.triggerTypes} /></FilterField>
            <FilterField label={t('executions.triggerName')}><Input aria-label={t('executions.triggerName')} maxLength={200} onChange={(event) => setTriggerName(event.target.value)} placeholder={t('executions.triggerNamePlaceholder')} value={triggerName} /></FilterField>
          </div>
        </Popover.Content></Popover.Portal></Popover.Root>
      </div>
      {chips.length > 0 && <div className="flex flex-wrap items-center gap-2 border-b border-border px-5 py-2.5">{chips.map((chip) => <button className="inline-flex h-7 items-center gap-1 rounded-full border border-border bg-muted/35 px-2.5 text-[11px] text-muted-foreground hover:text-foreground" key={`${chip.key}:${chip.value}`} onClick={() => chip.value.startsWith('triggerName:') ? clearTriggerName() : updateMulti(chip.key, filters[chip.key].filter((item) => item !== chip.value))} type="button">{chip.label}<X className="size-3" /></button>)}<Button onClick={clearAll} size="sm" variant="ghost">{t('executions.clearAll')}</Button></div>}
      {executions.isLoading ? <p className="p-8 text-center text-sm text-muted-foreground">{t('common.loading')}</p> : executions.isError ? <p className="p-8 text-center text-sm text-danger">{executions.error.message}</p> : table.getRowModel().rows.length === 0 ? <EmptyState /> : <TableContainer><Table><thead className="bg-muted/45 text-[10px] uppercase tracking-[0.08em] text-muted-foreground">{table.getHeaderGroups().map((group) => <tr key={group.id}>{group.headers.map((header) => <TableHead key={header.id}>{flexRender(header.column.columnDef.header, header.getContext())}</TableHead>)}</tr>)}</thead><tbody>{table.getRowModel().rows.map((row) => <tr className="border-t border-border transition-colors hover:bg-muted/30" key={row.id}>{row.getVisibleCells().map((cell) => <TableCell key={cell.id}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</TableCell>)}</tr>)}</tbody></Table></TableContainer>}
      <div className="flex min-h-15 items-center justify-between border-t border-border px-5"><span className="text-xs text-muted-foreground">{t('executions.total', { count: executions.data?.total ?? 0 })}</span><div className="flex gap-2"><Button disabled={cursorIndex === 0 || executions.isFetching} onClick={() => setCursorIndex((value) => Math.max(0, value - 1))} size="sm" variant="secondary">{t('common.previous')}</Button><Button disabled={!executions.data?.nextCursor || executions.isFetching} onClick={() => { const next = executions.data?.nextCursor; if (!next) return; setCursorStack((current) => [...current.slice(0, cursorIndex + 1), next]); setCursorIndex((value) => value + 1) }} size="sm" variant="secondary">{t('common.next')}</Button></div></div>
    </Card>
  </PageContainer>
}

function FilterField({ label, children }: { label: string; children: ReactNode }) { return <div className="space-y-1.5"><span className="block text-[11px] font-medium text-muted-foreground">{label}</span>{children}</div> }
function csv(value: string | null) { return value?.split(',').map((item) => item.trim()).filter(Boolean).sort() ?? [] }
function canonicalFilterString(params: URLSearchParams) {
  const canonical = new URLSearchParams()
  for (const key of filterKeys) {
    const values = [...new Set(csv(params.get(key)))]
    if (values.length > 0) canonical.set(key, values.join(','))
  }
  for (const key of ['search', 'triggerName', 'createdAfter', 'createdBefore']) {
    const value = params.get(key)?.trim()
    if (value) canonical.set(key, value)
  }
  return canonical.toString()
}
function isoDate(value: string | null) { if (!value) return undefined; const date = new Date(value); return Number.isNaN(date.getTime()) ? undefined : date }
