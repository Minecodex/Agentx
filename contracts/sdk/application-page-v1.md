# Agentx page SDK 1

The trusted host installs `window.agentx` before page scripts run. All methods are asynchronous. The page receives no credentials, API client or configurable application target. Use native semantic HTML, associated `<label>` elements and the platform's CSS variables (`--background`, `--foreground`, `--surface`, `--muted-foreground`, `--border`, `--primary`, `--success`, `--warning`, `--danger`). Native input, select, textarea, button and table styles are provided. Long result strings wrap within their containers; keep that behavior for file hashes, identifiers and summaries rather than forcing them onto one line.

```ts
interface AgentxPage {
  getContext(): Promise<{ mode: 'preview' | 'live' | 'history'; contract: ApplicationPageContract; theme: 'light' | 'dark'; locale: string; user: { displayName: string }; taskId: string | null }>;
  invoke(input: unknown): Promise<TaskView>;
  getTask(taskId: string): Promise<TaskView>;
  subscribeTask(taskId: string, onTask: (task: TaskView) => void): Promise<() => void>;
  listMyTasks(query?: { limit?: number; cursor?: string; activeOnly?: boolean }): Promise<{ items: TaskView[]; next: string | null }>;
  submitTaskInput(taskId: string, waitId: string, content: unknown): Promise<TaskView>;
  cancelTask(taskId: string): Promise<TaskView>;
  uploadFile(file: File): Promise<{ artifactId: string; contentType: string; sizeBytes: number; sha256: string }>;
  downloadFile(taskId: string, artifactId: string): Promise<void>;
  openTask(taskId: string): Promise<void>;
  openApproval(approvalId: string): Promise<void>;
  createSession(input?: Record<string, unknown>): Promise<{ id: string }>;
  getSession(sessionId: string): Promise<{ id: string; messages: unknown[] }>;
  sendMessage(sessionId: string, text: string, files?: Array<{ artifactId: string }>): Promise<TaskView>;
}
```

`TaskView.taskId` is the Invocation ID. `state` is `accepted`, `running`, `waiting_input`, `waiting_approval`, `completed`, `failed`, `cancelled` or `outcome_unknown`. Display the returned state and actual `stages`, not a timer-derived percentage. `outputs` is the exact workflow output object. `messages` contains `{id,content,createdAt}`. `pendingInputs` is the complete array of active `{waitId,nodePath,occurrence,responseSchema,expiresAt}`; render each wait independently. A missing `responseSchema` accepts raw JSON including strings. `pendingApprovals` contains read-only `{approvalId,title,status,version,expiresAt}`. The generated page cannot approve or reject. `files` contains authorized Artifact metadata; use downloadFile, not a constructed URL.

`allowedActions` is an object with `{canCancel: boolean, inputWaitIds: string[], downloadableArtifactIds: string[]}`. Read `task.allowedActions.canCancel` directly; never call `indexOf`, `includes` or another array method on `allowedActions`. The `application_contract` tool returns the authoritative `taskViewSchema` for all task fields. Render safely from that schema and retain visible error feedback when a request or rendering fails. Preview diagnostics include bounded visible page text as untrusted inspection data; that text may contain the concrete error caught and displayed by the page.

The workflow input's Artifact-marked fields take returned Artifact references in the exact shape declared by the schema. Represent each such field with a file input whose data-field-path is the complete Artifact field pointer (for example /document); use a multiple file input at the array pointer for an Artifact array (for example /documents). Do not ask the employee to type artifactId, hash or other file metadata. Upload each user-selected File before invocation. File bytes and download navigation are handled by the host.

On load, when `getContext().taskId` is present, call `getTask` and subscribe to that original task. Use `openTask` for history entries so the trusted host loads the original page version. A `PAGE_TASK_VERSION_MISMATCH` requires opening the original page, not changing the task or coercing its data.

An invocation initially returns an accepted or running task; its outputs and waits can arrive later. After every invoke or sendMessage, subscribe to that task and render every update, or poll getTask until terminal. Keep the same task ID after input submission and approvals. The preview also completes asynchronously and rejects a page that never observes task updates.

Only use conversation methods when `contract.chatMapping` is present. Session identity belongs to the current authenticated user. Task subscriptions can return an updated wait or another approval; keep the same task ID. Network loss does not cancel a task. Use `openTask` to restore a history entry in its matching page version; a history page cannot submit a new invocation. In preview all SDK operations are synthetic and clearly labelled. Preview must never claim a real execution succeeded.

Errors reject with `{code,message,fieldErrors?:Array<{field,code,message}>}`. Field names are JSON Pointers. Never silently turn an error into success. One explicit click creates one logical request; the host keeps its request ID stable across network retries. Mark inputs using `data-field-path="/customer/name"` (array indices are explicit); mark actions with `data-action="invoke"`, `submit-input`, `cancel`, `upload-file`, `download-file`, `list-tasks` or `send-message`. These markers support checks and automation, without prescribing page layout.

Wrap every pending-input form in `data-wait-id="<waitId>"`. Mark a task history item with `data-task-id` and a file action with `data-artifact-id`. Array add/remove buttons use `data-action="add-item"` / `remove-item` and `data-field-path` for the array or row. Nested fields keep their full JSON Pointer. These attributes must describe the real SDK state, never invented identities.

Assign row indices immediately when adding or removing a row, before submission. For example, the second row's name input uses `/items/1/name`, and its remove button uses `/items/1`. Reindex the remaining rows after removal. Preview checks at least two rows when the schema permits them and reports absent fields and row actions.

A scalar reply (for example a wait with `{"type":"string"}`) uses the empty root JSON Pointer: `data-field-path=""`. Submit the scalar itself as `content`, rather than wrapping it in an invented object.

Native form constraints must accept the frozen schema's valid values. A `number` can contain a fractional value; use `step="any"` when no `multipleOf` is declared, rather than silently restricting it to integers or cents. An `integer` remains integral. Preview checks include valid fractional samples and report the control's native validation message. A wait form is checked against that wait's response schema, separately from the initial workflow input object.

Preview actively clicks array add/remove actions even when enough rows already exist. Respect `minItems` and `maxItems`; a fixed-length array needs no impossible add/remove action. Connect each actual button to its handler, update row paths immediately, and do not include extra blank rows in the submitted business input. Row controls must not submit an application task.

While a wait form contains typed synthetic input, preview sends a genuine new task revision without changing that waitId. Keep the active form and its input nodes and values until the wait closes. Replacing all wait forms during status rendering loses user input and fails inspection. An absent or null responseSchema accepts raw JSON; a declared schema remains authoritative.

Synthetic history also contains a task from another publication. Direct getTask/subscribeTask calls for that task are rejected, as they are in a live page with a different binding. Clicking its marked history item must call openTask. This verifies the history action in simulation; loading and using the actual original HTML/SDK/Bundle is verified separately through the formal employee page.
