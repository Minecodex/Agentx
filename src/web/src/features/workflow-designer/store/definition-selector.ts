import type { StudioDocument } from '../model/types'

/** Definition consumers must not rebuild catalogs or plugin contracts on drag frames. */
export function createDefinitionSelector() {
  let previous: StudioDocument | undefined
  return (state: StudioDocument): StudioDocument => {
    if (previous && previous.start === state.start && previous.end === state.end &&
      previous.settings === state.settings && previous.edges === state.edges &&
      previous.nodes.length === state.nodes.length && state.nodes.every((node, index) => {
        const prior = previous!.nodes[index]
        return prior.id === node.id && prior.type === node.type && prior.data === node.data
      })) return previous
    previous = {
      start: state.start, nodes: state.nodes, edges: state.edges, end: state.end,
      settings: state.settings, viewport: state.viewport, boundaryLayouts: state.boundaryLayouts,
      annotations: state.annotations, groups: state.groups,
    }
    return previous
  }
}
