import ELK from 'elkjs/lib/elk-api.js'
import elkWorkerUrl from 'elkjs/lib/elk-worker.min.js?url'
import type { NodeManifest, StudioDocument } from '../model/types'
import { layoutStudioDocumentCore } from './layout-core'

self.onmessage = async (event: MessageEvent<{ document: StudioDocument; manifests: Array<[string, NodeManifest]> }>) => {
  const elk = new ELK({ workerFactory: () => new Worker(elkWorkerUrl) })
  try { self.postMessage({ document: await layoutStudioDocumentCore(event.data.document, new Map(event.data.manifests), elk) }) }
  catch (error) { self.postMessage({ error: error instanceof Error ? error.message : String(error) }) }
  finally { elk.terminateWorker() }
}
