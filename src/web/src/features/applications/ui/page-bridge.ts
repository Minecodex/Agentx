import { z } from 'zod'

// Runtime parses UUID-shaped identities, including deterministic IDs without RFC version bits.
const task = { taskId: z.guid() }
const object = <Shape extends z.ZodRawShape>(shape: Shape) => z.object(shape).strict()
const query = object({ limit: z.number().int().min(1).max(50).optional(), cursor: z.string().max(1024).optional(), activeOnly: z.boolean().optional() })
const methods = {
  getContext: object({}),
  invoke: object({ input: z.unknown() }),
  getTask: object(task),
  listMyTasks: object({ query }),
  submitTaskInput: object({ ...task, waitId: z.guid(), content: z.unknown() }),
  cancelTask: object(task),
  openTask: object(task),
  openApproval: object({ approvalId: z.guid() }),
  downloadFile: object({ ...task, artifactId: z.guid() }),
  uploadFile: object({ name: z.string().max(255), contentType: z.string().max(255), bytes: z.instanceof(ArrayBuffer).refine(bytes => bytes.byteLength <= 50 * 1024 * 1024) }),
  subscribeTask: object({ ...task, subscriptionId: z.string().regex(/^subscription_\d{1,12}$/) }),
  unsubscribeTask: object({ subscriptionId: z.string().regex(/^subscription_\d{1,12}$/) }),
  createSession: object({ input: z.record(z.string(), z.unknown()) }),
  getSession: object({ sessionId: z.guid() }),
  sendMessage: object({ sessionId: z.guid(), text: z.string().max(128 * 1024), files: z.array(object({ artifactId: z.guid() })).max(20) }),
} as const

export type PageMethod = keyof typeof methods
export type PageDispatcher = (method: PageMethod, args: Record<string, unknown>, requestId: string, notifyTask:(subscriptionId:string,task:unknown)=>void) => Promise<unknown>

/** One port belongs to one immutable account/application/page binding. Closing it invalidates every request. */
export function connectPageBridge(port: MessagePort, dispatch: PageDispatcher) {
  const schema = object({ type: z.literal('request'), id: z.string().max(160), method: z.enum(Object.keys(methods) as [PageMethod, ...PageMethod[]]), args: z.record(z.string(), z.unknown()) })
  const responses = new Map<string, { fingerprint: string; response: Promise<unknown>; settled: boolean }>()
  let active = 0
  let admissions = 0
  let closed = false
  const send = (value: unknown) => { if (!closed) port.postMessage(value) }
  const receive = async (event: MessageEvent) => {
    if (event.data?.type !== 'request') return
    const parsed = schema.safeParse(event.data)
    if (closed) return
    if (!parsed.success) {
      if (typeof event.data?.id === 'string' && event.data.id.length <= 160) send({type:'response',id:event.data.id,error:{code:'PAGE_REQUEST_INVALID',message:'The page request does not match an allowed capability'}})
      return
    }
    const { id, method, args } = parsed.data
    if (admissions >= 16) { send({type:'response',id,error:{code:'PAGE_BUSY',message:'The application page request limit was reached'}}); return }
    admissions += 1
    try {
    let fingerprint: string
    try {
      if (!methods[method].safeParse(args).success) throw new Error('Invalid page request')
      const bytes = method === 'uploadFile' && args.bytes instanceof ArrayBuffer ? Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', args.bytes))).map(value=>value.toString(16).padStart(2,'0')).join('') : undefined
      fingerprint = JSON.stringify({ method, args, bytes })
      if (fingerprint.length > 1024 * 1024) throw new Error('Invalid page request')
    } catch { send({ type: 'response', id, error: { code: 'PAGE_REQUEST_INVALID', message: 'The page request does not match an allowed capability' } }); return }
    if (closed) return
    const previous = responses.get(id)
    if (previous && previous.fingerprint !== fingerprint) { send({ type: 'response', id, error: { code: 'PAGE_REQUEST_CONFLICT', message: 'Request identity was reused with different arguments' } }); return }
    if (!previous && responses.size >= 256) for (const [key,value] of responses) { if(value.settled){responses.delete(key);break} }
    if (!previous && (active >= 16 || responses.size >= 256)) { send({ type: 'response', id, error: { code: 'PAGE_BUSY', message: 'The application page request limit was reached' } }); return }
    const response = previous?.response ?? (async () => {
      active += 1
      try { return { type: 'response', id, result: await dispatch(method, args, id,(subscriptionId,task)=>send({type:'task',subscriptionId,task})) } }
      catch (error) {
        const detail = error as { detail?: {code?:string;message?:string;fieldErrors?:unknown[]}; code?:string;message?:string;fieldErrors?:unknown[] }
        const safe = detail.detail ?? detail
        return { type: 'response', id, error: { code: safe.code ?? 'PAGE_OPERATION_FAILED', message: safe.message ?? 'The application operation failed', fieldErrors: safe.fieldErrors ?? [] } }
      } finally { active -= 1 }
    })()
    if (!previous) {
      const record = { fingerprint, response, settled:false }
      responses.set(id, record)
      void response.then(() => { record.settled = true })
    }
    send(await response)
    } finally { admissions -= 1 }
  }
  port.addEventListener('message',receive)
  port.start()
  return {
    task(subscriptionId: string, task: unknown) { send({ type: 'task', subscriptionId, task }) },
    close() { closed = true; port.close(); responses.clear() },
  }
}
