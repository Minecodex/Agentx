/**
 * Auto-generated from contracts/openapi/trigger-gateway.json. Do not edit directly.
 */

export interface components {
    schemas: {
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
            message: string;
        };
        /** InvocationRequestV1 */
        InvocationRequest: {
            input: unknown;
            responseMode?: string | null;
            /** Format: uuid */
            sessionId?: string | null;
        };
        /** InvocationResponseV1 */
        InvocationResponse: {
            /** Format: uint64 */
            admissionEpoch: number;
            /** Format: uuid */
            applicationId: string;
            /** Format: uuid */
            bundleId: string;
            conversationId?: string | null;
            createdAt: string;
            endUser?: components["schemas"]["InvocationResponse"]["$defs"]["ApplicationUserIdentityV1"] | null;
            error?: unknown;
            /** Format: uuid */
            executionId?: string | null;
            /** Format: uuid */
            id: string;
            outputs?: unknown;
            provider?: components["schemas"]["InvocationResponse"]["$defs"]["WebhookProviderV1"] | null;
            providerEventId?: string | null;
            /** Format: uuid */
            sessionId?: string | null;
            status: string;
            triggerContext?: unknown;
            userIdentityDiagnostic?: string | null;
            $defs: {
                /** @description The end user a call acts for. API identities are declarations, not authentication. */
                ApplicationUserIdentityV1: {
                    externalUserId: string;
                    source: components["schemas"]["InvocationResponse"]["$defs"]["ApplicationUserSourceV1"];
                    sourceId: string;
                };
                /** @enum {string} */
                ApplicationUserSourceV1: "platform" | "channel" | "api_parameter";
                /** @enum {string} */
                WebhookProviderV1: "agentx" | "dingtalk" | "wecom" | "feishu";
            };
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
            parts: components["schemas"]["MessageRequest"]["$defs"]["MessagePartInputV1"][];
            $defs: {
                MessagePartInputV1: {
                    /** Format: uuid */
                    artifactId?: string | null;
                    content?: unknown;
                    partType: string;
                };
            };
        };
        /** MessageResponseV1 */
        MessageResponse: {
            createdAt: string;
            /** Format: uuid */
            id: string;
            /** Format: uuid */
            invocationId?: string | null;
            parts: components["schemas"]["MessageResponse"]["$defs"]["MessagePartInputV1"][];
            role: string;
            /** Format: uint64 */
            sequence: number;
            $defs: {
                MessagePartInputV1: {
                    /** Format: uuid */
                    artifactId?: string | null;
                    content?: unknown;
                    partType: string;
                };
            };
        };
        /** SessionResponseV1 */
        SessionResponse: {
            /** Format: uuid */
            applicationDeploymentId: string;
            /** Format: uuid */
            applicationId: string;
            /** Format: uuid */
            bundleId?: string | null;
            endUser?: components["schemas"]["SessionResponse"]["$defs"]["ApplicationUserIdentityV1"] | null;
            externalUserId?: string | null;
            /** Format: uuid */
            id: string;
            status: string;
            title?: string | null;
            updatedAt: string;
            /** Format: uint64 */
            version: number;
            versionPolicy: components["schemas"]["SessionResponse"]["$defs"]["SessionVersionPolicyV1"];
            /** Format: uuid */
            workflowVersionId?: string | null;
            $defs: {
                /** @description The end user a call acts for. API identities are declarations, not authentication. */
                ApplicationUserIdentityV1: {
                    externalUserId: string;
                    source: components["schemas"]["SessionResponse"]["$defs"]["ApplicationUserSourceV1"];
                    sourceId: string;
                };
                /** @enum {string} */
                ApplicationUserSourceV1: "platform" | "channel" | "api_parameter";
                /** @enum {string} */
                SessionVersionPolicyV1: "pinned" | "follow_deployment" | "manual_upgrade";
            };
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
