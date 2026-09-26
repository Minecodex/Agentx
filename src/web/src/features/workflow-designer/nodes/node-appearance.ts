import type { CanvasNodeFamily, CanvasNodeRole, NodeManifest } from '../model/types'

export function canvasNodeRole(manifest?: NodeManifest): CanvasNodeRole {
  return manifest?.uiSchema.canvas?.role ?? 'default'
}

export function nodeShape(manifest?: NodeManifest) {
  return canvasNodeRole(manifest)
}

export function canvasNodeFamily(role: CanvasNodeRole): CanvasNodeFamily {
  return role === 'agent' ? 'agent' : 'compact'
}

/** Dify-style visual groups driving the node card icon color. */
export type NodeVisualGroup = 'start' | 'ai' | 'logic' | 'transform' | 'integrate' | 'output'

const VISUAL_GROUPS: Record<string, NodeVisualGroup> = {
  start: 'start', boundary: 'start',
  agent: 'ai', model: 'ai',
  if: 'logic', merge: 'logic', loop_over_items: 'logic', approval: 'logic',
  code: 'transform', set: 'transform', list: 'transform',
  declarative_http: 'integrate', sub_workflow: 'integrate', reply_message: 'integrate', send_message: 'integrate',
  exit: 'output',
}

const GROUP_COLORS: Record<NodeVisualGroup, string> = {
  start: '#155EEF',
  ai: '#6366F1',
  logic: '#06B6D4',
  transform: '#3B82F6',
  integrate: '#8B5CF6',
  output: '#F59E0B',
}

export function nodeGroup(nodeType: string): NodeVisualGroup {
  return VISUAL_GROUPS[nodeType] ?? 'integrate'
}

export function groupColor(group: NodeVisualGroup): string {
  return GROUP_COLORS[group]
}

export function nodeGroupColor(nodeType: string): string {
  return groupColor(nodeGroup(nodeType))
}

export type CanvasNodePorts = {
  richHeight?: number
  /** Number of summary rows rendered inside the card body (0-3). */
  bodyRows?: number
  /** Branch rows rendered as 34px output rows (if cases / approval buttons). */
  branchRows?: number
  /** Stacked 26px input rows rendered on the body left edge (multi-input merge). */
  inputRows?: number
  /** Whether the card renders an attachment chip row. */
  attachments?: boolean
  kind?: 'action' | 'group'
}

export type CanvasPlacementRect = { x: number; y: number; width: number; height: number }

const PLACEMENT_GAP = 40

export function findOpenCanvasPosition(anchor: { x: number; y: number }, size: { width: number; height: number }, occupied: CanvasPlacementRect[], bounds?: CanvasPlacementRect) {
  const origin = { x: anchor.x - size.width / 2, y: anchor.y - size.height / 2 }
  const stepX = Math.max(160, size.width + 72)
  const stepY = Math.max(152, size.height + 64)
  const candidates = [origin]
  for (let radius = 1; radius <= 8; radius += 1) {
    candidates.push(
      { x: origin.x + radius * stepX, y: origin.y },
      { x: origin.x, y: origin.y + radius * stepY },
      { x: origin.x - radius * stepX, y: origin.y },
      { x: origin.x, y: origin.y - radius * stepY },
    )
    for (let offset = 1; offset < radius; offset += 1) {
      candidates.push(
        { x: origin.x + radius * stepX, y: origin.y + offset * stepY },
        { x: origin.x + offset * stepX, y: origin.y + radius * stepY },
        { x: origin.x - radius * stepX, y: origin.y - offset * stepY },
        { x: origin.x - offset * stepX, y: origin.y - radius * stepY },
      )
    }
  }
  const available = candidates.find((candidate) => (!bounds || rectangleInsideBounds({ ...candidate, ...size }, bounds)) && occupied.every((rect) => !rectanglesOverlap(
    { ...candidate, width: size.width, height: size.height },
    rect,
  )))
  if (available) return available
  if (!bounds) return candidates.at(-1)!
  const minX = bounds.x + PLACEMENT_GAP
  const minY = bounds.y + PLACEMENT_GAP
  const maxX = bounds.x + bounds.width - size.width - PLACEMENT_GAP
  const maxY = bounds.y + bounds.height - size.height - PLACEMENT_GAP
  const visibleCandidates = []
  for (let y = minY; y <= maxY; y += 32) {
    for (let x = minX; x <= maxX; x += 32) visibleCandidates.push({ x, y })
  }
  visibleCandidates.sort((left, right) => squaredDistance(left, origin) - squaredDistance(right, origin) || left.y - right.y || left.x - right.x)
  const visible = visibleCandidates.find((candidate) => occupied.every((rect) => !rectanglesOverlap({ ...candidate, ...size }, rect)))
  if (visible) return visible
  return {
    x: Math.min(Math.max(origin.x, minX), maxX),
    y: Math.min(Math.max(origin.y, minY), maxY),
  }
}

function squaredDistance(left: { x: number; y: number }, right: { x: number; y: number }) {
  return (left.x - right.x) ** 2 + (left.y - right.y) ** 2
}

function rectanglesOverlap(left: CanvasPlacementRect, right: CanvasPlacementRect) {
  return left.x < right.x + right.width + PLACEMENT_GAP
    && left.x + left.width + PLACEMENT_GAP > right.x
    && left.y < right.y + right.height + PLACEMENT_GAP
    && left.y + left.height + PLACEMENT_GAP > right.y
}

function rectangleInsideBounds(rect: CanvasPlacementRect, bounds: CanvasPlacementRect) {
  return rect.x >= bounds.x + PLACEMENT_GAP
    && rect.y >= bounds.y + PLACEMENT_GAP
    && rect.x + rect.width <= bounds.x + bounds.width - PLACEMENT_GAP
    && rect.y + rect.height <= bounds.y + bounds.height - PLACEMENT_GAP
}

/** Card geometry: 240px wide white card, height = head 44 + rows x 18 + attachment row 24. */
export const STUDIO_CARD_WIDTH = 240
export const STUDIO_CARD_HEAD_HEIGHT = 44
export const STUDIO_CARD_ROW_HEIGHT = 18
export const STUDIO_CARD_ATTACHMENT_HEIGHT = 24
/** Branch rows carry their own right-edge handles: 34px per row (demo2 .n-row). */
export const STUDIO_BRANCH_ROW_HEIGHT = 34
/** Stacked input rows carry left-edge handles: 26px per row (demo2 merge inputs). */
export const STUDIO_INPUT_ROW_HEIGHT = 26

export function canvasNodeMetrics(_role: CanvasNodeRole, ports: CanvasNodePorts = {}, collapsed = false) {
  if (ports.kind === 'group') return collapsed
    ? { width: 240, height: 64, labelBelow: false }
    : { width: 240, height: 160, labelBelow: false }
  const rows = Math.max(0, Math.min(3, ports.bodyRows ?? 1))
  const height = Math.ceil(
    STUDIO_CARD_HEAD_HEIGHT
    + rows * STUDIO_CARD_ROW_HEIGHT
    + (ports.branchRows ?? 0) * STUDIO_BRANCH_ROW_HEIGHT
    + (ports.inputRows ?? 0) * STUDIO_INPUT_ROW_HEIGHT
    + (ports.attachments ? STUDIO_CARD_ATTACHMENT_HEIGHT : 0),
  )
  return { width: STUDIO_CARD_WIDTH, height, labelBelow: false }
}
