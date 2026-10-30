import type { NodeManifest, StudioDocument } from '../model/types'
export * from './layout-constants'

/** Layout is explicit, off-thread and transactional. A completed job owns no worker. */
export function layoutStudioDocument(document: StudioDocument, manifests?: Map<string, NodeManifest>): Promise<StudioDocument> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL('./layout.worker.ts', import.meta.url), { type: 'module' })
    worker.onmessage = (event: MessageEvent<{ document?: StudioDocument; error?: string }>) => {
      worker.terminate()
      if (event.data.document) resolve(event.data.document)
      else reject(new Error(event.data.error ?? 'Layout did not return a document'))
    }
    worker.onerror = event => { worker.terminate(); reject(new Error(event.message)) }
    worker.postMessage({ document, manifests: [...(manifests ?? [])] })
  })
}
