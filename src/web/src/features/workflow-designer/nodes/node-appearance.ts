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

/** Semantic visual groups driving the node card icon color. */
export type NodeVisualGroup = 'start' | 'ai' | 'logic' | 'transform' | 'integrate' | 'output'

const VISUAL_GROUPS: Record<string, NodeVisualGroup> = {
  start: 'start', boundary: 'start',
  agent: 'ai', model: 'ai', knowledge_retrieval: 'ai',
  if: 'logic', merge: 'logic', loop_over_items: 'logic', approval: 'logic',
  code: 'transform', set: 'transform', list: 'transform',
  declarative_http: 'integrate', sub_workflow: 'integrate', reply_message: 'integrate', send_message: 'integrate',
  exit: 'output',
}

const GROUP_COLORS: Record<NodeVisualGroup, string> = {
  start: 'var(--studio-group-start)',
  ai: 'var(--studio-group-ai)',
  logic: 'var(--studio-group-logic)',
  transform: 'var(--studio-group-transform)',
  integrate: 'var(--studio-group-integrate)',
  output: 'var(--studio-group-output)',
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
  /** Number of summary rows rendered inside the card body (0-3). */
  bodyRows?: number
  /** Branch rows rendered as 32px output rows (if cases / approval buttons). */
  branchRows?: number
  /** Stacked 32px input rows rendered on the body left edge (multi-input merge). */
  inputRows?: number
  attachmentRows?: number
  wide?: boolean
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

export const STUDIO_CARD_WIDTH = 240
export const STUDIO_BOUNDARY_WIDTH = 184
export const STUDIO_BOUNDARY_HEIGHT = 80
export const STUDIO_CARD_HEAD_HEIGHT = 48
export const STUDIO_CARD_CONTENT_TOP = 68
export const STUDIO_PORT_MAIN_Y = 24
export const STUDIO_PORT_ERROR_Y = 52
export const STUDIO_CARD_ROW_HEIGHT = 20
export const STUDIO_CARD_ATTACHMENT_HEIGHT = 24
export const STUDIO_BRANCH_ROW_HEIGHT = 32

export function canvasNodeMetrics(role: CanvasNodeRole, ports: CanvasNodePorts = {}) {
  const rows = Math.max(0, Math.min(3, ports.bodyRows ?? 1))
  const height = Math.ceil(Math.max(104,
    STUDIO_CARD_CONTENT_TOP + 12
    + rows * STUDIO_CARD_ROW_HEIGHT
    + Math.max(ports.branchRows ?? 0, ports.inputRows ?? 0) * STUDIO_BRANCH_ROW_HEIGHT
    + (ports.attachmentRows ?? 0) * STUDIO_CARD_ATTACHMENT_HEIGHT,
  ) / 8) * 8
  const wide = ports.wide || ['agent', 'branch', 'approval'].includes(role) || (ports.inputRows ?? 0) > 1
  return { width: wide ? 280 : STUDIO_CARD_WIDTH, height }
}
