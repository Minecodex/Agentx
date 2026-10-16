import EditorWorker from 'monaco-editor/editor/editor.worker.js?worker'
import JsonWorker from 'monaco-editor/language/json/json.worker.js?worker'
import CssWorker from 'monaco-editor/language/css/css.worker.js?worker'
import HtmlWorker from 'monaco-editor/language/html/html.worker.js?worker'
import TypeScriptWorker from 'monaco-editor/language/typescript/ts.worker.js?worker'

// Vite emits same-origin worker assets; Monaco's default relative bootstrap URL
// cannot resolve from a production blob URL. Load this before creating editors.
self.MonacoEnvironment = {
  getWorker(_workerId, language) {
    switch (language) {
      case 'json': return new JsonWorker()
      case 'css': case 'scss': case 'less': return new CssWorker()
      case 'html': case 'handlebars': case 'razor': return new HtmlWorker()
      case 'javascript': case 'typescript': return new TypeScriptWorker()
      default: return new EditorWorker()
    }
  },
}
