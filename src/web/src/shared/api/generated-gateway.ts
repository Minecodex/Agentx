/**
 * Auto-generated from contracts/openapi/trigger-gateway.json. Do not edit directly.
 */

export interface components {
    schemas: {
        /** ApplicationPageViewV1 */
        ApplicationPageView: {
            historical: boolean;
            html: string;
            publication: components["schemas"]["ApplicationPageView_ApplicationPagePublicationV1"];
            routeKey: string;
            sdkScript: string;
            stylesheet: string;
            /** Format: uuid */
            taskId?: string | null;
        };
        /** @enum {string} */
        ApplicationPageView_ApplicationApprovalKindV1: "workflow_node" | "agent_tool_effect";
        ApplicationPageView_ApplicationApprovalStageV1: {
            kind: components["schemas"]["ApplicationPageView_ApplicationApprovalKindV1"];
            nodeId: string;
            title: string;
        };
        ApplicationPageView_ApplicationPageContractV1: {
            /** Format: uuid */
            applicationId: string;
            applicationName: string;
            approvalStages: components["schemas"]["ApplicationPageView_ApplicationApprovalStageV1"][];
            /** Format: uuid */
            bundleId: string;
            chatMapping?: components["schemas"]["ApplicationPageView_ChatMappingV1"] | null;
            /** Format: uuid */
            deploymentId: string;
            description?: string | null;
            inputSchema: unknown;
            /** Format: uint64 */
            maxUploadBytes: number;
            outputSchema: unknown;
            /** Format: uint32 */
            sdkVersion: number;
            waitInputNodes: components["schemas"]["ApplicationPageView_WaitInputNodeV1"][];
            /** Format: uuid */
            workflowVersionId: string;
        };
        ApplicationPageView_ApplicationPagePublicationV1: {
            /** Format: uuid */
            applicationId: string;
            contract: components["schemas"]["ApplicationPageView_ApplicationPageContractV1"];
            contractHash: string;
            enabled: boolean;
            /** Format: uint64 */
            expectedRevision: number;
            html: components["schemas"]["ApplicationPageView_RuntimeObjectReferenceV1"];
            htmlHash: string;
            /** Format: uuid */
            publicationId: string;
            /** Format: uint64 */
            revision: number;
            sdk: components["schemas"]["ApplicationPageView_RuntimeObjectReferenceV1"];
            slug: string;
            /** Format: uint32 */
            styleVersion: number;
            stylesheet: components["schemas"]["ApplicationPageView_RuntimeObjectReferenceV1"];
            /** Format: uuid */
            tenantId: string;
            /** Format: uuid */
            uiVersionId: string;
        };
        ApplicationPageView_ChatMappingV1: {
            answerFilesOutput?: string | null;
            answerOutput: string;
            fileInput?: string | null;
            questionInput: string;
        };
        ApplicationPageView_RuntimeObjectReferenceV1: {
            contentHash: string;
            mediaType: string;
            /** Format: uuid */
            objectId: string;
            objectKey: string;
            /** Format: uint64 */
            sizeBytes: number;
            storageDomain: components["schemas"]["ApplicationPageView_StorageDomain"];
            /** Format: uuid */
            tenantId: string;
        };
        /** @enum {string} */
        ApplicationPageView_StorageDomain: "runtime" | "observability";
        ApplicationPageView_WaitInputNodeV1: {
            name: string;
            nodeId: string;
            path: string;
            responseSchema?: unknown;
        };
        /** ArtifactUploadResponseV1 */
        ArtifactUploadResponse: {
            /** Format: uuid */
            artifactId: string;
            contentType: string;
            sha256: string;
            /** Format: uint64 */
            sizeBytes: number;
        };
        /** CommandAcceptedV1 */
        CommandAccepted: {
            accepted: boolean;
            replayed: boolean;
        };
        /** CreateSessionRequestV1 */
        CreateSessionRequest: {
            /** @default {} */
            input: {
                [key: string]: unknown;
            };
            title?: string | null;
        };
        /** GatewayErrorV1 */
        GatewayError: {
            code: string;
            fieldErrors?: components["schemas"]["GatewayError_GatewayFieldErrorV1"][];
            message: string;
        };
        GatewayError_GatewayFieldErrorV1: {
            code: string;
            /** @description Stable JSON Pointer, relative to the business input. */
            field: string;
            message: string;
        };
        /** InvocationRequestV1 */
        InvocationRequest: {
            input: unknown;
            responseMode?: string | null;
            /** Format: uuid */
            sessionId?: string | null;
            /**
             * @description Omitted for formal external input. Explicit presets, including an empty plan, are available only to authenticated platform testing users.
             * @default null
             */
            waitInputs: {
                [key: string]: unknown[];
            } | null;
        };
        /** InvocationResponseV1 */
        InvocationResponse: {
            activeWaits: components["schemas"]["InvocationResponse_InputWaitV1"][];
            /** Format: uint64 */
            admissionEpoch: number;
            /** Format: uuid */
            applicationId: string;
            /** Format: uuid */
            bundleId: string;
            conversationId?: string | null;
            createdAt: string;
            endUser?: components["schemas"]["InvocationResponse_ApplicationUserIdentityV1"] | null;
            error?: unknown;
            /** Format: uuid */
            executionId?: string | null;
            /** Format: uuid */
            id: string;
            messages: components["schemas"]["InvocationResponse_WorkflowMessageV1"][];
            outputs?: unknown;
            provider?: components["schemas"]["InvocationResponse_WebhookProviderV1"] | null;
            providerEventId?: string | null;
            /** Format: uuid */
            sessionId?: string | null;
            status: string;
            triggerContext?: unknown;
            userIdentityDiagnostic?: string | null;
        };
        /** @description The end user a call acts for. API identities are declarations, not authentication. */
        InvocationResponse_ApplicationUserIdentityV1: {
            externalUserId: string;
            source: components["schemas"]["InvocationResponse_ApplicationUserSourceV1"];
            sourceId: string;
        };
        /** @enum {string} */
        InvocationResponse_ApplicationUserSourceV1: "platform" | "channel" | "api_parameter";
        InvocationResponse_InputWaitV1: {
            content?: unknown;
            /** Format: uuid */
            executionId: string;
            expiresAt?: string | null;
            /** Format: uuid */
            nodeExecutionId: string;
            nodeId: string;
            nodePath: string;
            /** Format: uint32 */
            occurrence: number;
            responseSchema?: unknown;
            resumeUrl?: string | null;
            source?: string | null;
            status: string;
            /** Format: uuid */
            waitId: string;
        };
        /** @enum {string} */
        InvocationResponse_WebhookProviderV1: "agentx" | "dingtalk" | "wecom" | "feishu";
        InvocationResponse_WorkflowMessageV1: {
            content: string;
            createdAt: string;
            /** Format: uuid */
            executionId: string;
            /** Format: uuid */
            id: string;
            /** Format: uuid */
            nodeExecutionId: string;
        };
        /** MessagePartInputV1 */
        MessagePartInput: {
            /** Format: uuid */
            artifactId?: string | null;
            content?: unknown;
            partType: string;
        };
        /** MessageRequestV1 */
        MessageRequest: {
            parts: components["schemas"]["MessageRequest_MessagePartInputV1"][];
            /** @default null */
            waitInputs: {
                [key: string]: unknown[];
            } | null;
        };
        MessageRequest_MessagePartInputV1: {
            /** Format: uuid */
            artifactId?: string | null;
            content?: unknown;
            partType: string;
        };
        /** MessageResponseV1 */
        MessageResponse: {
            createdAt: string;
            /** Format: uuid */
            id: string;
            /** Format: uuid */
            invocationId?: string | null;
            parts: components["schemas"]["MessageResponse_MessagePartInputV1"][];
            role: string;
            /** Format: uint64 */
            sequence: number;
        };
        MessageResponse_MessagePartInputV1: {
            /** Format: uuid */
            artifactId?: string | null;
            content?: unknown;
            partType: string;
        };
        /** MyTasksPageV1 */
        MyTasksPage: {
            items: components["schemas"]["MyTasksPage_TaskViewV1"][];
            /** @description Opaque page boundary scoped to the same authenticated application and caller. */
            next?: string | null;
        };
        MyTasksPage_ApplicationAccessBindingV1: {
            /** @constant */
            kind: "web";
            /** Format: uuid */
            publicationId: string;
            /** Format: uint64 */
            revision: number;
        } | {
            /** @constant */
            kind: "mcp";
            /** Format: uuid */
            publicationId: string;
            /** Format: uint64 */
            revision: number;
        };
        MyTasksPage_ArtifactUploadResponseV1: {
            /** Format: uuid */
            artifactId: string;
            contentType: string;
            sha256: string;
            /** Format: uint64 */
            sizeBytes: number;
        };
        MyTasksPage_InputWaitV1: {
            content?: unknown;
            /** Format: uuid */
            executionId: string;
            expiresAt?: string | null;
            /** Format: uuid */
            nodeExecutionId: string;
            nodeId: string;
            nodePath: string;
            /** Format: uint32 */
            occurrence: number;
            responseSchema?: unknown;
            resumeUrl?: string | null;
            source?: string | null;
            status: string;
            /** Format: uuid */
            waitId: string;
        };
        MyTasksPage_TaskActionsV1: {
            canCancel: boolean;
            downloadableArtifactIds: string[];
            inputWaitIds: string[];
        };
        MyTasksPage_TaskApprovalV1: {
            /** Format: uuid */
            approvalId: string;
            expiresAt?: string | null;
            status: string;
            title: string;
            /** Format: uint64 */
            version: number;
        };
        MyTasksPage_TaskStageV1: {
            /** Format: uuid */
            nodeExecutionId: string;
            status: string;
            title: string;
        };
        /** @enum {string} */
        MyTasksPage_TaskStateV1: "accepted" | "running" | "waiting_input" | "waiting_approval" | "completed" | "failed" | "cancelled" | "outcome_unknown";
        MyTasksPage_TaskViewV1: {
            accessBinding?: components["schemas"]["MyTasksPage_ApplicationAccessBindingV1"] | null;
            allowedActions: components["schemas"]["MyTasksPage_TaskActionsV1"];
            /** Format: uuid */
            applicationId: string;
            /** Format: uuid */
            bundleId: string;
            callerType: string;
            createdAt: string;
            /** Format: uuid */
            deploymentId: string;
            error?: unknown;
            /** Format: uint64 */
            eventCursor: number;
            files: components["schemas"]["MyTasksPage_ArtifactUploadResponseV1"][];
            invocationStatus: string;
            messages: components["schemas"]["MyTasksPage_WorkflowMessageV1"][];
            outputs?: unknown;
            pendingApprovals: components["schemas"]["MyTasksPage_TaskApprovalV1"][];
            pendingInputs: components["schemas"]["MyTasksPage_InputWaitV1"][];
            /** Format: uint64 */
            revision: number;
            /** Format: uuid */
            sessionId?: string | null;
            stages: components["schemas"]["MyTasksPage_TaskStageV1"][];
            state: components["schemas"]["MyTasksPage_TaskStateV1"];
            /** Format: uuid */
            taskId: string;
            taskUrl?: string | null;
            /** Format: uuid */
            workflowVersionId: string;
        };
        MyTasksPage_WorkflowMessageV1: {
            content: string;
            createdAt: string;
            /** Format: uuid */
            executionId: string;
            /** Format: uuid */
            id: string;
            /** Format: uuid */
            nodeExecutionId: string;
        };
        /** SessionResponseV1 */
        SessionResponse: {
            /** Format: uuid */
            applicationDeploymentId: string;
            /** Format: uuid */
            applicationId: string;
            /** Format: uuid */
            bundleId?: string | null;
            endUser?: components["schemas"]["SessionResponse_ApplicationUserIdentityV1"] | null;
            externalUserId?: string | null;
            /** Format: uuid */
            id: string;
            status: string;
            title?: string | null;
            updatedAt: string;
            /** Format: uint64 */
            version: number;
            versionPolicy: components["schemas"]["SessionResponse_SessionVersionPolicyV1"];
            /** Format: uuid */
            workflowVersionId?: string | null;
        };
        /** @description The end user a call acts for. API identities are declarations, not authentication. */
        SessionResponse_ApplicationUserIdentityV1: {
            externalUserId: string;
            source: components["schemas"]["SessionResponse_ApplicationUserSourceV1"];
            sourceId: string;
        };
        /** @enum {string} */
        SessionResponse_ApplicationUserSourceV1: "platform" | "channel" | "api_parameter";
        /** @enum {string} */
        SessionResponse_SessionVersionPolicyV1: "pinned" | "follow_deployment" | "manual_upgrade";
        /** SubmitPage */
        SubmitPage: {
            input: unknown;
            /** Format: uuid */
            publicationId: string;
            /** Format: uint64 */
            revision: number;
        };
        /** SubmitWaitInputV1 */
        SubmitWaitInput: {
            content: unknown;
        };
        /** TaskViewV1 */
        TaskView: {
            accessBinding?: components["schemas"]["TaskView_ApplicationAccessBindingV1"] | null;
            allowedActions: components["schemas"]["TaskView_TaskActionsV1"];
            /** Format: uuid */
            applicationId: string;
            /** Format: uuid */
            bundleId: string;
            callerType: string;
            createdAt: string;
            /** Format: uuid */
            deploymentId: string;
            error?: unknown;
            /** Format: uint64 */
            eventCursor: number;
            files: components["schemas"]["TaskView_ArtifactUploadResponseV1"][];
            invocationStatus: string;
            messages: components["schemas"]["TaskView_WorkflowMessageV1"][];
            outputs?: unknown;
            pendingApprovals: components["schemas"]["TaskView_TaskApprovalV1"][];
            pendingInputs: components["schemas"]["TaskView_InputWaitV1"][];
            /** Format: uint64 */
            revision: number;
            /** Format: uuid */
            sessionId?: string | null;
            stages: components["schemas"]["TaskView_TaskStageV1"][];
            state: components["schemas"]["TaskView_TaskStateV1"];
            /** Format: uuid */
            taskId: string;
            taskUrl?: string | null;
            /** Format: uuid */
            workflowVersionId: string;
        };
        TaskView_ApplicationAccessBindingV1: {
            /** @constant */
            kind: "web";
            /** Format: uuid */
            publicationId: string;
            /** Format: uint64 */
            revision: number;
        } | {
            /** @constant */
            kind: "mcp";
            /** Format: uuid */
            publicationId: string;
            /** Format: uint64 */
            revision: number;
        };
        TaskView_ArtifactUploadResponseV1: {
            /** Format: uuid */
            artifactId: string;
            contentType: string;
            sha256: string;
            /** Format: uint64 */
            sizeBytes: number;
        };
        TaskView_InputWaitV1: {
            content?: unknown;
            /** Format: uuid */
            executionId: string;
            expiresAt?: string | null;
            /** Format: uuid */
            nodeExecutionId: string;
            nodeId: string;
            nodePath: string;
            /** Format: uint32 */
            occurrence: number;
            responseSchema?: unknown;
            resumeUrl?: string | null;
            source?: string | null;
            status: string;
            /** Format: uuid */
            waitId: string;
        };
        TaskView_TaskActionsV1: {
            canCancel: boolean;
            downloadableArtifactIds: string[];
            inputWaitIds: string[];
        };
        TaskView_TaskApprovalV1: {
            /** Format: uuid */
            approvalId: string;
            expiresAt?: string | null;
            status: string;
            title: string;
            /** Format: uint64 */
            version: number;
        };
        TaskView_TaskStageV1: {
            /** Format: uuid */
            nodeExecutionId: string;
            status: string;
            title: string;
        };
        /** @enum {string} */
        TaskView_TaskStateV1: "accepted" | "running" | "waiting_input" | "waiting_approval" | "completed" | "failed" | "cancelled" | "outcome_unknown";
        TaskView_WorkflowMessageV1: {
            content: string;
            createdAt: string;
            /** Format: uuid */
            executionId: string;
            /** Format: uuid */
            id: string;
            /** Format: uuid */
            nodeExecutionId: string;
        };
        /** WaitInputReceiptV1 */
        WaitInputReceipt: {
            /** Format: uuid */
            executionId: string;
            /** Format: uuid */
            receiptId: string;
            status: string;
            /** Format: uuid */
            waitId: string;
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
