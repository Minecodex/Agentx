import { serializeStudio } from './serializer'
import type { StudioDocument } from './types'

// Only persisted data matters. Selection and resolved UI metadata may change
// while a save is pending without invalidating the saved document.
export function samePersistedDraft(saved: StudioDocument, current: StudioDocument) {
  return JSON.stringify(serializeStudio(saved)) === JSON.stringify(serializeStudio(current))
}
