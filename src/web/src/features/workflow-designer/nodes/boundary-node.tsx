import { Position, type NodeProps } from '@xyflow/react'
import { Flag, Home } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { CanvasNode } from '../model/types'
import { boundaryGeometry } from './canvas-node-geometry'
import { CardShell } from './card-shell'
import { nodeGroupColor } from './node-appearance'
import { PortHandle } from './port-handle'

export function BoundaryNode({ data, selected }: NodeProps<CanvasNode>) {
  const { t } = useTranslation()
  if (data.editorKind !== 'boundary') return null
  const start = data.boundary === 'start'
  const geometry = boundaryGeometry(start ? 'start' : 'exit')
  const label = t(start ? 'studio.boundary.start' : 'studio.boundary.end')
  return <div className="studio-node studio-node-zoom-full relative shrink-0" data-testid={`workflow-${data.boundary}`} style={{ width: geometry.width, height: geometry.height }}>
    <CardShell group={nodeGroupColor(start ? 'start' : 'exit')} icon={start ? <Home className="size-3.5" /> : <Flag className="size-3.5" />} label={label} selected={selected}>
      <div className="studio-boundary-summary absolute inset-x-3 truncate text-xs text-muted-foreground" style={{ top: geometry.summaryTop }} title={t(start ? 'studio.boundary.startSummary' : 'studio.boundary.endSummary')}>{t(start ? 'studio.boundary.startSummary' : 'studio.boundary.endSummary')}</div>
    </CardShell>
    {geometry.ports.map(port => <PortHandle id={port.id} key={port.id} kind={port.kind} label={t(start ? 'studio.boundary.input' : port.kind === 'error' ? 'studio.boundary.error' : 'studio.boundary.output')} placement={{ position: port.side === 'left' ? Position.Left : Position.Right, axis: port.y }} row={port.kind !== 'error'} testId={`boundary-port-${port.id}`} type={port.direction === 'output' ? 'source' : 'target'} />)}
  </div>
}
