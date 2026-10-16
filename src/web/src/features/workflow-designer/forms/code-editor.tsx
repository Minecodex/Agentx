import { lazy, Suspense, forwardRef, useEffect, useImperativeHandle, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import type { OnMount } from '@monaco-editor/react'

import { Textarea } from '../../../shared/ui/textarea'

const Monaco = lazy(async () => {
  await import('./monaco-workers')
  const [reactMonaco, monaco] = await Promise.all([import('@monaco-editor/react'), import('monaco-editor')])
  reactMonaco.loader.config({ monaco })
  return { default: reactMonaco.default }
})

export type EditorCompletion = { label: string; insertText: string; detail?: string }
export type CodeEditorHandle = { insertText: (value: string) => void }

export const CodeEditor = forwardRef<CodeEditorHandle, { value: string; language?: string; height?: string; completionItems?: EditorCompletion[]; readOnly?: boolean; onChange: (value: string) => void; onBlur?: (value: string) => void }>(function CodeEditor({ value, language = 'plaintext', height = '150px', completionItems = [], readOnly = false, onChange, onBlur }, ref) {
  const { t } = useTranslation()
  const theme = typeof document !== 'undefined' && document.documentElement.classList.contains('dark') ? 'vs-dark' : 'light'
  const disposable = useRef<{ dispose: () => void }>()
  const blurDisposable = useRef<{ dispose: () => void }>()
  const onBlurRef = useRef(onBlur)
  onBlurRef.current = onBlur
  const editor = useRef<Parameters<OnMount>[0]>()
  useEffect(() => () => { disposable.current?.dispose(); blurDisposable.current?.dispose() }, [])
  useImperativeHandle(ref, () => ({ insertText: (text) => {
    const instance = editor.current
    const selection = instance?.getSelection()
    if (!instance || !selection) return onChange(`${value}${text}`)
    instance.pushUndoStop()
    instance.executeEdits('agentx-reference-picker', [{ range: selection, text, forceMoveMarkers: true }])
    instance.pushUndoStop()
    instance.focus()
  } }), [onChange, value])
  const fallback = <div className="border border-border bg-muted/20 p-1"><Textarea aria-label={t('studio.editorFallback')} className="resize-none border-0 bg-transparent font-mono text-xs focus-visible:ring-0" onBlur={(event) => onBlurRef.current?.(event.target.value)} onChange={(event) => onChange(event.target.value)} style={{ height }} value={value} /></div>
  if (typeof navigator !== 'undefined' && /jsdom/i.test(navigator.userAgent)) return fallback
  return <Suspense fallback={fallback}><Monaco height={height} language={language} onChange={(next) => onChange(next ?? '')} onMount={(mountedEditor, monaco) => {
    editor.current = mountedEditor
    disposable.current?.dispose()
    blurDisposable.current?.dispose()
    blurDisposable.current = mountedEditor.onDidBlurEditorText(() => onBlurRef.current?.(mountedEditor.getValue()))
    if (!completionItems.length) return
    const provider: Parameters<typeof monaco.languages.registerCompletionItemProvider>[1] = {
      provideCompletionItems(model: { getWordUntilPosition: (position: EditorPosition) => { startColumn: number; endColumn: number } }, position: EditorPosition) {
        const word = model.getWordUntilPosition(position)
        const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn }
        return { suggestions: completionItems.map((item) => ({ ...item, kind: monaco.languages.CompletionItemKind.Variable, range })) }
      },
    }
    disposable.current = monaco.languages.registerCompletionItemProvider(language, provider)
  }} options={{ automaticLayout: true, minimap: { enabled: false }, fontSize: 12, lineNumbers: language === 'plaintext' ? 'off' : 'on', readOnly, scrollBeyondLastLine: false, wordWrap: 'on' }} theme={theme} value={value} /></Suspense>
})

type EditorPosition = { lineNumber: number; column: number }
