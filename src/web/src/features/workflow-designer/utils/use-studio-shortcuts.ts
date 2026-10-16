import { useEffect, useRef } from 'react'

import type { StudioEdge, StudioNode } from '../model/types'
import { useEditorStoreApi } from '../store/editor-store'
import { cloneStudioFragment, readStudioClipboard, writeStudioClipboard } from './studio-clipboard'

type ShortcutOptions = {
  workflowId?: string
  revalidatePaste?: (nodes: StudioNode[], edges: StudioEdge[]) => Promise<string[]>
  onPasteRejected?: (messages: string[]) => void
  onOpenNode?: (nodeId: string) => void
  onEscape?: () => void
}

export function useStudioShortcuts(options: ShortcutOptions = {}) {
  const storeApi = useEditorStoreApi()
  const optionsRef = useRef(options)
  optionsRef.current = options

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (isEditable(event.target)) return
      const state = storeApi.getState()
      const command = event.ctrlKey || event.metaKey
      if (event.key === 'Escape') {
        optionsRef.current.onEscape?.()
        return
      }
      if (event.key === 'Enter' && state.selectedId && state.nodes.some((node) => node.id === state.selectedId)) {
        event.preventDefault()
        optionsRef.current.onOpenNode?.(state.selectedId)
        return
      }
      if (command && event.key.toLowerCase() === 'z') {
        event.preventDefault()
        if (event.shiftKey) state.redo()
        else state.undo()
        return
      }
      if (command && event.key.toLowerCase() === 'y') {
        event.preventDefault()
        state.redo()
        return
      }
      if (command && event.key.toLowerCase() === 'c') {
        const selected = state.nodes.filter((node) => node.selected || node.id === state.selectedId)
        const ids = new Set(selected.map((node) => node.id))
        if (selected.length) writeStudioClipboard(optionsRef.current.workflowId ?? '', selected, state.edges.filter((edge) => ids.has(edge.source) && ids.has(edge.target)))
        return
      }
      const clipboard = readStudioClipboard()
      if (command && event.key.toLowerCase() === 'v' && clipboard) {
        event.preventDefault()
        const [nodes, edges] = cloneStudioFragment(clipboard)
        const current = optionsRef.current
        if (clipboard.sourceWorkflowId && current.workflowId && clipboard.sourceWorkflowId !== current.workflowId && current.revalidatePaste) {
          void current.revalidatePaste(nodes, edges).then((messages) => {
            if (messages.length) current.onPasteRejected?.(messages)
            else storeApi.getState().paste(nodes, edges)
          }).catch((error: Error) => current.onPasteRejected?.([error.message]))
        } else state.paste(nodes, edges)
        return
      }
      if (event.key === 'Delete' || event.key === 'Backspace') {
        if (state.edges.some((edge) => edge.selected)) {
          event.preventDefault()
          state.removeSelectedEdges()
          return
        }
        state.removeSelected()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [storeApi])
}

function isEditable(target: EventTarget | null) {
  return target instanceof HTMLElement && Boolean(target.closest('input, textarea, select, [contenteditable="true"], .monaco-editor'))
}
