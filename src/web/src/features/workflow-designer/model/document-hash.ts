function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical)
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical((value as Record<string, unknown>)[key])]))
  return value
}

export function canonicalDocument(value: unknown) { return JSON.stringify(canonical(value)) }

export async function documentHash(value: unknown) {
  const bytes = new TextEncoder().encode(JSON.stringify(canonical(value)))
  const hash = await crypto.subtle.digest('SHA-256', bytes)
  return `sha256:v1:${Array.from(new Uint8Array(hash), byte => byte.toString(16).padStart(2, '0')).join('')}`
}
