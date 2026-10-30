import type { ActionNodeData, CanvasNodeData, NodeManifest, PortKind, StudioNode } from '../model/types'
import { cardContent, visibleInputPorts, type CardContent } from './card-content'
import { LOOP_CONTAINER_DEFAULT_HEIGHT, LOOP_CONTAINER_DEFAULT_WIDTH, LOOP_CONTAINER_MIN_HEIGHT, LOOP_CONTAINER_MIN_WIDTH, LOOP_CONTAINER_PADDING } from '../utils/layout-constants'
import { canvasNodeMetrics, canvasNodeRole, STUDIO_BOUNDARY_HEIGHT, STUDIO_BOUNDARY_WIDTH, STUDIO_CARD_CONTENT_TOP, STUDIO_PORT_MAIN_Y, STUDIO_PORT_ERROR_Y, STUDIO_BRANCH_ROW_HEIGHT } from './node-appearance'

export type CanvasPortGeometry = { id: string; portName: string; kind: PortKind; direction: 'input' | 'output'; side: 'left' | 'right'; x: number; y: number; row?: number }
export type CanvasNodeGeometry = { width: number; height: number; ports: CanvasPortGeometry[]; rowCount: number; contentTop: number; summaryTop: number; attachmentTop: number }

/** Pixel coordinates are the authority for cards, handles, placement and layout. */
export function actionNodeGeometry(data: ActionNodeData, manifest?: NodeManifest, content: CardContent = cardContent(data, manifest)): CanvasNodeGeometry {
  const inputs = manifest ? visibleInputPorts(manifest, data.parameters) : []
  const outputs = manifest?.outputPorts ?? []
  const normalInputs = inputs.filter(port => port.kind !== 'error')
  const branchNames = new Set(content.branches.map(row => row.portName))
  if (data.nodeType === 'if') branchNames.add('case')
  if (data.nodeType === 'approval') branchNames.add('decision')
  const normalOutputs = outputs.filter(port => port.kind !== 'error' && !branchNames.has(port.name))
  const inputRows = normalInputs.length > 1 ? normalInputs.length : 0
  const outputRows = content.branches.length || (normalOutputs.length > 1 ? normalOutputs.length : 0)
  const rowCount = Math.max(inputRows, outputRows)
  const attachmentRows = content.attachments.length ? Math.ceil((content.attachments.length + 1) / 2) : 0
  const metrics = canvasNodeMetrics(canvasNodeRole(manifest), { bodyRows: content.summaries.length, inputRows, branchRows: outputRows, attachmentRows, wide: inputs.length > 1 || outputs.filter(port => port.kind !== 'error').length > 1 })
  const ports: CanvasPortGeometry[] = []
  const add = (id: string, portName: string, kind: PortKind, direction: 'input' | 'output', row?: number) => ports.push({ id, portName, kind, direction, side: direction === 'input' ? 'left' : 'right', x: direction === 'input' ? 0 : metrics.width, y: kind === 'error' ? STUDIO_PORT_ERROR_Y : row === undefined ? STUDIO_PORT_MAIN_Y : STUDIO_CARD_CONTENT_TOP + (row + .5) * STUDIO_BRANCH_ROW_HEIGHT, row })
  for (const input of inputs) add(input.name, input.name, input.kind, 'input', input.kind !== 'error' && inputRows ? normalInputs.indexOf(input) : undefined)
  content.branches.forEach((branch, index) => add(branch.handleId, branch.portName, 'main', 'output', index))
  for (const output of outputs) if (!branchNames.has(output.name)) add(output.name, output.name, output.kind, 'output', output.kind !== 'error' && normalOutputs.length > 1 ? normalOutputs.indexOf(output) : undefined)
  const summaryTop = STUDIO_CARD_CONTENT_TOP + rowCount * STUDIO_BRANCH_ROW_HEIGHT
  return { ...metrics, ports, rowCount, contentTop: STUDIO_CARD_CONTENT_TOP, summaryTop, attachmentTop: summaryTop + content.summaries.length * 20 }
}

export function boundaryGeometry(kind: 'start' | 'exit'): CanvasNodeGeometry {
  const output = kind === 'start'
  const ports: CanvasPortGeometry[] = [{ id: 'main', portName: 'main', kind: 'main', direction: output ? 'output' : 'input', side: output ? 'right' : 'left', x: output ? STUDIO_BOUNDARY_WIDTH : 0, y: STUDIO_PORT_MAIN_Y }]
  if (!output) ports.push({ id: 'error', portName: 'error', kind: 'error', direction: 'input', side: 'left', x: 0, y: STUDIO_PORT_ERROR_Y })
  return { width: STUDIO_BOUNDARY_WIDTH, height: STUDIO_BOUNDARY_HEIGHT, ports, rowCount: 0, contentTop: 48, summaryTop: 64, attachmentTop: 80 }
}

export function groupGeometry(): CanvasNodeGeometry {
  const metrics = canvasNodeMetrics('default')
  const ports = (['input', 'output'] as const).flatMap(direction => (['main', 'error'] as const).map(kind => ({
    id: `group-${kind === 'error' ? 'error-' : ''}${direction === 'input' ? 'in' : 'out'}`, portName: kind, kind, direction,
    side: direction === 'input' ? 'left' as const : 'right' as const, x: direction === 'input' ? 0 : metrics.width, y: kind === 'error' ? STUDIO_PORT_ERROR_Y : STUDIO_PORT_MAIN_Y,
  })))
  return { ...metrics, ports, rowCount: 0, contentTop: STUDIO_CARD_CONTENT_TOP, summaryTop: STUDIO_CARD_CONTENT_TOP, attachmentTop: metrics.height }
}

export function loopGeometry(width = LOOP_CONTAINER_DEFAULT_WIDTH, height = LOOP_CONTAINER_DEFAULT_HEIGHT): CanvasNodeGeometry {
  width = Math.max(LOOP_CONTAINER_MIN_WIDTH, width)
  height = Math.max(LOOP_CONTAINER_MIN_HEIGHT, height)
  return { width, height, rowCount: 0, contentTop: STUDIO_CARD_CONTENT_TOP, summaryTop: STUDIO_CARD_CONTENT_TOP, attachmentTop: height, ports: [
    { id: 'main', portName: 'main', kind: 'main', direction: 'input', side: 'left', x: 0, y: STUDIO_PORT_MAIN_Y },
    { id: 'main', portName: 'main', kind: 'main', direction: 'output', side: 'right', x: width, y: STUDIO_PORT_MAIN_Y },
    { id: 'error', portName: 'error', kind: 'error', direction: 'output', side: 'right', x: width, y: STUDIO_PORT_ERROR_Y },
  ] }
}

/** Container bounds include the visible content while retaining the saved minimum frame. */
export function loopGeometryForContent(size: { width?: number; height?: number }, content: Array<{ x: number; y: number; width: number; height: number }>): CanvasNodeGeometry {
  const base = loopGeometry(size.width, size.height)
  return loopGeometry(Math.max(base.width, ...content.map(rect => rect.x + rect.width + LOOP_CONTAINER_PADDING.right)), Math.max(base.height, ...content.map(rect => rect.y + rect.height + LOOP_CONTAINER_PADDING.bottom)))
}

export function studioNodeGeometry(node: Pick<StudioNode, 'data'> & Partial<Pick<StudioNode, 'id' | 'width' | 'height'>>, manifest?: NodeManifest): CanvasNodeGeometry {
  if (node.id === '__start__') return boundaryGeometry('start')
  if (node.data.editorKind === 'exit') return boundaryGeometry('exit')
  if (node.data.nodeType === 'loop_over_items') return loopGeometry(node.width, node.height)
  return actionNodeGeometry(node.data, manifest)
}

export function canvasGeometry(data: CanvasNodeData, manifest?: NodeManifest): CanvasNodeGeometry {
  if (data.editorKind === 'action') return data.nodeType === 'loop_over_items' ? loopGeometry() : actionNodeGeometry(data, manifest)
  if (data.editorKind === 'exit' || data.editorKind === 'iteration-end') return boundaryGeometry('exit')
  if (data.editorKind === 'boundary' || data.editorKind === 'iteration-chip') return boundaryGeometry('start')
  if (data.editorKind === 'loop-container') return loopGeometry()
  if (data.editorKind === 'group') return groupGeometry()
  return { ...canvasNodeMetrics('default'), ports: [], rowCount: 0, contentTop: STUDIO_CARD_CONTENT_TOP, summaryTop: STUDIO_CARD_CONTENT_TOP, attachmentTop: STUDIO_CARD_CONTENT_TOP }
}
