import { createContext, useContext } from 'react'

// UI providers and custom plugin components can have automatic effects.
// A Builder editor binding renders the platform's static controls only.
export const DesignEffectsContext = createContext(false)
export function useDesignOnly() { return useContext(DesignEffectsContext) }
