import type { ActionNodeData, InputBinding, NodeManifest } from '../model/types'

type Translate = (key: string, options?: Record<string, unknown>) => string
export type CardBinding = { resourceType: string; label?: string }
export type CardSummary = { text: string; badge?: string; mono?: boolean }
export type CardBranch = { handleId: string; portName: string; tag: string; tone: 'logic' | 'blue' | 'muted'; label: string; sub?: string; badge?: string }
export type CardContent = { summaries: CardSummary[]; branches: CardBranch[]; attachments: Array<{ resourceType: string; count: number }> }

/** Rendering and geometry share this projection, including all dynamic row counts. */
export function cardContent(data: ActionNodeData, manifest?: NodeManifest, bindings: CardBinding[] = data.resourceReferences.map(reference => ({ resourceType: reference.resourceType, label: reference.resourceId })), t: Translate = key => key): CardContent {
  const p = data.parameters
  const summaries: CardSummary[] = []
  const push = (text: string, extra?: Omit<CardSummary, 'text'>) => summaries.push({ text, ...extra })
  switch (manifest?.nodeType ?? data.nodeType) {
    case 'declarative_http': push(dynamicText(p.url) ?? '', { badge: typeof p.method === 'string' ? p.method : 'GET', mono: true }); break
    case 'code': {
      const runner = typeof p.runner === 'string' ? ({ python: 'Python', javascript: 'JavaScript', shell: 'Shell' }[p.runner] ?? p.runner) : ''
      const outputs = p.outputExample && typeof p.outputExample === 'object' && !Array.isArray(p.outputExample) ? Object.keys(p.outputExample) : []
      push([runner, outputs.length ? `${t('studio.card.outputs')}: ${outputs.slice(0, 3).join(', ')}` : ''].filter(Boolean).join(' · '))
      break
    }
    case 'model':
    case 'agent': {
      const model = bindings.find(binding => binding.resourceType === 'model')?.label ?? data.resourceReferences.find(reference => reference.resourceType === 'model')?.resourceId
      if (model) push(model)
      if (data.nodeType === 'model') push(t('studio.card.modelMode', { mode: t(`studio.card.modelModes.${p.responseMode === 'json_schema' ? 'json_schema' : 'text'}`) }))
      if (data.nodeType === 'agent' && typeof p.maxIterations === 'number') push(t('studio.card.maxIterations', { count: p.maxIterations }))
      break
    }
    case 'set': {
      const values = p.values as { kind?: unknown; fields?: unknown } | undefined
      push(t('studio.card.setFields', { count: values?.kind === 'object' && values.fields && typeof values.fields === 'object' ? Object.keys(values.fields).length : 0 }))
      break
    }
    case 'merge': push(t('studio.card.mergeMode', { mode: t(`studio.card.mergeModes.${['combine_by_position', 'combine_by_key'].includes(String(p.mode)) ? p.mode : 'append'}`) })); break
    case 'list': {
      const filter = p.filter as { conditions?: unknown[] } | undefined
      push(t('studio.card.listRules', { filters: Array.isArray(filter?.conditions) ? filter.conditions.length : 0, sorts: Array.isArray(p.sort) ? p.sort.length : 0 }))
      if (typeof p.takeN === 'number') push(t('studio.card.takeN', { count: p.takeN }))
      break
    }
    case 'sub_workflow': if (typeof p.workflowVersionId === 'string') push(t('studio.card.version', { value: p.workflowVersionId.slice(0, 8) })); break
    default:
      if (manifest?.plugin) {
        const properties = (manifest.parameterSchema.properties ?? {}) as Record<string, { title?: string }>
        for (const [key, value] of Object.entries(p)) if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') push(`${manifest.uiSchema.fields?.[key]?.label ?? properties[key]?.title ?? key}: ${String(value)}`)
      }
      break
    case 'loop_over_items':
      push(t(`studio.container.errorMode.${['continue', 'remove'].includes(String(p.errorMode)) ? p.errorMode : 'terminate'}`))
      push(t('studio.container.parallelChip', { count: typeof p.parallelism === 'number' ? p.parallelism : 10 }))
      break
  }
  const branches = manifest ? branchContent(manifest, p, t) : []
  const counts = new Map<string, number>()
  if (data.nodeType === 'agent') for (const binding of bindings) counts.set(binding.resourceType, (counts.get(binding.resourceType) ?? 0) + 1)
  return { summaries: branches.length ? [] : summaries.slice(0, 3), branches, attachments: [...counts].map(([resourceType, count]) => ({ resourceType, count })) }
}

export function visibleInputPorts(manifest: NodeManifest, parameters: Record<string, unknown>) {
  if (manifest.nodeType !== 'merge') return manifest.inputPorts
  const names = parameters.mode === 'combine_by_position' || parameters.mode === 'combine_by_key' ? new Set(['left', 'right']) : new Set(['main'])
  return manifest.inputPorts.filter(port => names.has(port.name))
}

function branchContent(manifest: NodeManifest, parameters: Record<string, unknown>, t: Translate): CardBranch[] {
  const rows: CardBranch[] = []
  const has = (name: string) => manifest.outputPorts.some(port => port.name === name)
  if (manifest.nodeType === 'if' && has('case') && Array.isArray(parameters.cases)) {
    parameters.cases.forEach((raw, index) => {
      const item = raw as { id?: string; logicalOp?: string; conditions?: Array<{ label?: string }> }
      if (!item?.id) return
      const labels = Array.isArray(item.conditions) ? item.conditions.map((condition, i) => (typeof condition?.label === 'string' ? condition.label.trim() : '') || t('studio.card.conditionN', { index: i + 1 })) : []
      rows.push({ handleId: `case:${item.id}`, portName: 'case', tag: t(index ? 'studio.card.elifTag' : 'studio.card.ifTag'), tone: 'logic', label: labels.join(', ') || t('studio.card.conditionN', { index: 1 }), badge: labels.length > 1 ? t(item.logicalOp === 'or' ? 'studio.card.or' : 'studio.card.and') : undefined })
    })
  }
  if (manifest.nodeType === 'if' && has('else')) rows.push({ handleId: 'else', portName: 'else', tag: t('studio.card.elseTag'), tone: 'muted', label: '', sub: t('studio.card.elseFallback') })
  if (manifest.nodeType === 'approval' && has('decision')) {
    const buttons = Array.isArray(parameters.buttons) ? parameters.buttons as Array<{ id: string; label?: string }> : [{ id: 'approved' }, { id: 'rejected' }]
    for (const button of buttons) if (button?.id) rows.push({ handleId: `decision:${button.id}`, portName: 'decision', tag: t('studio.card.buttonTag'), tone: 'blue', label: button.label || (['approved', 'rejected'].includes(button.id) ? t(button.id === 'approved' ? 'studio.card.approveDefault' : 'studio.card.rejectDefault') : button.id) })
    if (has('timed_out')) rows.push({ handleId: 'timed_out', portName: 'timed_out', tag: t('studio.card.timeoutRow'), tone: 'muted', label: '', sub: typeof parameters.timeoutMs === 'number' ? t('studio.card.timeoutHours', { hours: Math.round(parameters.timeoutMs / 36_000) / 100 }) : undefined })
  }
  return rows
}

function dynamicText(value: unknown): string | undefined {
  if (typeof value === 'string') return value
  if (!value || typeof value !== 'object' || !('kind' in value)) return undefined
  const binding = value as InputBinding
  if (binding.kind === 'literal') return binding.value == null ? undefined : String(binding.value)
  if (binding.kind === 'template') return binding.segments.map(segment => segment.kind === 'text' ? segment.text : '{{…}}').join('')
  return undefined
}
