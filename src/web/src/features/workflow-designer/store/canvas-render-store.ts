import { createStore, useStore } from 'zustand'
import { createContext, createElement, useContext, useState, type ReactNode } from 'react'

import type { NodeManifest } from '../model/types'
import type { NodeBindingSummary } from '../utils/graph-index'

export type CanvasZoomTier = 'full' | 'compact' | 'minimal'

type CanvasRenderState = {
  manifests: Map<string, NodeManifest>
  runtimeStatuses: Map<string, string>
  bindingSummaries: Map<string, NodeBindingSummary[]>
  occupiedHandlesByNodeId: Map<string, string>
  zoomTier: CanvasZoomTier
  onQuickAdd?: (nodeId: string, handleId: string, manifestPortName?: string) => void
  onSourceHover?: (nodeId: string, handleId: string, active: boolean) => void
}

export const createCanvasRenderStore = () => createStore<CanvasRenderState>(() => ({
  manifests: new Map(), runtimeStatuses: new Map(), bindingSummaries: new Map(), occupiedHandlesByNodeId: new Map(), zoomTier: 'full',
}))

const CanvasRenderContext = createContext<ReturnType<typeof createCanvasRenderStore> | null>(null)
export function CanvasRenderProvider({ children, store: supplied }: { children: ReactNode; store?: ReturnType<typeof createCanvasRenderStore> }) {
  const [store] = useState(() => supplied ?? createCanvasRenderStore())
  return createElement(CanvasRenderContext.Provider, {value: store}, children)
}
export function useCanvasRenderStoreApi() {
  const store = useContext(CanvasRenderContext)
  if (!store) throw new Error('A canvas render provider is required')
  return store
}
export function useCanvasRenderStore<T>(selector: (state: CanvasRenderState) => T) {
  return useStore(useCanvasRenderStoreApi(), selector)
}

export const canvasZoomTier = (zoom: number): CanvasZoomTier => zoom < 0.25 ? 'minimal' : zoom < 0.5 ? 'compact' : 'full'
