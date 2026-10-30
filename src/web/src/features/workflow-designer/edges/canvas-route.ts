export type CanvasPoint = { x: number; y: number }
export type CanvasRect = CanvasPoint & { width: number; height: number; id?: string }
export type CanvasRoute = { status: 'ready' | 'pending' | 'blocked'; points: CanvasPoint[]; path: string; label: CanvasPoint; bounds: CanvasRect }
export const ROUTE_RADIUS = 12
export const ROUTE_PADDING = 16
// Two grid cells keep independently rounded channels at least 12px apart.
export const ROUTE_CHANNEL_GAP = 16

/** Library grid endpoints can have a sub-cell diagonal after waypoint stitching.
 * Normalize those drawing joins only; obstacle search stays in the library. */
export function orthogonalRoutePoints(points: CanvasPoint[], gridSize = 8): CanvasPoint[] {
  const result: CanvasPoint[] = []
  for (const point of points) {
    const previous = result.at(-1)
    if (previous && point.x !== previous.x && point.y !== previous.y && Math.min(Math.abs(point.x - previous.x), Math.abs(point.y - previous.y)) <= gridSize) {
      result.push(Math.abs(point.x - previous.x) >= Math.abs(point.y - previous.y) ? { x: point.x, y: previous.y } : { x: previous.x, y: point.y })
    }
    result.push(point)
  }
  return result
}

export function routeFromPoints(points: CanvasPoint[], status: CanvasRoute['status'] = 'ready'): CanvasRoute {
  const clean = points.filter((point, i) => !i || point.x !== points[i - 1].x || point.y !== points[i - 1].y)
    .filter((point, i, values) => !i || i === values.length - 1 || !((values[i - 1].x === point.x && point.x === values[i + 1].x) || (values[i - 1].y === point.y && point.y === values[i + 1].y)))
  let path = clean.length ? `M ${clean[0].x} ${clean[0].y}` : ''
  for (let i = 1; i < clean.length - 1; i++) {
    const a = clean[i - 1], b = clean[i], c = clean[i + 1]
    const first = Math.hypot(b.x - a.x, b.y - a.y), second = Math.hypot(c.x - b.x, c.y - b.y)
    const r = Math.min(ROUTE_RADIUS, first / 2, second / 2)
    path += ` L ${b.x - (b.x - a.x) * r / first} ${b.y - (b.y - a.y) * r / first} Q ${b.x} ${b.y} ${b.x + (c.x - b.x) * r / second} ${b.y + (c.y - b.y) * r / second}`
  }
  if (clean.length > 1) path += ` L ${clean.at(-1)!.x} ${clean.at(-1)!.y}`
  let length = -1
  let label = clean[0] ?? { x: 0, y: 0 }
  for (let i = 1; i < clean.length; i++) {
    const a = clean[i - 1], b = clean[i]
    const segment = Math.hypot(b.x - a.x, b.y - a.y)
    if (segment > length && (a.y === b.y || length < 0)) { length = segment; label = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 } }
  }
  const xs = clean.map(p => p.x), ys = clean.map(p => p.y)
  const x = xs.length ? Math.min(...xs) : 0, y = ys.length ? Math.min(...ys) : 0
  return { status, points: clean, path, label, bounds: { x, y, width: xs.length ? Math.max(...xs) - x : 0, height: ys.length ? Math.max(...ys) - y : 0 } }
}

/** Geometry assertions also guard library failure paths from becoming committed routes. */
export function routeIsClear(points: CanvasPoint[], obstacles: CanvasRect[], domain?: CanvasRect) {
  if (points.length < 2 || points.some(point => !Number.isFinite(point.x) || !Number.isFinite(point.y))) return false
  if (domain && points.some(point => point.x < domain.x || point.y < domain.y || point.x > domain.x + domain.width || point.y > domain.y + domain.height)) return false
  for (let i = 1; i < points.length; i++) {
    const a = points[i - 1], b = points[i]
    if (Math.abs(a.x - b.x) > .1 && Math.abs(a.y - b.y) > .1) return false
    for (const box of obstacles) {
      if (a.x === b.x && a.x > box.x + .1 && a.x < box.x + box.width - .1 && Math.max(a.y, b.y) > box.y + .1 && Math.min(a.y, b.y) < box.y + box.height - .1) return false
      if (a.y === b.y && a.y > box.y + .1 && a.y < box.y + box.height - .1 && Math.max(a.x, b.x) > box.x + .1 && Math.min(a.x, b.x) < box.x + box.width - .1) return false
    }
  }
  return true
}

export function temporaryRoute(source: CanvasPoint, target: CanvasPoint) {
  const mid = source.x + Math.max(24, (target.x - source.x) / 2)
  return routeFromPoints([source, { x: mid, y: source.y }, { x: mid, y: target.y }, target], 'pending')
}

/** A screen-sized toolbar must fit on a real segment without covering cards. */
export function routeToolbarPoint(points: CanvasPoint[], obstacles: CanvasRect[], zoom: number): CanvasPoint | undefined {
  const width = 60 / zoom, height = 36 / zoom
  const segments = points.slice(1).map((end, i) => ({ start: points[i], end, length: Math.hypot(end.x - points[i].x, end.y - points[i].y) })).sort((a, b) => b.length - a.length)
  for (const { start, end, length } of segments) {
    const horizontal = start.y === end.y
    if (length < (horizontal ? width : height)) continue
    const inset = (horizontal ? width : height) / 2 / length
    for (const fraction of [.5, Math.min(1 - inset, .8), Math.max(inset, .2)]) {
      const point = { x: start.x + (end.x - start.x) * fraction, y: start.y + (end.y - start.y) * fraction }
      const box = { x: point.x - width / 2, y: point.y - height / 2, width, height }
      if (obstacles.every(other => box.x + box.width <= other.x || box.x >= other.x + other.width || box.y + box.height <= other.y || box.y >= other.y + other.height)) return point
    }
  }
  return undefined
}
