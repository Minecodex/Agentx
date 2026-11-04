"use strict";
/** Serialized into the opaque page before any generated script. This function must be self-contained. */
function installPageSdk(bindingId) {
    let port;
    let sequence = 0;
    const pending = new Map();
    const subscriptions = new Map();
    const errors = [];
    const waitSnapshots = new Map();
    let businessSubmissions = 0;
    addEventListener('error', event => { if (errors.length < 50)
        errors.push({ code: 'PAGE_SCRIPT_ERROR', message: (event.message || 'A page script failed').slice(0, 1000), line: event.lineno, column: event.colno }); }, true);
    addEventListener('unhandledrejection', event => { if (errors.length < 50)
        errors.push({ code: 'PAGE_ASYNC_ERROR', message: String(event.reason?.message ?? event.reason).slice(0, 1000) }); });
    function visibleText() { return (document.body?.innerText ?? '').slice(0, 8000); }
    function visible(element) { return element.checkVisibility({opacityProperty:true,visibilityProperty:true}) && !element.closest('[inert]'); }
    let acceptPort;
    let rejectPort;
    const ready = new Promise((resolve, reject) => { acceptPort = resolve; rejectPort = reject; });
    const readyTimer = setTimeout(() => rejectPort?.(Object.assign(new Error('The application page could not connect'), { code: 'PAGE_DISCONNECTED' })), 10_000);
    void ready.catch(() => undefined);
    const receive = (event) => {
        if (event.source !== parent || event.data?.type !== 'agentx.page.bind' || event.data.bindingId !== bindingId || port || event.ports.length !== 1)
            return;
        event.stopImmediatePropagation();
        port = event.ports[0];
        port.onmessage = event => {
            const message = event.data;
            if (!message || typeof message !== 'object')
                return;
            if (message.type === 'inspect' && typeof message.id === 'string' && message.id.length < 160) {
                void inspect(message.input, message.submit === true, message.waitId, message.options ?? {}).then(report => port?.postMessage({ type: 'inspection', id: message.id, report })).catch(error => port?.postMessage({ type: 'inspection', id: message.id, report: { errors: [{ code: 'PAGE_INSPECTION_FAILED', message: String(error).slice(0, 1000) }], fields: [], actions: [], visibleText: visibleText() } }));
                return;
            }
            if (message.type === 'task' && typeof message.subscriptionId === 'string') {
                subscriptions.get(message.subscriptionId)?.(message.task);
                return;
            }
            if (message.type !== 'response' || typeof message.id !== 'string')
                return;
            const request = pending.get(message.id);
            if (!request)
                return;
            pending.delete(message.id);
            clearTimeout(request.timer);
            if (message.error)
                request.reject(Object.assign(new Error(message.error.message ?? 'Application request failed'), { code: message.error.code, fieldErrors: message.error.fieldErrors ?? [] }));
            else
                request.resolve(message.result);
        };
        port.start();
        clearTimeout(readyTimer);
        acceptPort?.();
        acceptPort = undefined;
        rejectPort = undefined;
    };
    addEventListener('message', receive, { capture: true });
    function request(method, args, transfer = []) {
        return ready.then(() => new Promise((resolve, reject) => {
            if (!port || pending.size >= 16) {
                reject(Object.assign(new Error('Application page is busy or disconnected'), { code: 'PAGE_BUSY' }));
                return;
            }
            const id = `${bindingId}_${++sequence}`;
            const timer = setTimeout(() => { pending.delete(id); reject(Object.assign(new Error('The request has not returned; check the original task before retrying'), { code: 'PAGE_REQUEST_TIMEOUT' })); }, 90_000);
            pending.set(id, { resolve, reject, timer });
            try {
                port.postMessage({ type: 'request', id, method, args }, transfer);
            }
            catch {
                pending.delete(id);
                clearTimeout(timer);
                reject(Object.assign(new Error('The page request could not be transferred'), { code: 'PAGE_REQUEST_INVALID' }));
            }
        }));
    }
    async function inspect(input, submit, waitId, options) {
        const pause = () => new Promise(resolve => setTimeout(resolve, 30));
        const issues = [...errors];
        const knownErrors = errors.length;
        if (options.historyBatchId) {
            const item=()=>Array.from(document.querySelectorAll('[data-batch-id]')).find(e=>e.getAttribute('data-batch-id')===options.historyBatchId);
            if(!item())document.querySelector('[data-action="list-batches"]')?.click();
            for(let i=0;!item()&&i<50;i++)await pause();
            const entry=item();if(!entry)issues.push({code:'PAGE_BATCH_HISTORY_MISSING',message:'Batch history must expose data-batch-id and open the original page through openBatch'});else{(entry.querySelector('button,a')??entry).click();await pause()}
            return {errors:issues,fields:[],actions:[],visibleText:visibleText()};
        }
        if (options.historyTaskId) {
            const item = () => Array.from(document.querySelectorAll('[data-task-id]')).find(element => element.getAttribute('data-task-id') === options.historyTaskId);
            if (!item()) document.querySelector('[data-action="list-tasks"]')?.click();
            for (let index = 0; !item() && index < 50; index += 1) await pause();
            const entry = item();
            if (!entry) issues.push({code:'PAGE_HISTORY_ITEM_MISSING',message:'The history list must expose the previous-version task using its data-task-id. Refresh or show all owned history entries.'});
            else { (entry.querySelector('button,a') ?? entry).click(); await pause(); }
            issues.push(...errors.slice(knownErrors));
            return {errors:issues,fields:[],actions:[],visibleText:visibleText()};
        }
        let scope = waitId ? Array.from(document.querySelectorAll('[data-wait-id]')).find(element => element.getAttribute('data-wait-id') === waitId) : document.body;
        if (!scope)
            return { errors: [...issues, { code: options.waitCheck === 'verify' ? 'PAGE_WAIT_FORM_REPLACED' : 'PAGE_WAIT_FORM_MISSING', message: 'The active input wait has no form with its waitId' }], fields: [], actions: [], visibleText: visibleText() };
        if(!waitId&&!options.batchInputs){
            const selector=options.submitAction?`[data-action="${options.submitAction}"]`:'[data-action="invoke"], [data-action="send-message"]';
            const primary=Array.from(document.querySelectorAll(selector)).find(visible);
            scope=primary?.closest('form')??scope;
        }
        function fields() { return Array.from(scope.querySelectorAll('input:not([type=hidden]),select,textarea')); }
        const valueOf = field => field instanceof HTMLInputElement && (field.type === 'checkbox' || field.type === 'radio') ? field.checked : field.value;
        if (options.waitCheck === 'verify') {
            const saved = waitSnapshots.get(waitId);
            if (!saved || saved.some(item => !item.field.isConnected || !scope.contains(item.field))) issues.push({code:'PAGE_WAIT_FORM_REPLACED',message:'A task status update replaced an active wait form. Keep the same waitId form and its input nodes until that wait closes.'});
            else for (const item of saved) if (valueOf(item.field) !== item.value) issues.push({code:'PAGE_WAIT_VALUE_LOST',message:`A task status update lost the typed value of ${item.path || '(root)'}. Preserve active wait input values.`});
            waitSnapshots.delete(waitId);
        }
        const checkedArrays = new Set();
        function action(name, path) { return Array.from(scope.querySelectorAll(`[data-action="${name}"]`)).find(element => element.getAttribute('data-field-path') === path); }
        function rowCount(path) {
            const indices = new Set(fields().map(field => field.getAttribute('data-field-path') ?? '').filter(pointer => pointer.startsWith(path + '/')).map(pointer => pointer.slice(path.length + 1).split('/')[0]).filter(index => /^(0|[1-9]\d*)$/.test(index)).map(Number));
            return indices.size;
        }
        async function checkArray(path, schema) {
            if (!schema || checkedArrays.has(path)) return false;
            checkedArrays.add(path);
            const count = rowCount(path), minimum = Number(schema.minItems ?? 0), maximum = Number(schema.maxItems ?? Infinity);
            const before = businessSubmissions;
            let removedExisting = false;
            if (count < maximum) {
                const add = action('add-item', path);
                if (!add) { issues.push({code:'PAGE_ARRAY_ADD_MISSING',message:`The array ${path || '(root)'} needs an add-item action`}); return false; }
                add.click(); await pause();
                if (rowCount(path) !== count + 1) { issues.push({code:'PAGE_ARRAY_ADD_NOT_WORKING',message:`Clicking add-item for ${path || '(root)'} must add exactly one row and assign its field paths immediately. Connect the real button to the row creation handler.`}); return false; }
                const remove = action('remove-item', `${path}/${count}`);
                if (!remove) issues.push({code:'PAGE_ARRAY_REMOVE_MISSING',message:`The added row ${path}/${count} needs its remove-item action`});
                else { remove.click(); await pause(); if (rowCount(path) !== count) issues.push({code:'PAGE_ARRAY_REMOVE_NOT_WORKING',message:`Removing the added row must restore the original ${path || '(root)'} rows and paths`}); }
            } else if (count > minimum) {
                const remove = action('remove-item', `${path}/${count - 1}`), add = action('add-item', path);
                if (!remove || !add) issues.push({code:'PAGE_ARRAY_ACTION_MISSING',message:`The variable-length array ${path || '(root)'} needs functional add/remove actions`});
                else {
                    remove.click(); await pause();
                    if (rowCount(path) !== count - 1) issues.push({code:'PAGE_ARRAY_REMOVE_NOT_WORKING',message:`The remove-item action for ${path}/${count - 1} did not remove its row`});
                    else { add.click(); await pause(); removedExisting = true; if (rowCount(path) !== count) issues.push({code:'PAGE_ARRAY_ADD_NOT_WORKING',message:`The add-item action for ${path || '(root)'} did not restore the removed row`}); }
                }
            }
            if (businessSubmissions !== before) issues.push({code:'PAGE_ARRAY_ACTION_INVOKED',message:'Array add/remove controls must not submit a business task. Use non-submitting buttons.'});
            return removedExisting;
        }
        async function fill(value, path = '', rawSchema = options.schema) {
            const schema = rawSchema?.oneOf?.[0] ?? rawSchema?.anyOf?.[0] ?? rawSchema;
            const upload = fields().find(field => field.getAttribute('data-field-path') === path && field instanceof HTMLInputElement && field.type === 'file');
            if (upload) {
                const files = new DataTransfer(), count = Array.isArray(value) ? value.length : 1;
                if (count > 1 && !upload.multiple) issues.push({code:'PAGE_MULTIPLE_FILES_MISSING',message:`The file input ${path} must accept multiple attachments`});
                for (let index = 0; index < count; index += 1) files.items.add(new File(['Synthetic preview attachment'], `preview${index}.txt`, {type:upload.accept.split(',')[0]?.includes('/') ? upload.accept.split(',')[0] : 'text/plain'}));
                upload.files=files.files;upload.dispatchEvent(new Event('change',{bubbles:true}));await pause();return;
            }
            if (value && typeof value === 'object' && !Array.isArray(value)) {
                for (const [key, child] of Object.entries(value))
                    await fill(child, `${path}/${key.replaceAll('~', '~0').replaceAll('/', '~1')}`, schema?.properties?.[key]);
                return;
            }
            if (Array.isArray(value)) {
                for (let index = 0; index < value.length; index += 1) {
                    if (!fields().some(field => field.getAttribute('data-field-path') === `${path}/${index}` || field.getAttribute('data-field-path')?.startsWith(`${path}/${index}/`))) {
                        const add = Array.from(scope.querySelectorAll('[data-action="add-item"]')).find(button => button.getAttribute('data-field-path') === path);
                        if (!add) issues.push({code:'PAGE_ARRAY_ADD_MISSING',message:`The array ${path} needs a marked add-item action`});
                        add?.click();
                        await pause();
                    }
                    await fill(value[index], `${path}/${index}`, schema?.items);
                    if ((!schema || Number(schema.minItems ?? 0) !== Number(schema.maxItems ?? Infinity)) && !action('remove-item', `${path}/${index}`)) issues.push({code:'PAGE_ARRAY_REMOVE_MISSING',message:`The row ${path}/${index} needs a marked remove-item action`});
                }
                if (await checkArray(path, schema)) for (let index = 0; index < value.length; index += 1) await fill(value[index], `${path}/${index}`, schema?.items);
                return;
            }
            const field = fields().find(field => field.getAttribute('data-field-path') === path);
            if (!field) issues.push({code:'PAGE_BUSINESS_FIELD_MISSING',message:`The business field ${path || '(root)'} has no input with its exact JSON Pointer`});
            if (!field || value === null || value === undefined)
                return;
            if (!(field instanceof HTMLInputElement && field.type === 'file') && !visible(field))
                issues.push({code:'PAGE_FIELD_HIDDEN',message:`The business field ${path || '(root)'} is hidden. Show the active form and its ancestors before accepting input.`});
            if (field instanceof HTMLInputElement && field.type === 'checkbox')
                field.checked = Boolean(value);
            else if (field instanceof HTMLInputElement && field.type === 'file') {
                const files = new DataTransfer();
                files.items.add(new File(['Synthetic preview attachment'], 'preview.txt', { type: 'text/plain' }));
                field.files = files.files;
            }
            else
                field.value = String(value);
            field.dispatchEvent(new Event('input', { bubbles: true }));
            field.dispatchEvent(new Event('change', { bubbles: true }));
            await pause();
        }
        function describeFields() {
            return fields().map(field => {
                const label = field.getAttribute('aria-label') || field.getAttribute('aria-labelledby')?.split(/\s+/).map(id => document.getElementById(id)?.textContent ?? '').join(' ') || Array.from(field.labels ?? []).map(label => label.textContent ?? '').join(' ');
                const path = field.getAttribute('data-field-path');
                if (path === null)
                    issues.push({ code: 'PAGE_FIELD_MARKER_MISSING', message: `A form input (${field.name || field.id || field.tagName}) is missing data-field-path` });
                if (!label?.trim())
                    issues.push({ code: 'PAGE_FIELD_LABEL_MISSING', message: `The input ${path ?? field.name} has no accessible label` });
                if ((submit || options.waitCheck === 'prepare') && field.validity && !field.validity.valid)
                    issues.push({ code: 'PAGE_FIELD_CONSTRAINT_MISMATCH', message: `The schema sample for ${path || '(root)'} is rejected by this control: ${field.validationMessage}` });
                return { path, label, type: field instanceof HTMLInputElement ? field.type : field.tagName.toLowerCase(), required: field.required };
            });
        }
        let entries;
        if (submit || options.waitCheck === 'prepare') {
            if(options.batchInputs){
                document.querySelector('[data-action="batch-mode"]')?.click();await pause();
                const rows=()=>Array.from(document.querySelectorAll('[data-batch-row]')).filter(visible);
                for(let i=0;rows().length<options.batchInputs.length&&i<100;i++){const before=rows().length;document.querySelector('[data-action="add-batch-item"]')?.click();await pause();if(rows().length===before)break}
                for(let i=0;rows().length>options.batchInputs.length&&i<100;i++){const before=rows().length;rows().at(-1)?.querySelector('[data-action="remove-batch-item"]')?.click();await pause();if(rows().length===before)break}
                if(rows().length!==options.batchInputs.length)issues.push({code:'PAGE_BATCH_ROW_COUNT',message:'Batch add/remove controls must produce exactly the requested rows'});
                const ids=rows().map(row=>row.getAttribute('data-client-item-id'));
                if(ids.some(id=>!id)||new Set(ids).size!==ids.length)issues.push({code:'PAGE_BATCH_ROW_ID',message:'Every batch row needs its own stable data-client-item-id'});
                entries=[];
                for(let i=0;i<Math.min(rows().length,options.batchInputs.length);i++){scope=rows()[i];await fill(options.batchInputs[i]);entries.push(...describeFields())}
                scope=document.body;
            }else await fill(input);
            for (const field of fields())
                if (field instanceof HTMLInputElement && field.type === 'file' && !field.files?.length) {
                    const files = new DataTransfer();
                    files.items.add(new File(['Synthetic preview attachment'], 'preview.txt', { type: field.accept.split(',')[0]?.includes('/') ? field.accept.split(',')[0] : 'text/plain' }));
                    field.files = files.files;
                    field.dispatchEvent(new Event('change', { bubbles: true }));
                }
            await pause();
            // A successful submission can replace/remove its form. Inspect its
            // labels while the controls still belong to the live document.
            entries ??= describeFields();
            if (options.waitCheck === 'prepare') waitSnapshots.set(waitId, fields().map(field => ({field,path:field.getAttribute('data-field-path'),value:valueOf(field)})));
        }
        if (submit) {
            const selector=waitId?'[data-action="submit-input"]':options.submitAction?`[data-action="${options.submitAction}"]`:'[data-action="invoke"], [data-action="send-message"]';
            const candidates = [...(scope.matches(selector) ? [scope] : []), ...scope.querySelectorAll(selector)];
            const action = candidates.find(element => !element.querySelector(selector));
            if (!action)
                issues.push({ code: 'PAGE_SUBMIT_ACTION_MISSING', message: 'A marked business submit action is required' });
            else {
                for (let index = 0; action.disabled && index < 30; index += 1)
                    await pause();
                if (!visible(action)) issues.push({code:'PAGE_SUBMIT_ACTION_HIDDEN',message:'The marked submit action is hidden by its own or an ancestor style. Show the active form before submission.'});
                else if (action.disabled) issues.push({code:'PAGE_SUBMIT_ACTION_DISABLED',message:'The marked submit action remains disabled after valid fields are filled.'});
                else { if (action instanceof HTMLFormElement) action.requestSubmit(); else action.click(); await pause(); }
            }
        }
        entries ??= describeFields();
        for (const element of document.querySelectorAll('base,object,embed,iframe,meta[http-equiv="refresh"],script[src],link[href]'))
            issues.push({ code: 'PAGE_RESOURCE_FORBIDDEN', message: `Remove unsupported external or navigable element: ${element.tagName}` });
        issues.push(...errors.slice(knownErrors));
        return { errors: issues, fields: entries, actions: Array.from(document.querySelectorAll('[data-action]')).map(element => element.getAttribute('data-action')), visibleText: visibleText() };
    }
    // Coalesce only concurrent identical submissions; a later intentional task remains new.
    const activeSubmissions = new Map();
    function submit(method, args) {
        const key = method + ':' + JSON.stringify(args);
        const active = activeSubmissions.get(key);
        if (active) return active;
        const pending = request(method, args);
        if (method === 'invoke' || method === 'sendMessage' || method === 'createBatch') businessSubmissions += 1;
        activeSubmissions.set(key, pending);
        void pending.finally(() => { if (activeSubmissions.get(key) === pending) activeSubmissions.delete(key); }).catch(() => undefined);
        return pending;
    }
    const sdk = {
        getContext: () => request('getContext', {}),
        createBatch: (batch) => submit('createBatch', batch),
        listMyBatches: (query = {}) => request('listMyBatches', { query }),
        getBatch: (batchId) => request('getBatch', { batchId }),
        listBatchItems: (batchId, query = {}) => request('listBatchItems', { batchId, query }),
        openBatch: (batchId) => request('openBatch', { batchId }),
        stopBatch: (batchId) => submit('stopBatch', { batchId }),
        cancelBatch: (batchId) => submit('cancelBatch', { batchId }),
        rerunFailedItems: (batchId, itemIds) => submit('rerunFailedItems', { batchId, itemIds }),
        exportBatch: (batchId) => submit('exportBatch', { batchId }),
        getBatchExport: (batchId, exportId) => request('getBatchExport', { batchId, exportId }),
        downloadBatchExport: (batchId, exportId) => request('downloadBatchExport', { batchId, exportId }),
        readTable: (source, delimiter = '\t') => request('readTable', typeof source === 'string' ? { text: source, delimiter } : { artifactId: source.artifactId, delimiter }),
        invoke: (input) => submit('invoke', { input }),
        getTask: (taskId) => request('getTask', { taskId }),
        listMyTasks: (query = {}) => request('listMyTasks', { query }),
        submitTaskInput: (taskId, waitId, content) => submit('submitTaskInput', { taskId, waitId, content }),
        cancelTask: (taskId) => submit('cancelTask', { taskId }),
        openTask: (taskId) => request('openTask', { taskId }),
        openApproval: (approvalId) => request('openApproval', { approvalId }),
        downloadFile: (taskId, artifactId) => request('downloadFile', { taskId, artifactId }),
        async uploadFile(file) {
            if (!(file instanceof File) || file.size > 50 * 1024 * 1024)
                throw Object.assign(new Error('Select a file of at most 50 MiB'), { code: 'PAGE_FILE_INVALID' });
            const bytes = await file.arrayBuffer();
            return request('uploadFile', { name: file.name, contentType: file.type, bytes }, [bytes]);
        },
        async subscribeTask(taskId, onTask) {
            if (typeof onTask !== 'function')
                throw new Error('A task callback is required');
            const subscriptionId = `subscription_${++sequence}`;
            subscriptions.set(subscriptionId, onTask);
            try {
                await request('subscribeTask', { taskId, subscriptionId });
            }
            catch (error) {
                subscriptions.delete(subscriptionId);
                throw error;
            }
            return () => { subscriptions.delete(subscriptionId); void request('unsubscribeTask', { subscriptionId }).catch(() => undefined); };
        },
        createSession: (input = {}) => request('createSession', { input }),
        getSession: (sessionId) => request('getSession', { sessionId }),
        sendMessage: (sessionId, text, files = []) => submit('sendMessage', { sessionId, text, files }),
    };
    Object.defineProperty(window, 'agentx', { value: Object.freeze(sdk), configurable: false, writable: false });
    parent.postMessage({ type: 'agentx.page.ready', bindingId }, '*');
    addEventListener('pagehide', () => { clearTimeout(readyTimer); rejectPort?.(Object.assign(new Error('The page was closed'), { code: 'PAGE_DISCONNECTED' })); port?.close(); for (const request of pending.values()) {
        clearTimeout(request.timer);
        request.reject(Object.assign(new Error('The page was closed'), { code: 'PAGE_DISCONNECTED' }));
    } pending.clear(); subscriptions.clear(); }, { once: true });
}

installPageSdk(__AGENTX_PAGE_BINDING__);
