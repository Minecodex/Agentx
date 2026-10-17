export const applicationIntegrationDocs = {
  openApiKey: 'API integration guide',
  openWebhook: 'Webhook integration guide',
  openWebhookSpecific: 'View integration',
  overview: 'Quick start',
  apiReference: 'API reference',
  schemas: 'Schemas',
  security: 'Security',
  errors: 'Troubleshooting',
  endpoint: 'Endpoint',
  fields: 'Request fields',
  responseFields: 'Response fields',
  requestExample: 'Request examples',
  requestSchema: 'Workflow input schema',
  responseSchema: 'Workflow output schema',
  schemaSource: 'Schemas come from the current active application deployment and do not depend on an API key or Webhook being created.',
  noActiveDeployment: 'This application has no active deployment. After deployment, this guide will automatically show that version\'s actual input and output schemas.',
  copyCode: 'Copy code',
  copied: 'Copied',
  close: 'Close',
  table: { location: 'Location', name: 'Field', type: 'Type', required: 'Required', description: 'Description', yes: 'Yes', no: 'No' },
  languages: { curl: 'Curl', java: 'Java', go: 'Go', node: 'Node.js', python: 'Python' },
  apiKey: {
    title: 'API key integration guide',
    description: 'Invoke this application, manage sessions, and track invocations with an API key.',
    intro: 'An API key is a Bearer credential for the Application API. You can inspect this guide and the deployed schemas immediately; a key is only required when sending a request.',
    stepsTitle: 'Integration steps',
    steps: [
      'Confirm that the application has an active deployment and inspect its input and output fields under Schemas.',
      'Create an API key and immediately store the one-time secret in a secret manager.',
      'Inject it into the caller as AGENTX_API_KEY. Never put it in source code, logs, or a browser application.',
      'Generate a stable unique Idempotency-Key for each write operation and reuse it for safe retries.',
      'Store the returned Invocation ID to inspect status, subscribe to SSE, or cancel the run.',
    ],
    headersTitle: 'Common headers',
    headers: [
      ['Header', 'Authorization', 'string', true, 'Bearer $AGENTX_API_KEY; required on every endpoint.'],
      ['Header', 'Idempotency-Key', 'string (1–192)', true, 'Required for writes; retries of the same operation must reuse it.'],
      ['Header', 'Content-Type', 'application/json', true, 'Required when a JSON body is present.'],
    ],
    endpoints: [
      {
        id: 'createInvocation', method: 'POST', path: '/applications/{slug}/invocations', title: 'Create a stateless invocation', description: 'Creates an asynchronous Invocation against the active deployment and returns 202.', responseKind: 'invocation',
        fields: [
          ['Path', 'slug', 'string', true, 'Current application slug; already filled in the examples.'], ['Header', 'Authorization', 'Bearer API key', true, 'An API key for this application.'], ['Header', 'Idempotency-Key', 'string', true, 'Unique business-operation value; reuse for retries.'],
          ['Body', 'input', 'object', true, 'Workflow input that must match the input schema.'], ['Body', 'responseMode', 'string | null', false, 'Response mode; examples use async.'], ['Body', 'sessionId', 'UUID | null', false, 'Optional Session ID; omit for a stateless call.'],
        ],
      },
      {
        id: 'createSession', method: 'POST', path: '/applications/{slug}/sessions', title: 'Create a session', description: 'Creates a Session for multi-turn messages under the deployment version policy and returns 201.', responseKind: 'session',
        fields: [
          ['Path', 'slug', 'string', true, 'Current application slug.'], ['Header', 'Authorization', 'Bearer API key', true, 'An API key for this application.'], ['Header', 'Idempotency-Key', 'string', true, 'Unique value for this session creation.'],
          ['Body', 'title', 'string | null', false, 'Session title.'], ['Body', 'input', 'object', false, 'Initial workflow inputs. The deployment’s selected user ID field identifies this session’s user without identity verification. Later messages reuse these business inputs.'],
        ],
      },
      {
        id: 'sendMessage', method: 'POST', path: '/sessions/{id}/messages', title: 'Send a session message', description: 'Adds a user message to an existing Session, creates an Invocation, and returns 202.', responseKind: 'invocation',
        fields: [
          ['Path', 'id', 'UUID', true, 'Session ID returned when the session was created.'], ['Header', 'Authorization', 'Bearer API key', true, 'Must belong to the Session application.'], ['Header', 'Idempotency-Key', 'string', true, 'Unique value for this send operation.'],
          ['Body', 'parts', 'array', true, 'Ordered message content.'], ['Body', 'parts[].partType', 'string', true, 'Content type such as text, json, image, audio, or file.'], ['Body', 'parts[].content', 'any | null', false, 'Text or structured content; file parts normally use artifactId.'], ['Body', 'parts[].artifactId', 'UUID | null', false, 'ID of a previously uploaded Artifact.'],
        ],
      },
      {
        id: 'getInvocation', method: 'GET', path: '/invocations/{id}', title: 'Get an invocation', description: 'Reads current status, output, and error information and returns 200.', responseKind: 'invocation',
        fields: [['Path', 'id', 'UUID', true, 'Invocation ID returned at creation.'], ['Header', 'Authorization', 'Bearer API key', true, 'Must belong to the Invocation application.']],
      },
      {
        id: 'streamEvents', method: 'GET', path: '/invocations/{id}/events', title: 'Stream SSE events', description: 'Streams Invocation events and resumes after disconnect using the last cursor.', responseKind: 'events',
        fields: [['Path', 'id', 'UUID', true, 'Invocation ID.'], ['Header', 'Authorization', 'Bearer API key', true, 'Must belong to the Invocation application.'], ['Header', 'Accept', 'text/event-stream', false, 'Recommended explicit SSE media type.'], ['Header', 'Last-Event-ID', 'integer ≥ 0', false, 'Resume cursor; omit or use 0 on the first connection.']],
      },
      {
        id: 'cancelInvocation', method: 'POST', path: '/invocations/{id}/cancel', title: 'Cancel an invocation', description: 'Requests cancellation of a running Invocation and returns 202.', responseKind: 'accepted',
        fields: [['Path', 'id', 'UUID', true, 'Invocation ID.'], ['Header', 'Authorization', 'Bearer API key', true, 'Must belong to the Invocation application.'], ['Header', 'Idempotency-Key', 'string', true, 'Unique cancellation command value; reuse for retries.']],
      },
    ],
    responses: {
      invocation: [
        ['id', 'UUID', true, 'Invocation ID.'], ['applicationId', 'UUID', true, 'Application ID.'], ['bundleId', 'UUID', true, 'Execution Bundle pinned for this call.'], ['admissionEpoch', 'integer', true, 'Admission-state version.'], ['status', 'string', true, 'Current invocation status.'],
        ['executionId', 'UUID | null', false, 'Execution ID after runtime admission.'], ['sessionId', 'UUID | null', false, 'Associated Session ID.'], ['outputs', 'any', false, 'Workflow output after completion.'], ['error', 'any', false, 'Failure details.'], ['createdAt', 'string', true, 'Creation time.'],
      ],
      session: [
        ['id', 'UUID', true, 'Session ID.'], ['applicationId', 'UUID', true, 'Application ID.'], ['applicationDeploymentId', 'UUID', true, 'Application deployment used at creation.'], ['versionPolicy', 'string', true, 'pinned, follow_deployment, or manual_upgrade.'], ['workflowVersionId', 'UUID | null', false, 'Currently pinned Workflow version.'],
        ['bundleId', 'UUID | null', false, 'Currently pinned Bundle.'], ['title', 'string | null', false, 'Session title.'], ['externalUserId', 'string | null', false, 'External user identifier.'], ['status', 'string', true, 'Session status.'], ['version', 'integer', true, 'Concurrency-control version.'], ['updatedAt', 'string', true, 'Last update time.'],
      ],
      accepted: [['accepted', 'boolean', true, 'Whether Runtime accepted the command.'], ['replayed', 'boolean', true, 'Whether an existing command was replayed for the same Idempotency-Key.']],
      events: [['id', 'integer', true, 'SSE cursor used by Last-Event-ID.'], ['event', 'string', true, 'Event type.'], ['data', 'JSON string', true, 'Event payload; query the Invocation for authoritative final state.']],
    },
    securityItems: [
      ['Shown once', 'The full API key is shown only after creation or rotation; Agentx stores only a one-way hash.'], ['Distribute minimally', 'Create a separate key for each calling system for independent revocation and auditing.'], ['Rotate immediately', 'Rotate or revoke a suspected leak immediately; the old key stops working.'], ['No untrusted browser', 'Keep API keys in a controlled backend and never ship them to an untrusted client.'],
    ],
  },
  webhook: {
    title: 'Webhook integration guide',
    description: 'Connect DingTalk, WeCom, or Feishu event callbacks to the active application deployment.',
    intro: 'A channel only accepts inbound events and starts the Workflow. Callback mode stores the Token/AES key inside the channel; the production endpoint activates after the application is published and synced. DingTalk and Feishu also support long-connection (Stream) mode with no public URL — the platform is connected outbound after publishing.',
    noSpecificEndpoint: 'No specific channel is selected. Select a channel after publishing to view its one production endpoint or connection status.',
    endpointInactive: 'The production endpoint activates after the channel is saved and published. Copy the one endpoint from Channel details; stream channels have no endpoint and show a connection status instead.',
    stepsTitle: 'Integration steps',
    steps: ['Confirm the input fields of the active deployment under Schemas.', 'Callback mode: configure the callback URL on a DingTalk app bot, WeCom smart bot, or Feishu event subscription. Stream mode: pick Stream/long connection on the DingTalk/Feishu open platform — no public URL needed.', 'Callback mode: paste the one production endpoint from Channel details and configure the platform Token/AES key. Stream mode: fill in Client ID/Secret or App ID/Secret in the channel.', 'Callback mode passes the platform Challenge; stream mode connects automatically after publishing and shows a connection status in Channel details. Text events map to Workflow Start Input and receive an immediate ACK.'],
    signingTitle: 'Platform integration protocol',
    signatureTitle: 'Verification and decryption',
    signatureFormula: 'Callback mode — DingTalk: timestamp + secret signing; WeCom: Token/timestamp/nonce/msg_signature + AES; Feishu: Verification Token/Encrypt Key + request signature. Stream mode authenticates with app credentials over an outbound connection and needs no signature check.',
    fields: [
      ['Request', 'Platform callback', 'JSON/XML', true, 'Challenge or text event sent by the platform-specific adapter.'], ['Field', 'event_id', 'string', true, 'Stable provider event ID used for idempotency.'], ['Field', 'conversation.id', 'string', true, 'Normalized conversation ID used to distinguish multiple groups on one endpoint.'], ['Field', 'message.text', 'string', true, 'Text content mapped to Workflow Start Input.'],
    ],
    securityItems: [
      ['Credential reference', 'Tokens, AES keys, Verification Tokens, and Encrypt Keys stay in Credential/Vault and never appear in API responses or logs.'], ['Challenge', 'URL verification returns the Challenge and never creates an Invocation.'], ['Source tracing', 'Invocation keeps sanitized provider, event ID, conversation.id, and sender.id metadata.'], ['Business replies', 'This phase returns only the platform ACK; Workflow results are not sent back synchronously.'],
    ],
  },
  commonErrors: [
    ['400', 'The callback body or provider event format is invalid.'], ['401', 'The provider token, signature, or decryption check failed.'], ['404', 'The application route, Invocation, or Webhook does not exist or is inactive.'], ['409', 'The same provider event ID was used with different request content.'], ['422', 'Request semantics or application input validation failed.'], ['429', 'The application or tenant rate limit was exceeded.'], ['503', 'Runtime or a dependency is temporarily unavailable.'],
  ],
} as const
