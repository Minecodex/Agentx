import type { Knowledge } from '../../shared/api/types'

type Scope = Knowledge['scope']
export function scopeLabel(scope: Scope, defaultWorkspace: string) {
  return scope.kind === 'ragflow' ? scope.datasetIds.join(', ') : scope.workspace || defaultWorkspace
}
