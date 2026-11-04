import { createRoot } from 'react-dom/client'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ReactFlowProvider, useReactFlow } from '@xyflow/react'
import * as Tooltip from '@radix-ui/react-tooltip'
import { i18n } from '../app/i18n'
import { ThemeProvider } from '../app/providers/theme-provider'
import '../styles/globals.css'
import { PageFrame } from '../features/applications/ui/page-frame'
import { createPagePreview } from '../features/applications/ui/page-preview'
import { inspectPagePreview } from '../features/applications/ui/page-checks'
import { PAGE_SDK_SOURCE } from '../features/applications/ui/page-sdk'
import { PAGE_STYLE_SOURCE } from '../features/applications/ui/page-styles'
import type { PageDispatcher } from '../features/applications/ui/page-bridge'
import type { PageContract, PageFrameHandle } from '../features/applications/ui/page-types'
import { WorkflowFlow } from '../features/workflow-designer/canvas/workflow-flow'
import { createEditorStore, EditorStoreProvider } from '../features/workflow-designer/store/editor-store'
import { deserializeDraft } from '../features/workflow-designer/model/serializer'
import type { NodeManifest } from '../features/workflow-designer/model/types'
import type { components } from '../shared/api/generated'

type Request = { document: {kind:'workflow';document:components['schemas']['StudioEditorSnapshotV1']['document'];manifests:Record<string,NodeManifest>} | {kind:'application_page';html:string;contract:PageContract;contractHash:string};options:{locale:string;theme:string} }
type PreviewApi = {ready:boolean;reload:()=>Promise<void>;inspect:()=>Promise<unknown>;check:()=>Promise<unknown>;facts:()=>unknown;focusNodes?:(ids:string[])=>Promise<boolean>}
declare global { interface Window {agentxDesignInput:Request;agentxDesignPreview:PreviewApi} }
const input=window.agentxDesignInput
localStorage.setItem('agentx.theme',input.options.theme)
localStorage.setItem('agentx.locale',input.options.locale)
void i18n.changeLanguage(input.options.locale)

function PagePreview({document}:{document:Extract<Request['document'],{kind:'application_page'}>}) {
  const frame=useRef<PageFrameHandle>(null)
  const preview=useMemo(()=>createPagePreview(document.contract,document.contractHash,input.options.locale,input.options.theme),[document])
  const [generation,setGeneration]=useState(0)
  const target=useRef<{taskId?:string;batchId?:string}>({})
  const dispatch=useCallback<PageDispatcher>(async(method,args,id,notify)=>{
    const result=await preview.dispatch(method,args,id,notify)
    if((method==='invoke'||method==='sendMessage')&&result&&typeof result==='object'&&'taskId' in result)target.current={taskId:String(result.taskId)}
    if((method==='createBatch'||method==='rerunFailedItems')&&result&&typeof result==='object'&&'batchId' in result)target.current={batchId:String(result.batchId)}
    return result
  },[preview])
  useEffect(()=>{
    preview.seedHistory()
    window.agentxDesignPreview={ready:true,reload:async()=>{if(target.current.batchId)preview.reopenBatch(target.current.batchId);else if(target.current.taskId)preview.reopen(target.current.taskId);await frame.current!.restart();setGeneration(n=>n+1)},inspect:()=>frame.current!.inspect(),check:async()=>inspectPagePreview(frame.current!,preview,{invoke:'Invoke was not observed',updates:'Task updates were not observed',wait:'Waiting input was not submitted'}),facts:()=>({source:'synthetic_sdk',counts:preview.counts,batchCounts:preview.batches.counts,latestTask:preview.latest(),latestBatch:preview.batches.latest()})}
    return()=>preview.close()
  },[preview])
  return <div data-preview-generation={generation} style={{height:'100vh'}}><PageFrame ref={frame} html={document.html} sdkScript={PAGE_SDK_SOURCE} stylesheet={PAGE_STYLE_SOURCE} identity="isolated-design-preview" dispatch={dispatch}/></div>
}

function WorkflowPreview({document}:{document:Extract<Request['document'],{kind:'workflow'}>}) {
  return <ReactFlowProvider><WorkflowCanvas document={document}/></ReactFlowProvider>
}
function WorkflowCanvas({document}:{document:Extract<Request['document'],{kind:'workflow'}>}) {
  const flow=useReactFlow()
  const store=useMemo(()=>{const value=createEditorStore(true);value.getState().hydrate(deserializeDraft({id:'preview',workflowId:'preview',versionId:'',versionNumber:0,publicationStatus:'unpublished',archived:false,generation:0,baseVersionId:null,revision:0,schemaVersion:'8.0',definitionHash:'',editorHash:'',updatedAt:'',...document.document}));return value},[document])
  const manifests=useMemo(()=>new Map(Object.entries(document.manifests)),[document])
  useEffect(()=>{window.agentxDesignPreview={ready:true,reload:async()=>undefined,inspect:async()=>undefined,focusNodes:ids=>flow.fitView({nodes:ids.length?ids.map(id=>({id})):undefined,minZoom:0.1,maxZoom:1,padding:0.15,duration:0}),check:async()=>({source:'actual_canvas',validation:'not_performed',hint:'Use workflow_validate for semantic validation; browser geometry is visual evidence only'}),facts:()=>({source:'actual_canvas',nodeLayouts:document.document.editorDocument.nodeLayouts,boundaryLayouts:document.document.editorDocument.boundaryLayouts,connections:document.document.definition.connections})}},[document,flow])
  return <EditorStoreProvider store={store} readOnly><div style={{height:'100vh',display:'flex',flexDirection:'column'}}><WorkflowFlow manifests={manifests} runtimeStatuses={new Map()} readOnly onDropAction={()=>undefined}/></div></EditorStoreProvider>
}

window.agentxDesignPreview={ready:false,reload:async()=>undefined,inspect:async()=>undefined,check:async()=>undefined,facts:()=>undefined}
createRoot(document.getElementById('root')!).render(<ThemeProvider><Tooltip.Provider>{input.document.kind==='workflow'?<WorkflowPreview document={input.document}/>:<PagePreview document={input.document}/>}</Tooltip.Provider></ThemeProvider>)
