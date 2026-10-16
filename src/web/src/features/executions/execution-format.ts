export function executionStatus(status: string): 'success' | 'running' | 'waiting' | 'failed' | 'pending' | 'inactive' {
  if (status === 'succeeded') return 'success'
  if (status === 'running') return 'running'
  if (status.startsWith('waiting') || status === 'suspended') return 'waiting'
  if (status === 'failed' || status === 'timed_out') return 'failed'
  if (status === 'cancelled') return 'inactive'
  return 'pending'
}

export const formatCost = formatCurrencyCost
import { formatCost as formatCurrencyCost } from '../../shared/lib/cost-format'

export function designSourceLabel(source: {kind: 'draft_revision'; draftId: string; revision: number} | {kind: 'version'; versionNumber: number} | undefined, version: number | null | undefined, draftLabel: (revision: number, draft: string) => string) {
  return source?.kind === 'draft_revision' ? draftLabel(source.revision, source.draftId.slice(-8)) : `v${source?.kind === 'version' ? source.versionNumber : version ?? '—'}`
}
