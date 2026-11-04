import { inspectBatchPreview } from './page-batch-checks'
import { sampleValue, type createPagePreview } from './page-preview'
import type { PageInspection, PageFrameHandle, PageCheckIssue } from './page-types'

type Messages = { invoke: string; updates: string; wait: string }
const pause = (milliseconds: number) => new Promise(resolve => setTimeout(resolve, milliseconds))

function checkArrayLengths(expected:unknown, actual:unknown, errors:PageCheckIssue[], path='') {
  if(Array.isArray(expected)) {
    if(!Array.isArray(actual)||actual.length!==expected.length) {
      errors.push({code:'PAGE_INPUT_ARRAY_LENGTH_CHANGED',message:`The submitted array ${path || '(root)'} must contain exactly the rows filled in the preview. Do not submit extra blank rows or drop entered rows.`})
      return
    }
    expected.forEach((value,index)=>checkArrayLengths(value,actual[index],errors,`${path}/${index}`))
  } else if(expected&&typeof expected==='object') {
    for(const [key,value] of Object.entries(expected))checkArrayLengths(value,(actual as Record<string,unknown>|null)?.[key],errors,`${path}/${key.replaceAll('~','~0').replaceAll('/','~1')}`)
  }
}

/** The same isolated interaction check is used by the editor and browser regression tests. */
export async function inspectPagePreview(frame: PageFrameHandle, preview: ReturnType<typeof createPagePreview>, messages: Messages): Promise<PageInspection> {
  const restarted = frame.restart()
  preview.reset()
  const historicalTaskId = preview.seedHistory()
  const calls = preview.counts.invoke
  await restarted
  await pause(100)
  const inspected = await frame.inspect(preview.input, true, undefined, {schema:preview.contract.inputSchema,submitAction:preview.contract.chatMapping?'send-message':'invoke'})
  const errors = [...inspected.errors]
  let visibleText = inspected.visibleText
  for (let index = 0; index < 30 && preview.counts.invoke === calls; index += 1) await pause(100)
  if (preview.counts.invoke === calls) errors.push({ code: 'PAGE_SDK_INVOKE_MISSING', message: messages.invoke })
  if(!preview.contract.chatMapping&&preview.submittedInput()!==undefined)checkArrayLengths(preview.input,preview.submittedInput(),errors)
  for (let index = 0; index < 30 && preview.latest()?.state === 'accepted'; index += 1) await pause(100)
  for (let index = 0; index < 30 && preview.latest() && preview.observedState(preview.latest()!.taskId) !== preview.latest()!.state; index += 1) await pause(100)
  if (preview.latest() && preview.observedState(preview.latest()!.taskId) !== preview.latest()!.state) errors.push({ code: 'PAGE_TASK_UPDATES_MISSING', message: messages.updates })
  let task = preview.latest()
  for (let index = 0; task?.pendingInputs.length && index < 20; index += 1) {
    const wait = task.pendingInputs[0]!
    await pause(300)
    const submitted = preview.counts.input
    const input=sampleValue(wait.responseSchema)
    const prepared=await frame.inspect(input,false,wait.waitId,{schema:wait.responseSchema,waitCheck:'prepare'})
    errors.push(...prepared.errors)
    const revision=preview.updateWaitingTask(task.taskId)
    for(let attempt=0;attempt<30&&(preview.observedRevision(task.taskId)??0)<revision;attempt+=1)await pause(100)
    if((preview.observedRevision(task.taskId)??0)<revision)errors.push({code:'PAGE_WAIT_UPDATES_MISSING',message:'Continue observing the same waiting task after later status updates; keep the active input form and its values.'})
    const preserved=await frame.inspect(undefined,false,wait.waitId,{waitCheck:'verify'})
    errors.push(...preserved.errors)
    if(prepared.errors.length||preserved.errors.length)visibleText=preserved.visibleText
    const report = await frame.inspect(input, true, wait.waitId, {schema:wait.responseSchema})
    if (report.errors.length) visibleText = report.visibleText
    errors.push(...report.errors.map(issue => ({ ...issue, message: `Input wait ${wait.nodePath} (${wait.waitId}): ${issue.message}. Field paths are relative to this wait's responseSchema; scalar content uses data-field-path="".` })))
    for (let attempt = 0; attempt < 20 && preview.counts.input === submitted; attempt += 1) await pause(100)
    if (preview.counts.input === submitted) { errors.push({ code: 'PAGE_WAIT_INPUT_MISSING', message: messages.wait }); break }
    task = preview.latest()
  }
  if(preview.history.listed||inspected.actions.includes('list-tasks')) {
    const opened=preview.history.opened
    const report=await frame.inspect(undefined,false,undefined,{historyTaskId:historicalTaskId})
    errors.push(...report.errors)
    for(let attempt=0;attempt<30&&preview.history.opened===opened;attempt+=1)await pause(100)
    if(preview.history.opened===opened||preview.history.lastOpened!==historicalTaskId) {
      errors.push({code:'PAGE_HISTORY_OPEN_MISSING',message:'Clicking a previous-version history item must call agentx.openTask(taskId). Do not call getTask/renderTask in the current page or create another business task.'})
      visibleText=report.visibleText
    }
  }
  if(preview.counts.invoke!==calls+1)errors.push({code:'PAGE_PREVIEW_SUBMISSION_COUNT',message:'One explicit business submit must create exactly one synthetic task. Row controls, wait replies and history actions must not invoke the application again.'})
  if(preview.contract.chatMapping){
    const original=preview.latest()
    if(preview.chat.sent!==1||preview.chat.directInvocations||!original?.sessionId){
      errors.push({code:'PAGE_CHAT_METHODS_REQUIRED',message:'This application declares a chat mapping. Submit through agentx.createSession and agentx.sendMessage with data-action="send-message"; plain invoke cannot preserve the formal conversation.'})
    }else if(!errors.length){
      preview.reopen(original.taskId)
      await frame.restart()
      // Recreate the opaque document with its original task in host context.
      // In-memory page variables are gone; the real session must be recovered.
      for(let index=0;index<30&&!preview.observedState(original.taskId);index+=1)await pause(100)
      const followup=await frame.inspect(preview.input,true,undefined,{schema:preview.contract.inputSchema,submitAction:'send-message'})
      errors.push(...followup.errors)
      for(let index=0;index<30&&preview.chat.sent<2;index+=1)await pause(100)
      const next=preview.latest()
      if(Number(preview.chat.sent)!==2||preview.chat.directInvocations||preview.counts.invoke!==calls+2||!next||next.taskId===original.taskId||next.sessionId!==original.sessionId){
        errors.push({code:'PAGE_CHAT_SESSION_NOT_RESTORED',message:'After reload, recover getContext().taskId through the SDK and reuse that TaskView.sessionId for the next sendMessage. Do not create a new conversation or depend only on in-memory session variables.'})
        visibleText=followup.visibleText
      }
    }
  }
  const batch=await inspectBatchPreview(frame,preview)
  errors.push(...batch.errors)
  if(batch.errors.length)visibleText=batch.visibleText
  return { ...inspected, errors, visibleText }
}
