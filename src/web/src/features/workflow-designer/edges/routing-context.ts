import { createContext, useContext } from 'react'
import type { RoutingWorld } from './routing-world'

export const RoutingContext = createContext<RoutingWorld | null>(null)
export function useRoutingWorld() {
  const world = useContext(RoutingContext)
  if (!world) throw new Error('Canvas routing provider is required')
  return world
}
