import { expect, it } from 'vitest'
import type { components } from '../../../shared/api/generated'
import { restoreBuilderReceipt, type ReceiptProof } from './builder-receipt'

it('replays the exact applied ACK after durable storage without local-only fields', () => {
  type Ack = components['schemas']['StudioEditorAcknowledgementV1']
  const commandDocument = {
    definition: { schemaVersion: '8.0', nodes: [], connections: [] },
    editorDocument: { nodeLayouts: [], boundaryLayouts: [], edges: [], annotations: [], groups: [], viewport: { x: 0, y: 0, zoom: 1 } },
  } as unknown as Ack['snapshot']['document']
  const original: Ack = {
    commandId: 'command', status: 'applied', issues: [], snapshot: {
      editorInstanceId: 'editor', localDocumentRevision: 7, documentHash: 'sha256:applied',
      serverRevision: 2, saveStatus: 'unsaved', selectedNodeIds: [], selectedExecutionId: null,
      document: { ...commandDocument, editorDocument: { ...commandDocument.editorDocument, boundaryLayouts: [{ boundary: 'start', x: 40, y: 220 }] } },
    },
  }
  const { document, ...snapshot } = original.snapshot
  const proof: ReceiptProof = { commandId: original.commandId, status: original.status, issues: original.issues, snapshot, editorDocument: document.editorDocument }
  const restored = restoreBuilderReceipt(JSON.parse(JSON.stringify(proof)), commandDocument)
  expect(JSON.parse(JSON.stringify(restored))).toEqual(original)
  expect(Object.keys(restored).sort()).toEqual(['commandId', 'issues', 'snapshot', 'status'])
  expect(commandDocument.editorDocument.boundaryLayouts).toEqual([])
})
