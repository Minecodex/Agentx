import type { components } from '../../../shared/api/generated'
export type Dataset = components['schemas']['DatasetResponse']
export type TestCase = components['schemas']['TestCaseResponse']
export type ContractVersion = components['schemas']['ApplicationTestContract']
export type Profile = components['schemas']['EvaluationProfileResponse']
export type Batch = components['schemas']['EvaluationBatchReport']
export type Run = Batch['runs'][number]
export type CaseResult = Run['cases'][number]
export type CaseDraft = { name: string; input: Record<string, unknown>; expectedOutput: Record<string, unknown>; tags: string[]; pendingFields?: string[]; sourceInvocationId?: string | null; referenceConfirmed: boolean }
export type InvocationSample = components['schemas']['InvocationTestSample']
export type CaseFieldError = TestCase['validationErrors'][number]
export const testingPath = (applicationId: string) => `/applications/${applicationId}`
export const summary = (value: unknown) => JSON.stringify(value) ?? '—'
