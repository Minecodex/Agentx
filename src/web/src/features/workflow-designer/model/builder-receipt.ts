import type { components } from '../../../shared/api/generated'

type Ack = components['schemas']['StudioEditorAcknowledgementV1']
type Snapshot = Ack['snapshot']
export type ReceiptProof = Omit<Ack, 'snapshot'> & {
  snapshot: Omit<Snapshot, 'document'>
  editorDocument: Snapshot['document']['editorDocument']
}

/** Local proof geometry belongs inside the wire snapshot, never on the ACK root. */
export function restoreBuilderReceipt(proof: ReceiptProof, document: Snapshot['document']): Ack {
  const { editorDocument, snapshot, ...ack } = proof
  return { ...ack, snapshot: { ...snapshot, document: { ...document, editorDocument } } }
}
