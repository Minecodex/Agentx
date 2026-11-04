export const effortLabels: Record<string, string> = {
  none: 'models.reasoningLevels.none', minimal: 'models.reasoningLevels.minimal',
  low: 'models.reasoningLevels.low', medium: 'models.reasoningLevels.medium',
  high: 'models.reasoningLevels.high', xhigh: 'models.reasoningLevels.xhigh', max: 'models.reasoningLevels.max',
}

export function reasoningLabel(parameters: unknown, t: (key: string) => string) {
  const effort = parameters && typeof parameters === 'object' && 'reasoning_effort' in parameters ? parameters.reasoning_effort : undefined
  if (effort === undefined) return t('models.reasoningDefault')
  if (typeof effort !== 'string' || !effort) return t('models.reasoningCustom')
  return Object.hasOwn(effortLabels, effort) ? t(effortLabels[effort]) : effort
}
