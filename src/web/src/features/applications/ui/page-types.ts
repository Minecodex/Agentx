import type { components } from '../../../shared/api/generated'
import type { components as gateway } from '../../../shared/api/generated-gateway'

export type PageDraft = components['schemas']['ApplicationPageDraftV1']
export type PageContract = PageDraft['contract']
export type PageSnapshot = components['schemas']['PageEditorSnapshotV1']
export type PageCommand = components['schemas']['PageEditCommandV1']
export type PageSession = components['schemas']['PageBuilderSessionV1']
export type PageAck = components['schemas']['PageBuilderAcknowledgement']
export type TaskView = gateway['schemas']['TaskView']
export type PublishedPage = gateway['schemas']['ApplicationPageView']
export type PagePublications = components['schemas']['PagePublications']
export type PageCheckIssue = {code:string;message:string;field?:string;line?:number;column?:number}
export type PageCheckReport = Omit<components['schemas']['PageCheckReport'],'errors'> & {errors:PageCheckIssue[]}
export type PageInspection = {sourceHash?:string;errors:PageCheckIssue[];fields:Array<{path:string|null;label:string;type:string;required:boolean}>;actions:string[];visibleText:string}
export type PageInspectionOptions = {schema?:unknown;waitCheck?:'prepare'|'verify';historyTaskId?:string;submitAction?:'invoke'|'send-message'}
export type PageFrameHandle = {inspect(input?:unknown,submit?:boolean,waitId?:string,options?:PageInspectionOptions):Promise<PageInspection>;restart():Promise<void>}

export async function hashPageHtml(html: string) {
  const bytes = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(JSON.stringify(html)))
  return `sha256:${Array.from(new Uint8Array(bytes)).map(value => value.toString(16).padStart(2, '0')).join('')}`
}
