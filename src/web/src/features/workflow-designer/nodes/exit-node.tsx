import { Position, type NodeProps } from '@xyflow/react'
import { Flag, Lock } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { localizedExitLabel } from '../model/node-display'
import type { CanvasNode } from '../model/types'
import { boundaryGeometry } from './canvas-node-geometry'
import { CardShell } from './card-shell'
import { nodeGroupColor } from './node-appearance'
import { PortHandle } from './port-handle'

export function ExitNode({ data, selected }: NodeProps<CanvasNode>) {
  const { t } = useTranslation()
  if (data.editorKind !== 'exit') return null
  const geometry = boundaryGeometry('exit')
  const outputs = Object.keys(data.parameters.outputs).length
  const summary = `${t('studio.exit.summary')}${outputs ? ` ×${outputs}` : ''}`
  return <div className="studio-node studio-node-zoom-full relative shrink-0" data-testid={`exit-node-${data.key}`} style={{ width: geometry.width, height: geometry.height }}>
    <CardShell group={nodeGroupColor('exit')} icon={<Flag className="size-3.5" />} label={localizedExitLabel(data.label, t('studio.exit.defaultName'))} selected={selected} trailing={data.protected ? <Lock aria-label={t('studio.exit.protected')} className="size-3 shrink-0 text-muted-foreground" data-testid="exit-protected" /> : undefined}>
      <div className="studio-boundary-summary absolute inset-x-3 truncate text-xs text-muted-foreground" style={{ top: geometry.summaryTop }} title={summary}>{summary}</div>
    </CardShell>
    {geometry.ports.map(port => <PortHandle id={port.id} key={port.id} kind={port.kind} label={t(port.kind === 'error' ? 'studio.boundary.error' : 'studio.boundary.output')} placement={{ position: Position.Left, axis: port.y }} row={port.kind !== 'error'} testId={`exit-port-${port.id}`} type="target" />)}
  </div>
}
