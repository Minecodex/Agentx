import type { EditorDocument, NodeManifest, StudioNode } from '../model/types'
import { groupGeometry, studioNodeGeometry } from '../nodes/canvas-node-geometry'
import { manifestForNode } from './manifest-lookup'
import { LOOP_CONTAINER_PADDING } from './layout-constants'

export type GroupView = { id: string; group: EditorDocument['groups'][number]; position: { x: number; y: number }; width: number; height: number; parentId?: string }

/** The same proxy anchor and dimensions feed rendering and ELK. */
export function computeGroupView(group: EditorDocument['groups'][number], nodeMap: Map<string, StudioNode>, manifests = new Map<string, NodeManifest>()): GroupView | undefined {
  const members = group.nodeIds.flatMap(id => { const node = nodeMap.get(id); return node ? [node] : [] })
  if (!members.length) return undefined
  const absolute = (node: StudioNode) => {
    const parentId = node.data.editorKind === 'action' ? node.data.parentId : undefined
    const parent = parentId ? nodeMap.get(parentId) : undefined
    return parent ? { x: parent.position.x + node.position.x, y: parent.position.y + node.position.y } : node.position
  }
  const bounds = members.map(node => ({ position: absolute(node), metrics: studioNodeGeometry(node, manifestForNode(manifests, node)) }))
  const left = Math.min(...bounds.map(({ position }) => position.x)), top = Math.min(...bounds.map(({ position }) => position.y))
  const right = Math.max(...bounds.map(({ position, metrics }) => position.x + metrics.width)), bottom = Math.max(...bounds.map(({ position, metrics }) => position.y + metrics.height))
  const parents = new Set(members.map(node => node.data.editorKind === 'action' ? node.data.parentId : undefined))
  const owner = parents.size === 1 ? nodeMap.get([...parents][0] ?? '') : undefined
  const geometry = groupGeometry()
  const x = left - 32 - (owner?.position.x ?? 0), y = top - 48 - (owner?.position.y ?? 0)
  const position = group.collapsed && owner ? { x: Math.max(LOOP_CONTAINER_PADDING.left, x), y: Math.max(LOOP_CONTAINER_PADDING.top, y) } : { x, y }
  return { id: `group:${group.id}`, group, parentId: owner?.id, position, width: group.collapsed ? geometry.width : right - left + 64, height: group.collapsed ? geometry.height : bottom - top + 80 }
}
