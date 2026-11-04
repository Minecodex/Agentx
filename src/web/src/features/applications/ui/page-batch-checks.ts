import type { createPagePreview } from './page-preview'
import { sampleValue } from './page-preview'
import type { PageCheckIssue, PageFrameHandle } from './page-types'

const pause=(ms:number)=>new Promise(resolve=>setTimeout(resolve,ms))

/** Checks a real generated DOM against an isolated, asynchronously progressing batch. */
export async function inspectBatchPreview(frame:PageFrameHandle,preview:ReturnType<typeof createPagePreview>){
  const markers=await frame.inspect(undefined,false)
  const errors:PageCheckIssue[]=[]
  if(!markers.actions.includes('create-batch'))return{errors,visibleText:markers.visibleText}
  preview.reset()
  const historyId=preview.batches.seedHistory()
  preview.seedHistory()
  await frame.restart();await pause(100)
  const inputs=Array.from({length:3},()=>sampleValue(preview.contract.inputSchema))
  const report=await frame.inspect(undefined,true,undefined,{schema:preview.contract.inputSchema,batchInputs:inputs,submitAction:'create-batch'})
  errors.push(...report.errors)
  for(let i=0;i<30&&preview.batches.counts.create===0;i++)await pause(100)
  if(preview.batches.counts.create!==1||preview.batches.counts.items!==3||preview.counts.invoke!==0)errors.push({code:'PAGE_BATCH_SUBMISSION_COUNT',message:'One batch submit must create exactly one batch and exactly three independent items, with no direct invoke calls'})
  const submitted=preview.batches.submitted() as Array<{clientItemId:string;input:unknown}>
  if(new Set(submitted.map(i=>i.clientItemId)).size!==3||submitted.some(i=>!i.clientItemId))errors.push({code:'PAGE_BATCH_CLIENT_ID',message:'Preserve a distinct stable clientItemId for every submitted row'})
  // File inputs deliberately contain newly uploaded synthetic Artifact IDs.
  const compare=(expected:unknown,actual:unknown):boolean=>{
    if(expected&&typeof expected==='object'){
      if('artifactId' in expected)return Boolean(actual&&typeof actual==='object'&&'artifactId' in actual)
      if(Array.isArray(expected))return Array.isArray(actual)&&expected.length===actual.length&&expected.every((v,i)=>compare(v,actual[i]))
      return Boolean(actual&&typeof actual==='object'&&Object.entries(expected).every(([key,value])=>compare(value,(actual as Record<string,unknown>)[key])))
    }return expected===actual
  }
  if(submitted.length!==3||submitted.some((item,i)=>!compare(inputs[i],item.input)))errors.push({code:'PAGE_BATCH_INPUT_CHANGED',message:'Submit the exact typed values of all three rows; do not drop, duplicate, coerce or replace row data'})
  await pause(300)
  for(let i=0;i<60&&preview.batches.latest()&&preview.batches.counts.observedRevision!==preview.batches.latest()!.revision;i++)await pause(100)
  if(preview.batches.latest()&&preview.batches.counts.observedRevision!==preview.batches.latest()!.revision)errors.push({code:'PAGE_BATCH_UPDATES_MISSING',message:'Poll getBatch/listBatchItems after acceptance and render later statuses instead of stopping at the initial accepted response'})
  if(preview.maxTaskSubscriptions()>1)errors.push({code:'PAGE_BATCH_SUBSCRIPTION_FANOUT',message:'Use batch pagination/polling; subscribe only to the selected task, never once per row'})
  if(markers.actions.includes('list-batches')||preview.batches.counts.historyListed){
    const history=await frame.inspect(undefined,false,undefined,{historyBatchId:historyId})
    errors.push(...history.errors)
    if(preview.batches.counts.lastHistoryOpened!==historyId)errors.push({code:'PAGE_BATCH_HISTORY_OPEN_MISSING',message:'Open a previous-version batch through openBatch(batchId), not getBatch/render in the current page'})
  }
  if(preview.batches.counts.create!==1||preview.counts.invoke!==0)errors.push({code:'PAGE_BATCH_IMPLICIT_SUBMISSION',message:'Polling, history, row editing and import must not create additional business requests'})
  return{errors,visibleText:report.visibleText}
}
