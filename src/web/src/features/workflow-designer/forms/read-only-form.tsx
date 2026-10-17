import { createContext, useContext, type ReactNode } from 'react'

const ReadOnlyFormContext = createContext(false)
export function useReadOnlyForm() { return useContext(ReadOnlyFormContext) }

// Native controls inherit fieldset.disabled; rich editors consume the same state.
export function ReadOnlyForm({ readOnly = false, className, children }: { readOnly?: boolean; className?: string; children: ReactNode }) {
  const inherited = useReadOnlyForm()
  const disabled = readOnly || inherited
  return <ReadOnlyFormContext.Provider value={disabled}><fieldset className={className} disabled={disabled}>{children}</fieldset></ReadOnlyFormContext.Provider>
}
