import {afterEach,expect,it,vi} from 'vitest'
import {createPagePreview,sampleValue} from './page-preview'
import type {PageContract} from './page-types'

const contract:PageContract={applicationId:'application',applicationName:'Quote',description:null,deploymentId:'deployment',workflowVersionId:'workflow',bundleId:'bundle',inputSchema:{type:'object',properties:{customer:{type:'string'}}},outputSchema:{type:'object',properties:{answer:{type:'string'}}},waitInputNodes:[],approvalStages:[],chatMapping:null,sdkVersion:1,maxUploadBytes:1024}
afterEach(()=>vi.useRealTimers())

it('rejects malformed initial inputs before creating a task or charging a preview invocation',async()=>{
  const preview=createPagePreview({...contract,inputSchema:{type:'object',properties:{amount:{type:'number',minimum:0}},required:['amount'],additionalProperties:false}},'hash','zh-CN','light')
  try{
    await expect(preview.dispatch('invoke',{input:{amount:'12.50'}},'bad',vi.fn())).rejects.toMatchObject({code:'PREVIEW_INPUT_SCHEMA_INVALID'})
    expect(preview.latest()).toBeUndefined();expect(preview.counts.invoke).toBe(0)
    await preview.dispatch('invoke',{input:{amount:12.5}},'good',vi.fn())
    expect(preview.counts.invoke).toBe(1)
  }finally{preview.close()}
})

it('rejects pointer-key wait payloads without consuming the wait, then accepts the declared object',async()=>{
  vi.useFakeTimers()
  const responseSchema={type:'object',properties:{contact:{type:'string',minLength:2},date:{type:'string',format:'date'},email:{type:'string',format:'email'}},required:['contact','date','email'],additionalProperties:false}
  const preview=createPagePreview({...contract,waitInputNodes:[{nodeId:'delivery',name:'Delivery',path:'/delivery',responseSchema}]},'hash','zh-CN','light')
  try{
    const task=await preview.dispatch('invoke',{input:{}},'invoke',vi.fn()) as {taskId:string}
    await vi.advanceTimersByTimeAsync(300)
    const wait=preview.latest()!.pendingInputs[0]!,revision=preview.latest()!.revision
    await expect(preview.dispatch('submitTaskInput',{taskId:task.taskId,waitId:wait.waitId,content:{'/contact':'Preview','/date':'2026-11-02','/email':'preview@example.test'}},'bad',vi.fn())).rejects.toMatchObject({code:'PREVIEW_INPUT_SCHEMA_INVALID'})
    expect(preview.latest()).toMatchObject({state:'waiting_input',revision,pendingInputs:[{waitId:wait.waitId}]})
    expect(preview.counts.input).toBe(0)
    await preview.dispatch('submitTaskInput',{taskId:task.taskId,waitId:wait.waitId,content:{contact:'Preview',date:'2026-11-02',email:'preview@example.test'}},'good',vi.fn())
    expect(preview.latest()?.state).toBe('completed');expect(preview.counts).toMatchObject({input:1,formalRequests:0})
    expect(responseSchema).not.toHaveProperty('__absolute_uri__')
  }finally{preview.close()}
})

it('exercises fractional numbers without violating a declared numeric quantum or integer type',()=>{
  const scalar=sampleValue({type:'number',minimum:0,maximum:50}) as number
  expect(scalar).toBeGreaterThan(0);expect(scalar).toBeLessThan(50)
  expect(Number.isInteger(scalar)).toBe(false)
  const quantum=sampleValue({type:'number',minimum:0,maximum:2,multipleOf:0.25}) as number
  expect(Number.isInteger(quantum)).toBe(false);expect(quantum/0.25).toBeCloseTo(Math.round(quantum/0.25))
  expect(Number.isInteger(sampleValue({type:'integer',minimum:1,maximum:10}))).toBe(true)
  expect(sampleValue({type:'number',minimum:3,maximum:3})).toBe(3)
})

it('exercises a second array row only when the frozen cardinality allows it',()=>{
  expect(sampleValue({type:'array',items:{type:'string'},maxItems:1})).toHaveLength(1)
  expect(sampleValue({type:'array',items:{type:'string'},minItems:3})).toHaveLength(3)
  expect(sampleValue({type:'array',items:{type:'string'},maxItems:0})).toEqual([])
  expect(sampleValue({type:'array',items:{type:'string'},minItems:1,maxItems:20})).toHaveLength(2)
})

it('does not disguise a page missing task updates by completing during invoke',async()=>{
  vi.useFakeTimers()
  const preview=createPagePreview(contract,'hash','zh-CN','light'),notify=vi.fn()
  try{
    const initial=await preview.dispatch('invoke',{input:{customer:'Synthetic'}},'request',notify) as {taskId:string;state:string;outputs:unknown}
    expect(initial).toMatchObject({state:'accepted',outputs:null})
    await vi.advanceTimersByTimeAsync(500)
    expect(preview.latest()).toMatchObject({taskId:initial.taskId,state:'completed',outputs:{answer:'Preview'}})
    expect(preview.observedState(initial.taskId)).toBeUndefined()
    expect(initial.outputs).toBeNull()
    await preview.dispatch('getTask',{taskId:initial.taskId},'query',notify)
    expect(preview.observedState(initial.taskId)).toBe('completed')
    expect(preview.counts.formalRequests).toBe(0)
  }finally{preview.close()}
})

it('keeps parallel waits and their subscription on the same synthetic task',async()=>{
  vi.useFakeTimers()
  const waits=[{nodeId:'invoice',name:'Invoice',path:'invoice',responseSchema:{type:'string'}},{nodeId:'delivery',name:'Delivery',path:'delivery',responseSchema:{type:'object',properties:{contact:{type:'string'}}}}]
  const preview=createPagePreview({...contract,waitInputNodes:waits},'hash','zh-CN','light'),notify=vi.fn()
  try{
    const initial=await preview.dispatch('invoke',{input:{}},'request',notify) as {taskId:string}
    await preview.dispatch('subscribeTask',{taskId:initial.taskId,subscriptionId:'watch'},'watch',notify)
    await vi.advanceTimersByTimeAsync(300)
    const task=preview.latest()!
    expect(task.state).toBe('waiting_input');expect(task.pendingInputs).toHaveLength(2)
    expect(preview.observedState(task.taskId)).toBe('waiting_input')
    const waitingNotifications=notify.mock.calls.length
    await vi.advanceTimersByTimeAsync(1000)
    expect(notify).toHaveBeenCalledTimes(waitingNotifications)
    for(const wait of [...task.pendingInputs])await preview.dispatch('submitTaskInput',{taskId:task.taskId,waitId:wait.waitId,content:sampleValue(wait.responseSchema)},wait.waitId,notify)
    await vi.advanceTimersByTimeAsync(100)
    expect(preview.latest()).toMatchObject({taskId:initial.taskId,state:'completed',pendingInputs:[]})
    expect(preview.observedState(task.taskId)).toBe('completed')
    expect(preview.counts).toMatchObject({invoke:1,input:2,formalRequests:0})
  }finally{preview.close()}
})

it('does not resurrect a cancelled synthetic task or leave preview timers after closing',async()=>{
  vi.useFakeTimers()
  const preview=createPagePreview(contract,'hash','zh-CN','light'),notify=vi.fn()
  const initial=await preview.dispatch('invoke',{input:{}},'request',notify) as {taskId:string}
  await preview.dispatch('subscribeTask',{taskId:initial.taskId,subscriptionId:'watch'},'watch',notify)
  await preview.dispatch('cancelTask',{taskId:initial.taskId},'cancel',notify)
  await vi.advanceTimersByTimeAsync(500)
  expect(preview.latest()?.state).toBe('cancelled')
  expect(notify.mock.calls.at(-1)?.[1]).toMatchObject({state:'cancelled'})
  preview.close();const calls=notify.mock.calls.length
  await vi.advanceTimersByTimeAsync(1000)
  expect(notify).toHaveBeenCalledTimes(calls)
  expect(vi.getTimerCount()).toBe(0)
})

it('starts a new isolated check without prior tasks, notifications or pending transitions',async()=>{
  vi.useFakeTimers()
  const preview=createPagePreview(contract,'hash','zh-CN','light'),notify=vi.fn()
  try{
    const previous=await preview.dispatch('invoke',{input:{}},'old',notify) as {taskId:string}
    await preview.dispatch('subscribeTask',{taskId:previous.taskId,subscriptionId:'old'},'watch',notify)
    preview.reset()
    const notifications=notify.mock.calls.length
    await vi.advanceTimersByTimeAsync(1000)
    expect(notify).toHaveBeenCalledTimes(notifications)
    expect(vi.getTimerCount()).toBe(0)
    expect(preview.latest()).toBeUndefined()
    expect(preview.observedState(previous.taskId)).toBeUndefined()
    expect(await preview.dispatch('listMyTasks',{},'history',notify)).toEqual({items:[],next:null})
    await expect(preview.dispatch('getTask',{taskId:previous.taskId},'stale',notify)).rejects.toMatchObject({code:'TASK_NOT_FOUND'})
    const current=await preview.dispatch('invoke',{input:{}},'new',notify) as {taskId:string}
    expect(current.taskId).not.toBe(previous.taskId)
    await vi.advanceTimersByTimeAsync(300)
    expect(preview.latest()).toMatchObject({taskId:current.taskId,state:'completed'})
    expect(preview.counts).toEqual({invoke:1,input:0,upload:0,formalRequests:0})
  }finally{preview.close()}
})

it('keeps previous-version preview history separate from current task reads and business submissions',async()=>{
  const preview=createPagePreview(contract,'hash','zh-CN','light'),notify=vi.fn()
  try{
    const id=preview.seedHistory()
    expect(preview.counts.invoke).toBe(0);expect(preview.latest()).toBeUndefined()
    await expect(preview.dispatch('getTask',{taskId:id},'read',notify)).rejects.toMatchObject({code:'PAGE_TASK_VERSION_MISMATCH'})
    await expect(preview.dispatch('subscribeTask',{taskId:id,subscriptionId:'wrong'},'subscribe',notify)).rejects.toMatchObject({code:'PAGE_TASK_VERSION_MISMATCH'})
    await preview.dispatch('openTask',{taskId:id},'open',notify)
    expect(preview.history).toMatchObject({opened:1,lastOpened:id})
    preview.reset()
    expect(preview.history).toEqual({listed:0,opened:0,lastOpened:null,taskId:null})
    expect(await preview.dispatch('listMyTasks',{},'empty',notify)).toEqual({items:[],next:null})
    expect(preview.counts.formalRequests).toBe(0)
  }finally{preview.close()}
})

it('uses a genuine new synthetic revision without closing or replacing active wait identities',async()=>{
  vi.useFakeTimers()
  const preview=createPagePreview({...contract,waitInputNodes:[{nodeId:'reply',name:'Reply',path:'/reply',responseSchema:{type:'string'}}]},'hash','zh-CN','light'),notify=vi.fn()
  try{
    const task=await preview.dispatch('invoke',{input:{customer:'Synthetic'}},'invoke',notify) as {taskId:string}
    await preview.dispatch('subscribeTask',{taskId:task.taskId,subscriptionId:'watch'},'watch',notify)
    await vi.advanceTimersByTimeAsync(300)
    const before=structuredClone(preview.latest()!),revision=preview.updateWaitingTask(task.taskId)
    await vi.advanceTimersByTimeAsync(100)
    expect(preview.latest()!.pendingInputs).toEqual(before.pendingInputs)
    expect(revision).toBe(before.revision+1);expect(preview.observedRevision(task.taskId)).toBe(revision)
    expect(preview.latest()!.messages).toHaveLength(1)
    expect(preview.counts).toMatchObject({invoke:1,input:0,formalRequests:0})
  }finally{preview.close()}
})

it('accepts raw JSON when an input wait has no response schema',async()=>{
  vi.useFakeTimers()
  const preview=createPagePreview({...contract,waitInputNodes:[{nodeId:'reply',name:'Reply',path:'/reply',responseSchema:null}]},'hash','zh-CN','light')
  try{
    const task=await preview.dispatch('invoke',{input:{customer:'Synthetic'}},'invoke',vi.fn()) as {taskId:string}
    await vi.advanceTimersByTimeAsync(300)
    const wait=preview.latest()!.pendingInputs[0]!
    await preview.dispatch('submitTaskInput',{taskId:task.taskId,waitId:wait.waitId,content:{comment:'Synthetic JSON',rows:[1,true,null]}},'reply',vi.fn())
    expect(preview.latest()?.state).toBe('completed');expect(preview.counts).toMatchObject({input:1,formalRequests:0})
  }finally{preview.close()}
})
