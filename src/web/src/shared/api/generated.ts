/** Generated schema components from contracts/openapi/platform-api.json. Do not edit. */
export interface components {
    schemas: {
        AgentIterationDetail: {
            agentRunId: string;
            endedAt?: string | null;
            id: string;
            iterationIndex: number;
            startedAt: string;
            stateAfterHash?: string | null;
            stateArtifactId?: string | null;
            stateBeforeHash: string;
            status: string;
            stopReason?: string | null;
        };
        AgentRunDetail: {
            budget: unknown;
            costMicros: number;
            endedAt?: string | null;
            id: string;
            inputTokens: number;
            iterationCount: number;
            modelCallCount: number;
            nodeExecutionId: string;
            outputTokens: number;
            startedAt: string;
            stateArtifactId?: string | null;
            stateHash?: string | null;
            status: string;
            stopReason?: string | null;
            toolCallCount: number;
        };
        ApiErrorResponse: {
            code: string;
            details?: unknown;
            fieldErrors?: components["schemas"]["FieldError"][];
            message: string;
            requestId: string;
        };
        ApiKeyResponse: {
            createdAt: string;
            familyId: string;
            id: string;
            lastUsedAt?: string | null;
            name: string;
            prefix: string;
            secret?: string | null;
            status: string;
        };
        ApplicationDeploymentResponse: {
            applicationId: string;
            createdAt: string;
            environmentId: string;
            environmentName: string;
            id: string;
            inputSchema: unknown;
            outputSchema: unknown;
            publishAttemptId?: string | null;
            publishErrorCode?: string | null;
            publishErrorMessage?: string | null;
            sequenceNumber: number;
            sessionVersionPolicy: string;
            status: string;
            workflowVersionId: string;
            workflowVersionNumber: number;
        };
        ApplicationResponse: {
            activeDeploymentId?: string | null;
            activeVersionNumber?: number | null;
            description?: string | null;
            id: string;
            name: string;
            ownerDepartmentId: string;
            publishedRuntimeConfigRevision: number;
            runtimeConfigRevision: number;
            slug: string;
            status: string;
            updatedAt: string;
            version: number;
            visibility: string;
            workflowId: string;
            workflowName: string;
        };
        ApprovalActionResponse: {
            actionType: string;
            actorName: string;
            actorUserId: string;
            createdAt: string;
            fromStatus: string;
            id: string;
            input?: unknown;
            toStatus: string;
        };
        ApprovalCandidateResponse: {
            displayName: string;
            userId: string;
        };
        ApprovalResponse: {
            buttons: {
                id: string;
                label: string;
            }[];
            claimedBy?: string | null;
            claimedByName?: string | null;
            createdAt: string;
            deadlineAt?: string | null;
            decision?: Record<string, never> | null;
            description?: string | null;
            executionId: string;
            id: string;
            nodeId: string;
            requestPayload?: unknown;
            resumeStatus: string;
            status: string;
            title: string;
            version: number;
            workflowId: string;
            workflowName: string;
        };
        ArtifactUploadResponse: {
            entry: components["schemas"]["SkillWorkspaceEntry"];
            revision: number;
        };
        AuthResponse: {
            accessToken?: string | null;
            changePasswordToken?: string | null;
            expiresIn?: number | null;
            passwordChangeRequired: boolean;
            user?: null | components["schemas"]["MeResponse"];
        };
        BootstrapRequest: {
            adminDisplayName: string;
            adminUsername: string;
            companyName: string;
            locale: string;
            password: string;
            timezone: string;
        };
        BootstrapStatus: {
            required: boolean;
        };
        CancelResourceGrantRequest: {
            expectedVersion: number;
        };
        CaseInput: {
            caseKey: string;
            context?: unknown;
            evaluatorOverride?: unknown;
            expectedOutput?: unknown;
            input: unknown;
            name: string;
            tags?: string[];
        };
        ChangePasswordRequest: {
            password: string;
            token: string;
        };
        ChatMappingV1: {
            answerFilesOutput: string | null;
            answerOutput: string;
            fileInput: string | null;
            questionInput: string;
        };
        CheckpointListResponse: {
            items: components["schemas"]["CheckpointResponse"][];
        };
        CheckpointResponse: {
            activationCount: number;
            checkpointType: string;
            createdAt: string;
            deliveryCount: number;
            executionId: string;
            id: string;
            nodeExecutionId?: string | null;
            sequenceNumber: number;
            stateHash: string;
        };
        ConnectionResponse: {
            configuration: unknown;
            credentialId?: string | null;
            endpoint: string;
            healthPath: string;
            id: string;
            name: string;
            ownerDepartmentId: string;
            provider?: string | null;
            status: string;
            version: number;
        };
        CreateApiKeyRequest: {
            name: string;
        };
        CreateApplicationDeploymentRequest: {
            environmentId: string;
            sessionVersionPolicy: string;
            workflowVersionId: string;
        };
        CreateApplicationRequest: {
            description?: string | null;
            name: string;
            slug: string;
            visibility: string;
            workflowId: string;
        };
        CreateCaseRequest: components["schemas"]["CaseInput"] & {
            expectedRevision: number;
        };
        CreateConnectionRequest: {
            configuration: unknown;
            credentialId?: string | null;
            endpoint: string;
            healthPath?: string | null;
            name: string;
            ownerDepartmentId: string;
            /** @enum {string} */
            provider?: "lightrag" | "ragflow";
        };
        CreateCredentialRequest: {
            credentialType: string;
            name: string;
            ownerDepartmentId: string;
            secret: unknown;
        };
        CreateDatasetRequest: {
            description?: string | null;
            name: string;
            visibility: string;
        };
        CreateDepartmentRequest: {
            name: string;
            parentId: string;
        };
        CreateEntryRequest: {
            entryType: string;
            expectedRevision: number;
            name: string;
            parentId?: string | null;
        };
        CreateEnvironmentRequest: {
            code: string;
            name: string;
        };
        CreateEvaluationProfileRequest: {
            aggregation?: string;
            description?: string | null;
            name: string;
            passThreshold?: string;
            rules: components["schemas"]["EvaluationRuleInput"][];
            visibility?: string;
        };
        CreateEvaluationRunRequest: {
            datasetVersionId: string;
            evaluationProfileVersionId: string;
            name: string;
            parameters: unknown;
            visibility?: string;
            workflowVersionId: string;
        };
        CreateGrantRequest: {
            operation: string;
            resourceVersionId?: string | null;
            subjectId: string;
            subjectType: string;
        };
        CreateKnowledgeRequest: {
            connectionId: string;
            externalResourceId: string;
            name: string;
            ownerDepartmentId: string;
        };
        CreateMcpServerRequest: {
            configuration?: unknown;
            description?: string | null;
            name: string;
            ownerDepartmentId: string;
            transport: components["schemas"]["McpTransportInput"];
        };
        CreateMemoryRequest: {
            accessMode: string;
            connectionId: string;
            externalNamespace: string;
            name: string;
            ownerDepartmentId: string;
        };
        CreateModelPriceRequest: {
            currency: string;
            inputPerMillion: string;
            outputPerMillion: string;
        };
        CreateModelRequest: {
            alias?: string;
            connectionName: string;
            credentialId?: string | null;
            defaultParameters?: unknown;
            /** @description Input modality capabilities */
            capabilities?: ("vision" | "audio")[];
            endpoint: string;
            maxInputTokens?: number;
            maxOutputTokens?: number;
            modelName?: string;
            ownerDepartmentId: string;
            price: components["schemas"]["CreateModelPriceRequest"];
            providerType: string;
        };
        CreateResourceGrantRequest: {
            message?: string | null;
            operation: string;
            resourceId: string;
            resourceType: string;
            resourceVersionId?: string | null;
            sourceNodeId?: string | null;
            sourceRevision?: number | null;
        };
        CreateRetentionRunRequest: {
            artifactRetentionDays?: number;
            dryRun?: boolean;
            evaluationRetentionDays?: number;
            messageRetentionDays?: number;
            traceRetentionDays?: number;
        };
        CreateRoleRequest: {
            code: string;
            dataScope: string;
            description?: string | null;
            name: string;
            permissions: string[];
        };
        CreateSandboxProfileRequest: components["schemas"]["SandboxProfileVersionInput"] & {
            description?: string | null;
            name: string;
            ownerDepartmentId: string;
        };
        CreateScheduleRequest: {
            cronExpression: string;
            input: unknown;
            misfirePolicy?: string;
            name: string;
            timezone: string;
        };
        CreateSkillRequest: {
            alias: string;
            description: string;
            name: string;
            ownerDepartmentId: string;
        };
        CreateUserRequest: {
            departmentId: string;
            displayName: string;
            roleId: string;
            username: string;
        };
        CreateVersionRequest: {
            draftRevision: number;
        };
        CreateWebhookRequest: {
            channelConfig?: {
                [key: string]: unknown;
            } | null;
            channelMode?: string;
            fixedInputs?: {
                [key: string]: unknown;
            };
            inputMappings?: {
                missingPolicy?: string;
                source: string;
                target: string;
            }[];
            name: string;
            /** @enum {string} */
            providerType: "dingtalk" | "wecom" | "feishu" | "agentx";
            reply?: components["schemas"]["WebhookReplyConfig"];
        };
        CreateWorkflowRequest: {
            description?: string | null;
            name: string;
            visibility: string;
        };
        CredentialResponse: {
            credentialType: string;
            currentSecretVersion: number;
            id: string;
            maskedHint: string;
            name: string;
            ownerDepartmentId: string;
            status: string;
            storageMode: string;
            updatedAt: string;
            version: number;
        };
        DashboardSummaryResponse: {
            costMicrosToday: number;
            executionsToday: number;
            failedToday: number;
            pendingApprovals: number;
            runningExecutions: number;
            succeededToday: number;
            workflowCount: number;
        };
        DatasetResponse: {
            caseCount: number;
            description?: string | null;
            id: string;
            latestVersion?: number | null;
            name: string;
            ownerDepartmentId: string;
            revision: number;
            status: string;
            updatedAt: string;
            version: number;
            visibility: string;
        };
        DatasetVersionResponse: {
            caseCount: number;
            contentHash: string;
            createdAt: string;
            datasetId: string;
            id: string;
            sourceRevision: number;
            versionNumber: number;
        };
        DebugExecutionRequest: {
            context?: unknown;
            expectedRevision: number;
            idempotencyKey?: string | null;
            input?: unknown;
            inputSource?: unknown;
            mode: string;
            overlayIds?: string[];
            sideEffectDecisions?: unknown;
            targetNodeId?: string | null;
        };
        DebugMcpToolRequest: {
            arguments: unknown;
            confirmationText?: string | null;
            confirmed: boolean;
            expectedToolVersionId: string;
        };
        DebugMcpToolResponse: {
            durationMs: number;
            result: unknown;
            toolVersionId: string;
        };
        DebugOverlayResponse: {
            artifactId?: string | null;
            id: string;
            kind: string;
            nodeId: string;
            payload: unknown;
            schemaHash?: string | null;
            stale: boolean;
            updatedAt: string;
            updatedBy: string;
            workflowId: string;
        };
        DecideApprovalRequest: {
            decisionId: string;
            idempotencyKey: string;
            reason?: string | null;
            version: number;
        };
        DeleteCaseRequest: {
            expectedRevision: number;
        };
        DeletionImpactResponse: {
            deletable: boolean;
            page: number;
            pageSize: number;
            references: components["schemas"]["DeletionReference"][];
            targetVersion: number;
            total: number;
        };
        DeletionReference: {
            immutable: boolean;
            nodeId?: string | null;
            nodeName?: string | null;
            parentId?: string | null;
            relation: string;
            sourceId: string;
            sourceModule: string;
            sourceName: string;
            sourceType: string;
        };
        DepartmentResponse: {
            id: string;
            isRoot: boolean;
            name: string;
            parentId?: string | null;
            status: string;
            version: number;
        };
        DependencyHealth: {
            name: string;
            required: boolean;
            status: string;
        };
        DeploymentResponse: {
            createdAt: string;
            environmentId: string;
            environmentName: string;
            id: string;
            sequenceNumber: number;
            source: string;
            status: string;
            versionNumber: number;
            workflowId: string;
            workflowVersionId: string;
        };
        DraftResponse: {
            definition: unknown;
            definitionHash: string;
            editorDocument: unknown;
            editorHash: string;
            id: string;
            revision: number;
            schemaVersion: string;
            updatedAt: string;
            workflowId: string;
        };
        EnvironmentResponse: {
            code: string;
            id: string;
            isBuiltin: boolean;
            name: string;
            status: string;
            version: number;
        };
        EvaluationCaseResultResponse: {
            caseId: string;
            caseKey: string;
            costMicros: number;
            detail?: unknown;
            durationMs?: number | null;
            errorCode?: string | null;
            errorMessage?: string | null;
            ruleResults: components["schemas"]["EvaluationRuleResultResponse"][];
            score?: number | null;
            sourceCaseId: string;
            status: string;
            targetExecutionId?: string | null;
        };
        EvaluationProfileResponse: {
            aggregation: string;
            description?: string | null;
            id: string;
            name: string;
            ownerDepartmentId: string;
            passThreshold: string;
            rules: components["schemas"]["EvaluationRuleResponse"][];
            status: string;
            version: number;
            versionId: string;
            versionNumber: number;
            visibility: string;
        };
        EvaluationReportResponse: {
            metrics: unknown[];
            reportStatus: string;
            results: components["schemas"]["EvaluationCaseResultResponse"][];
            run: components["schemas"]["EvaluationRunResponse"];
        };
        EvaluationRuleInput: {
            configuration: unknown;
            evaluatorType: string;
            key: string;
            name: string;
            required?: boolean;
            weight?: string;
        };
        EvaluationRuleResponse: {
            configuration: unknown;
            evaluatorType: string;
            id: string;
            key: string;
            name: string;
            required: boolean;
            sortOrder: number;
            weight: string;
        };
        EvaluationRuleResultResponse: {
            costMicros: number;
            detail: unknown;
            durationMs?: number | null;
            evaluatorExecutionId?: string | null;
            evaluatorType: string;
            id: string;
            key: string;
            name: string;
            passed?: boolean | null;
            score?: number | null;
            status: string;
        };
        EvaluationRunResponse: {
            createdAt: string;
            datasetName: string;
            datasetVersionId: string;
            evaluationProfileVersionId: string;
            id: string;
            name: string;
            ownerDepartmentId: string;
            parameters: unknown;
            resultCount: number;
            status: string;
            visibility: string;
            workflowName: string;
            workflowVersionId: string;
        };
        ExecutionCommandResponse: {
            executionId: string;
            replayed: boolean;
            status: string;
        };
        ExecutionEventListResponse: {
            items: components["schemas"]["ExecutionEventResponse"][];
            nextCursor?: number | null;
        };
        ExecutionEventQuery: {
            after?: number | null;
            limit?: number | null;
        };
        ExecutionEventResponse: {
            eventType: string;
            occurredAt: string;
            sequence: number;
            status: string;
            summary: unknown;
        };
        ExecutionListResponse: {
            items: components["schemas"]["ExecutionResponse"][];
            limit: number;
            nextCursor?: string | null;
            snapshotId: string;
            total: number;
        };
        ExecutionResponse: {
            admissionEpoch?: number;
            applicationId?: string | null;
            applicationName?: string | null;
            callerExecutionId?: string | null;
            costCurrency?: string | null;
            costMicros: number;
            durationMs?: number | null;
            endedAt?: string | null;
            error?: unknown;
            errorCode?: string | null;
            errorMessage?: string | null;
            executionType: string;
            forkCheckpointId?: string | null;
            id: string;
            initiatorDepartmentId?: string | null;
            initiatorDepartmentName?: string | null;
            initiatorUserId?: string | null;
            initiatorUserName?: string | null;
            input?: unknown;
            inputTokens: number;
            invocationId?: string | null;
            output?: unknown;
            outputTokens: number;
            parentExecutionId?: string | null;
            sessionId?: string | null;
            startedAt: string;
            stateVersion?: number;
            status: string;
            traceId: string;
            traceWatermark?: number;
            triggerName?: string | null;
            triggerSourceId?: string | null;
            triggerType: string;
            workflowId: string;
            workflowName: string;
            workflowVersionId?: string | null;
            workflowVersionNumber?: number | null;
        };
        FieldError: {
            code: string;
            field: string;
            message: string;
        };
        ForkRequest: {
            checkpointId: string;
            idempotencyKey?: string | null;
            inputOverrides?: unknown;
            mode: string;
            nodeId?: string | null;
            sideEffectDecisions?: unknown;
        };
        GrantResponse: {
            createdAt: string;
            id: string;
            operation: string;
            resourceId: string;
            resourceType: string;
            resourceVersionId?: string | null;
            subjectId: string;
            subjectType: string;
        };
        GrantableResourceResponse: {
            detail: string;
            grantCount: number;
            id: string;
            name: string;
            ownerDepartmentId: string;
            resourceType: string;
            status: string;
            updatedAt: string;
        };
        HealthCheckResponse: {
            checkedAt: string;
            errorCode?: string | null;
            errorMessage?: string | null;
            latencyMs?: number | null;
            status: string;
        };
        HealthResponse: {
            dependencies?: components["schemas"]["DependencyHealth"][];
            service: string;
            status: string;
            version: string;
        };
        IdempotentCommandResponse: {
            accepted: boolean;
            replayed: boolean;
        };
        ImportCasesRequest: {
            content: string;
            expectedRevision: number;
            format: string;
        };
        ImportWorkflowPackageRequest: {
            description?: string | null;
            name: string;
            package: components["schemas"]["WorkflowPackage"];
            visibility: string;
        };
        ImportedWorkflowResponse: {
            definitionHash: string;
            draftId: string;
            workflowId: string;
        };
        KnowledgeResponse: {
            connectionId: string;
            connectionName: string;
            externalResourceId: string;
            grantCount: number;
            id: string;
            name: string;
            ownerDepartmentId: string;
            status: string;
            syncStatus: string;
            updatedAt: string;
            version: number;
        };
        LineageResponse: {
            deliveryId: string;
            sourceItemIndex: number;
            sourceNodeExecutionId: string;
            sourceOutputIndex: number;
            sourceRunIndex: number;
            targetItemIndex: number;
        };
        LoginRequest: {
            password: string;
            username: string;
        };
        McpDiscoveryResponse: {
            discoveredCount: number;
            runId: string;
            tools: components["schemas"]["McpToolResponse"][];
        };
        McpEnvironmentCredentialInput: {
            credentialId: string;
            name: string;
        };
        McpRuntimeSandboxInput: {
            resourceId: string;
            resourceVersionId: string;
        };
        McpServerResponse: {
            currentVersionNumber: number;
            description?: string | null;
            id: string;
            lastDiscoveredAt?: string | null;
            name: string;
            ownerDepartmentId: string;
            status: string;
            toolCount: number;
            transport: components["schemas"]["McpTransportInput"];
            updatedAt: string;
            version: number;
        };
        McpToolResponse: {
            annotations: unknown;
            availability: string;
            currentVersionId: string;
            currentVersionNumber: number;
            debugEnabled: boolean;
            description?: string | null;
            enabled: boolean;
            id: string;
            inputSchema: unknown;
            name: string;
            outputSchema?: unknown;
            schemaHash: string;
            serverId: string;
            sideEffect: string;
            timeoutSeconds: number;
            title?: string | null;
            version: number;
        };
        McpTransportInput: {
            bearerCredentialId?: string | null;
            endpoint: string;
            /** @constant */
            kind: "streamable_http";
        } | {
            bearerCredentialId?: string | null;
            endpoint: string;
            /** @constant */
            kind: "sse";
        } | {
            args: string[];
            command: string;
            environmentCredentialRefs: components["schemas"]["McpEnvironmentCredentialInput"][];
            /** @constant */
            kind: "stdio";
            runtimeSandbox: components["schemas"]["McpRuntimeSandboxInput"];
        };
        MeResponse: {
            companyId: string;
            companyName: string;
            departmentId: string;
            departmentName: string;
            displayName: string;
            id: string;
            locale: string;
            permissions: string[];
            roles: string[];
            timezone: string;
            username: string;
        };
        MemoryResponse: {
            accessMode: string;
            connectionId: string;
            connectionName: string;
            externalNamespace: string;
            grantCount: number;
            id: string;
            name: string;
            ownerDepartmentId: string;
            status: string;
            updatedAt: string;
            version: number;
        };
        MissingGrantResponse: {
            nodeId: string;
            operation: string;
            reason: string;
            requiredByResourceId?: string | null;
            resourceId: string;
            resourceType: string;
        };
        ModelDeploymentHistoryResponse: {
            changedAt: string;
            changedBy: string;
            connectionName: string;
            deploymentId: string;
            id: string;
            modelName: string;
            previousDeploymentId?: string | null;
            revisionNumber: number;
        };
        ModelPriceResponse: {
            createdAt: string;
            currency: string;
            deploymentId: string;
            id: string;
            inputPerMillion: string;
            outputPerMillion: string;
            versionNumber: number;
        };
        ModelResponse: {
            alias: string;
            aliasVersion: number;
            connectionCheckedAt?: string | null;
            connectionName: string;
            connectionStatus: string;
            credentialId?: string | null;
            defaultParameters: unknown;
            /** @description Input modality capabilities */
            capabilities?: ("vision" | "audio")[];
            deploymentId: string;
            endpoint: string;
            id: string;
            maxInputTokens: number;
            maxOutputTokens: number;
            modelName: string;
            ownerDepartmentId: string;
            providerType: string;
            revisionNumber: number;
            status: string;
            updatedAt: string;
        };
        MoveEntryRequest: {
            expectedRevision: number;
            name: string;
            parentId?: string | null;
        };
        NodeAttemptResponse: {
            attemptNumber: number;
            deadlineAt?: string | null;
            endedAt?: string | null;
            errorCode?: string | null;
            errorMessage?: string | null;
            id: string;
            startedAt?: string | null;
            status: string;
            workerInstanceId?: string | null;
        };
        NodeDefinitionDetail: {
            manifest: unknown;
            manifestHash: string;
            nodeType: string;
            version: number;
        };
        NodeDefinitionQuery: {
            category?: string | null;
            page?: number | null;
            pageSize?: number | null;
            search?: string | null;
        };
        NodeDefinitionSummary: {
            capability: string;
            category: string;
            description: string;
            displayName: string;
            executionStyle: string;
            iconKey: string;
            keywords: string[];
            manifestHash: string;
            nodeType: string;
            version: number;
        };
        NodeExecutionListResponse: {
            items: components["schemas"]["NodeExecutionResponse"][];
        };
        NodeExecutionResponse: {
            activationSlot: number;
            attempts: components["schemas"]["NodeAttemptResponse"][];
            capability: string;
            costCurrency?: string | null;
            costMicros: number;
            endedAt?: string | null;
            errorCode?: string | null;
            errorMessage?: string | null;
            executionId: string;
            generation: number;
            id: string;
            input?: unknown;
            iterationIndex: number;
            lineage: components["schemas"]["LineageResponse"][];
            nodeId: string;
            nodeName: string;
            nodeType: string;
            nodeVersion: number;
            output?: unknown;
            runIndex: number;
            sideEffectLevel: string;
            startedAt?: string | null;
            status: string;
        };
        NodeLock: {
            manifestHash: string;
            nodeType: string;
            version: number;
        };
        NodeProviderOption: {
            description?: string | null;
            label: string;
            manifest?: unknown;
            value: string;
        };
        NodeProviderOptionsResponse: {
            items: components["schemas"]["NodeProviderOption"][];
        };
        NodeProviderQuery: {
            limit?: number | null;
            search?: string | null;
        };
        NotificationInboxResponse: {
            items: components["schemas"]["NotificationResponse"][];
            unreadCount: number;
        };
        NotificationResponse: {
            arguments: unknown;
            bodyKey: string;
            createdAt: string;
            id: string;
            notificationType: string;
            read: boolean;
            targetId: string;
            targetPath: string;
            targetType: string;
            titleKey: string;
            tone: string;
        };
        PermissionResponse: {
            description?: string | null;
            key: string;
            name: string;
        };
        PlaygroundConfigResponse: {
            applicationId: string;
            deploymentId: string;
            deploymentVersion: number;
            errorCode: string | null;
            errorMessage: string | null;
            mapping: components["schemas"]["ChatMappingV1"] | null;
            /** @enum {string} */
            publishStatus: "active" | "publishing" | "failed";
            publishedVersion: number | null;
            version: number;
        };
        PublishAttemptResponse: {
            activationSequence: number;
            bundleId?: string | null;
            deploymentId: string;
            errorCode?: string | null;
            errorMessage?: string | null;
            id: string;
            nextAction: string;
            previousAttemptId?: string | null;
            state: string;
            updatedAt: string;
        };
        PublishSkillVersionRequest: {
            dependencies?: components["schemas"]["SkillDependencyInput"][];
            expectedRevision: number;
        };
        PublishWorkflowRequest: {
            environmentId: string;
            workflowVersionId: string;
        };
        PutPlaygroundConfigRequest: {
            expectedVersion: number;
            mapping: components["schemas"]["ChatMappingV1"] | null;
        };
        QueuedWorkflowRunResponse: {
            commandId: string;
            status: string;
        };
        QuotaPolicyInput: {
            dimension: string;
            hardLimit: string;
            periodSeconds?: number | null;
        };
        QuotaPolicyResponse: {
            activeReserved: string;
            dimension: string;
            hardLimit: string;
            periodSeconds?: number | null;
            periodUsage: string;
            version: number;
        };
        ReassignApprovalRequest: {
            targetUserId: string;
            version: number;
        };
        ResourceAuthorizationInput: {
            operation: string;
            resourceId: string;
            resourceType: string;
            resourceVersionId?: string | null;
        };
        ResourceAuthorizationResponse: {
            alreadyGrantedCount: number;
            grantedCount: number;
            resourceId: string;
            resourceType: string;
            workflowId: string;
        };
        ResourceGrantRequestAuditResponse: {
            action: string;
            actorName?: string | null;
            id: string;
            occurredAt: string;
        };
        ResourceGrantRequestResponse: {
            createdAt: string;
            history: components["schemas"]["ResourceGrantRequestAuditResponse"][];
            id: string;
            items: components["schemas"]["ResourceRequirementResponse"][];
            message?: string | null;
            operation: string;
            primaryResourceId: string;
            primaryResourceName?: string | null;
            primaryResourceType: string;
            requestedBy: string;
            requestedByName: string;
            reviews: components["schemas"]["ResourceGrantRequestReviewResponse"][];
            sourceNodeId?: string | null;
            sourceRevision?: number | null;
            status: string;
            subjectDepartmentName?: string | null;
            subjectId: string;
            /** @enum {string} */
            subjectType: "department" | "workflow_service_identity";
            updatedAt: string;
            version: number;
            workflowId?: string | null;
            workflowName?: string | null;
            workflowServiceIdentityId?: string | null;
        };
        ResourceGrantRequestReviewResponse: {
            canAct: boolean;
            id: string;
            ownerDepartmentId: string;
            ownerDepartmentName: string;
            reviewComment?: string | null;
            reviewedAt?: string | null;
            reviewedBy?: string | null;
            reviewedByName?: string | null;
            status: string;
            version: number;
        };
        ResourceOptionQuery: {
            operation?: string | null;
            page?: number | null;
            pageSize?: number | null;
            resourceType: string;
            search?: string | null;
        };
        ResourceRequirementResponse: {
            active: boolean;
            authorized: boolean;
            name?: string | null;
            operation: string;
            ownerDepartmentId?: string | null;
            requiredByResourceId?: string | null;
            resourceId: string;
            resourceType: string;
            resourceVersionId?: string | null;
        };
        ResourceValidationResponse: {
            missingGrants: components["schemas"]["MissingGrantResponse"][];
            valid: boolean;
        };
        RetentionItemResponse: {
            attemptCount: number;
            dataType: string;
            id: string;
            reason?: string | null;
            status: string;
            targetId: string;
            updatedAt: string;
        };
        RetentionRunResponse: {
            candidateCount: number;
            completedAt?: string | null;
            createdAt: string;
            deletedCount: number;
            dryRun: boolean;
            errorMessage?: string | null;
            id: string;
            status: string;
        };
        ReviewResourceGrantRequest: {
            comment?: string | null;
            expectedVersion: number;
        };
        RevisionResponse: {
            createdAt: string;
            createdBy: string;
            definitionHash: string;
            editorHash: string;
            id: string;
            revision: number;
            schemaVersion: string;
        };
        RoleResponse: {
            code: string;
            dataScope: string;
            description?: string | null;
            id: string;
            isBuiltin: boolean;
            memberCount: number;
            name: string;
            permissions: string[];
            status: string;
            version: number;
        };
        RollbackWorkflowRequest: {
            targetWorkflowVersionId: string;
        };
        RotateCredentialRequest: {
            secret: unknown;
            version: number;
        };
        RunWorkflowRequest: {
            idempotencyKey?: string | null;
            input?: unknown;
        };
        RuntimeCallDetail: {
            agentRunId?: string | null;
            attemptId: string;
            callIndex: number;
            callKind: string;
            costCurrency?: string | null;
            costMicros: number;
            endedAt?: string | null;
            errorCode?: string | null;
            errorMessage?: string | null;
            id: string;
            inputTokens: number;
            iterationIndex: number;
            outputTokens: number;
            requestFingerprint: string;
            resourceId?: string | null;
            resourceType?: string | null;
            resourceVersionId?: string | null;
            responseArtifactId?: string | null;
            sideEffect: string;
            startedAt: string;
            status: string;
            usageEstimated: boolean;
        };
        RuntimeComponentStatus: {
            component: string;
            instances?: number | null;
            lastHeartbeat?: string | null;
            queueDepth?: number | null;
            status: string;
        };
        RuntimeDetailsResponse: {
            agentRuns: components["schemas"]["AgentRunDetail"][];
            calls: components["schemas"]["RuntimeCallDetail"][];
            costCurrency?: string | null;
            costMicros: number;
            executionId: string;
            inputTokens: number;
            iterations: components["schemas"]["AgentIterationDetail"][];
            outputTokens: number;
            sandboxes: components["schemas"]["SandboxLeaseDetail"][];
        };
        RuntimeStatusResponse: {
            activeSandboxes: number;
            components: components["schemas"]["RuntimeComponentStatus"][];
            failedToday: number;
            running: number;
            sandboxCompatibility?: unknown;
            waiting: number;
        };
        SandboxLeaseDetail: {
            attemptId: string;
            createdAt: string;
            expiresAt: string;
            heartbeatAt: string;
            id: string;
            lastError?: string | null;
            nodeExecutionId: string;
            profileVersionId: string;
            sandboxId?: string | null;
            status: string;
            terminatedAt?: string | null;
            terminationAttempts: number;
        };
        SandboxProfileResponse: {
            current: components["schemas"]["SandboxProfileVersionResponse"];
            currentVersionNumber: number;
            description?: string | null;
            id: string;
            name: string;
            ownerDepartmentId: string;
            status: string;
            updatedAt: string;
            version: number;
            versions: components["schemas"]["SandboxProfileVersionResponse"][];
        };
        SandboxProfileVersionInput: {
            cpuMillis: number;
            diskBytes: number;
            imageDigest: string;
            memoryBytes: number;
            networkPolicy: unknown;
            outputLimitBytes: number;
            pidsLimit: number;
            runner: string;
            timeoutSeconds: number;
        };
        SandboxProfileVersionResponse: {
            configurationHash: string;
            cpuMillis: number;
            createdAt: string;
            diskBytes: number;
            id: string;
            imageDigest: string;
            memoryBytes: number;
            networkPolicy: unknown;
            outputLimitBytes: number;
            pidsLimit: number;
            runner: string;
            timeoutSeconds: number;
            versionNumber: number;
        };
        SaveDebugOverlayRequest: {
            artifactId?: string | null;
            kind: string;
            payload: unknown;
            schemaHash?: string | null;
        };
        SaveDraftRequest: {
            definition: unknown;
            editorDocument?: unknown;
            expectedRevision: number;
        };
        ScheduleResponse: {
            cronExpression: string;
            id: string;
            input: unknown;
            misfirePolicy: string;
            name: string;
            status: string;
            timezone: string;
            version: number;
        };
        SessionResponse: {
            applicationDeploymentId: string;
            applicationId: string;
            externalUserId?: string | null;
            id: string;
            status: string;
            title?: string | null;
            updatedAt: string;
            version: number;
            versionPolicy: string;
            workflowVersionId?: string | null;
        };
        SideEffectConfirmationRequest: {
            checkpointId?: string | null;
            decision: string;
            idempotencyKey: string;
            nodeExecutionId: string;
        };
        SkillDependencyInput: {
            operation: string;
            resourceId: string;
            resourceType: string;
            resourceVersionId?: string | null;
        };
        SkillResponse: {
            alias: string;
            description?: string | null;
            draftRevision: number;
            grantCount: number;
            id: string;
            latestVersion?: number | null;
            name: string;
            ownerDepartmentId: string;
            status: string;
            updatedAt: string;
            version: number;
        };
        SkillVersionResponse: {
            contentHash: string;
            createdAt: string;
            dependencies: components["schemas"]["SkillDependencyInput"][];
            fileCount: number;
            id: string;
            manifest: unknown;
            skillId: string;
            sourceRevision: number;
            versionNumber: number;
        };
        SkillWorkspaceEntry: {
            artifactId?: string | null;
            contentHash?: string | null;
            editable: boolean;
            entryType: string;
            id: string;
            mimeType?: string | null;
            name: string;
            parentId?: string | null;
            path: string;
            sizeBytes: number;
            updatedAt: string;
        };
        SkillWorkspaceResponse: {
            entries: components["schemas"]["SkillWorkspaceEntry"][];
            revision: number;
        };
        StartExecutionRequest: {
            idempotencyKey?: string | null;
            input?: unknown;
        };
        TestCaseResponse: {
            caseKey: string;
            context?: unknown;
            evaluatorOverride?: unknown;
            expectedOutput?: unknown;
            id: string;
            input: unknown;
            name: string;
            sortOrder: number;
            tags: string[];
            version: number;
        };
        TraceContent: {
            contentRef: string | null;
            eventId: string;
            /** @enum {string} */
            kind: "workflow_input" | "workflow_output" | "node_input" | "node_output" | "attempt_input" | "attempt_output" | "resolved_parameters" | "runtime_request" | "runtime_response" | "agent_input" | "agent_output" | "iteration_input" | "iteration_output" | "sandbox_request" | "sandbox_response" | "wait_request" | "wait_response" | "conversion_record" | "plugin_content";
            occurredAt: string;
            preview: unknown;
        };
        TraceEventResponse: {
            attributes: unknown;
            /** @enum {unknown} */
            contentKind: "workflow_input" | "workflow_output" | "node_input" | "node_output" | "attempt_input" | "attempt_output" | "resolved_parameters" | "runtime_request" | "runtime_response" | "agent_input" | "agent_output" | "iteration_input" | "iteration_output" | "sandbox_request" | "sandbox_response" | "wait_request" | "wait_response" | "conversion_record" | "plugin_content" | null;
            contentPreview: unknown;
            contentRef: string | null;
            costMicros?: number;
            durationMs?: number | null;
            errorCode?: string | null;
            errorMessage?: string | null;
            eventId: string;
            /** @enum {string} */
            eventKind: "started" | "updated" | "finished";
            eventType: string;
            occurredAt: string;
            status: string;
        };
        TraceResponse: {
            complete: boolean;
            degraded: boolean;
            executionId: string;
            expectedWatermark: number;
            ingestedWatermark: number;
            nextCursor?: string | null;
            spans: components["schemas"]["TraceSpanSummary"][];
            totalSpans: number;
            traceId: string;
            warningCode?: string | null;
        };
        TraceSpanDetailResponse: {
            contents: components["schemas"]["TraceContent"][];
            events: components["schemas"]["TraceEventResponse"][];
            executionId: string;
            span: components["schemas"]["TraceSpanSummary"];
            traceId: string;
        };
        TraceSpanSummary: {
            agentIterationId?: string | null;
            agentRunId?: string | null;
            attemptId?: string | null;
            costMicros: number;
            durationMs: number | null;
            endedAt: string | null;
            errorCode?: string | null;
            errorMessage?: string | null;
            hasDetails: boolean;
            inputTokens?: number | null;
            nodeExecutionId?: string | null;
            outputTokens?: number | null;
            parentSpanId: string | null;
            resourceId?: string | null;
            resourceType?: string | null;
            resourceVersion?: string | null;
            runtimeCallId?: string | null;
            sandboxLeaseId?: string | null;
            spanId: string;
            /** @enum {string} */
            spanKind: "execution" | "boundary" | "node" | "attempt" | "agent_run" | "agent_iteration" | "runtime_call" | "sandbox" | "wait" | "plugin_operation";
            spanName: string;
            startedAt: string;
            status: string;
            waitId?: string | null;
        };
        UpdateApplicationRequest: {
            description?: string | null;
            name: string;
            status: string;
            version: number;
            visibility: string;
        };
        UpdateCaseRequest: components["schemas"]["CaseInput"] & {
            expectedRevision: number;
            version: number;
        };
        UpdateCredentialRequest: {
            name: string;
            status: string;
            version: number;
        };
        UpdateDatasetRequest: {
            description?: string | null;
            name: string;
            status: string;
            version: number;
            visibility: string;
        };
        UpdateDepartmentRequest: {
            name: string;
            parentId?: string | null;
            version: number;
        };
        UpdateEnvironmentRequest: {
            name: string;
            status: string;
            version: number;
        };
        UpdateExternalResourceRequest: {
            name: string;
            status: string;
            version: number;
        };
        UpdateMarkdownRequest: {
            content: string;
            description?: string | null;
            expectedRevision: number;
        };
        UpdateMcpServerRequest: {
            configuration?: unknown;
            description?: string | null;
            name: string;
            status: string;
            transport: components["schemas"]["McpTransportInput"];
            version: number;
        };
        UpdateMcpToolPolicyRequest: {
            debugEnabled: boolean;
            enabled: boolean;
            sideEffect: string;
            timeoutSeconds: number;
            version: number;
        };
        UpdateModelRequest: {
            alias: string;
            connectionName: string;
            credentialId?: string | null;
            defaultParameters: unknown;
            /** @description Input modality capabilities */
            capabilities?: ("vision" | "audio")[];
            endpoint: string;
            expectedAliasVersion: number;
            maxInputTokens: number;
            maxOutputTokens: number;
            modelName: string;
            ownerDepartmentId: string;
            price: components["schemas"]["CreateModelPriceRequest"];
            providerType: string;
            status: string;
        };
        UpdateQuotaPoliciesRequest: {
            policies: components["schemas"]["QuotaPolicyInput"][];
        };
        UpdateRoleRequest: {
            dataScope: string;
            description?: string | null;
            name: string;
            permissions: string[];
            version: number;
        };
        UpdateSandboxProfileRequest: {
            description?: string | null;
            name: string;
            status: string;
            version: number;
        };
        UpdateScheduleRequest: {
            cronExpression: string;
            input: unknown;
            misfirePolicy?: string;
            name: string;
            status: string;
            timezone: string;
            version: number;
        };
        UpdateSkillRequest: {
            alias: string;
            description?: string | null;
            name: string;
            status: string;
            version: number;
        };
        UpdateUserRequest: {
            departmentId: string;
            displayName: string;
            roleId: string;
            version: number;
        };
        UpdateWebhookRequest: {
            channelConfig?: {
                [key: string]: unknown;
            } | null;
            channelMode?: string;
            fixedInputs?: {
                [key: string]: unknown;
            };
            inputMappings?: {
                missingPolicy?: string;
                source: string;
                target: string;
            }[];
            name: string;
            /** @enum {string} */
            providerType: "dingtalk" | "wecom" | "feishu" | "agentx";
            status: string;
            version: number;
            reply?: components["schemas"]["WebhookReplyConfig"];
        };
        UpdateWorkflowRequest: {
            description?: string | null;
            name: string;
            version: number;
            visibility: string;
        };
        UpgradeSessionRequest: {
            version: number;
            workflowVersionId: string;
        };
        UpsertWorkflowMemberRequest: {
            memberRole: string;
            userId: string;
        };
        UserResponse: {
            departmentId: string;
            departmentName: string;
            displayName: string;
            id: string;
            passwordChangeRequired: boolean;
            roles: string[];
            status: string;
            username: string;
            version: number;
        };
        ValidateDraftRequest: {
            definition: unknown;
            editorDocument?: unknown;
        };
        ValidateDraftResponse: {
            compilerVersion?: string | null;
            definitionHash?: string | null;
            editorHash?: string | null;
            issues: components["schemas"]["ValidationIssue"][];
        };
        ValidationIssue: {
            code: string;
            fieldPath?: string | null;
            message: string;
            nodeId?: string | null;
            severity: string;
        };
        VersionActionRequest: {
            version: number;
        };
        WebhookProviderTemplate: {
            fields: {
                key: string;
                required: boolean;
                sensitive: boolean;
            }[];
            /** @description Raw payload paths (raw.<dotted.path>) this provider's messages carry beyond the standardized Trigger Context fields, offered as mapping source suggestions. */
            mappingSources?: string[];
            mode: string;
            provider: string;
        };
        WebhookResponse: {
            channelMode?: string;
            configFields?: {
                [key: string]: unknown;
            };
            configurationRevision: number;
            connectionError?: string | null;
            connectionStatus?: string | null;
            fixedInputs: {
                [key: string]: unknown;
            };
            id: string;
            inputMappings: {
                missingPolicy?: string;
                source: string;
                target: string;
            }[];
            lastConnectedAt?: string | null;
            mappingCount: number;
            name: string;
            path: string;
            providerType: string;
            publicId: string;
            secret?: string | null;
            status: string;
            version: number;
            reply?: components["schemas"]["WebhookReplyConfig"];
        };
        WorkerCapabilityResponse: {
            capability: string;
            compilerVersionMax: string;
            compilerVersionMin: string;
            heartbeatAt: string;
            instanceId: string;
            irSchemaVersions: unknown;
            manifestHashes: unknown;
            nodeProtocolVersion: string;
            status: string;
        };
        WorkflowMemberResponse: {
            displayName: string;
            memberRole: string;
            userId: string;
            username: string;
        };
        WorkflowPackage: {
            apiBinding: unknown;
            editorDocument: unknown;
            manifest: components["schemas"]["WorkflowPackageManifest"];
            nodeLock: components["schemas"]["NodeLock"][];
            subWorkflowReferences: string[];
            workflowDefinition: unknown;
        };
        WorkflowPackageManifest: {
            contentHash: string;
            name: string;
            packageSchemaVersion: string;
            signature: string;
            signatureAlgorithm: string;
            signingKeyId: string;
            workflowSchemaVersion: string;
        };
        WorkflowResourceOptionResponse: {
            accessState: string;
            detail: string;
            id: string;
            name: string;
            pendingRequestId?: string | null;
            requirements: components["schemas"]["ResourceRequirementResponse"][];
            resourceType: string;
            resourceVersionId?: string | null;
            status: string;
        };
        WorkflowResponse: {
            description?: string | null;
            draftRevision: number;
            id: string;
            latestVersion?: number | null;
            name: string;
            ownerDepartmentId: string;
            ownerName: string;
            ownerUserId: string;
            serviceIdentityId: string;
            status: string;
            updatedAt: string;
            version: number;
            visibility: string;
            canEdit: boolean;
        };
        WorkflowVersionResponse: {
            contentHash: string;
            createdAt: string;
            createdBy: string;
            definition: unknown;
            editorDocument: unknown;
            id: string;
            schemaVersion: string;
            sourceRevision: number;
            versionNumber: number;
            workflowId: string;
        };
        CanvasPluginVersion: {
            id: string;
            packageVersion: string;
            bundleDigest: string;
            /** @enum {string} */
            status: "enabled" | "disabled";
            sdkApiVersion: number;
            nodeTypes: string[];
            createdAt: string;
        };
        CanvasPlugin: {
            id: string;
            packageId: string;
            displayName: string;
            description: string;
            /** @enum {string} */
            sourceType: "builtin" | "imported";
            defaultVersionId: string | null;
            version: number;
            versions: components["schemas"]["CanvasPluginVersion"][];
            nodeCount: number;
            updatedAt: string;
        };
        CanvasPluginPage: {
            items: components["schemas"]["CanvasPlugin"][];
            page: number;
            pageSize: number;
            total: number;
        };
        CanvasPluginImport: {
            id: string;
            /** @enum {string} */
            status: "ready" | "installed" | "failed" | "cancelled";
            bundleDigest: string;
            packageId: string;
            packageVersion: string;
            displayName: string;
            description: string;
            nodeTypes: string[];
            issues: {
                [key: string]: unknown;
            }[];
            installedVersionId: string | null;
            expiresAt: string;
        };
        CanvasPluginReferences: {
            drafts: number;
            workflowVersions: number;
            deployments: number;
            executions: number;
            total: number;
        };
        CanvasPluginInstallRequest: {
            bundleDigest: string;
            enable: boolean;
            setDefault: boolean;
        };
        CanvasPluginUpdateRequest: {
            defaultVersionId: string;
            expectedRevision: number;
        };
        CanvasPluginVersionUpdateRequest: {
            enabled: boolean;
            expectedRevision: number;
        };
        CanvasPluginAuditItem: {
            id: string;
            actorUserId: string;
            action: string;
            detail: unknown;
            createdAt: string;
        };
        PluginResolvedDefinition: {
            /** @enum {string} */
            status: "complete" | "incomplete" | "invalid";
            inputPorts?: {
                name: string;
                /** @enum {string} */
                kind: "main" | "error";
                required?: boolean;
                variadic?: boolean;
            }[];
            outputPorts?: {
                name: string;
                /** @enum {string} */
                kind: "main" | "error";
                required?: boolean;
                variadic?: boolean;
            }[];
            outputSchema?: unknown;
            outputPortSchemas?: {
                [key: string]: unknown;
            };
            issues?: {
                path: string;
                code: string;
                message: string;
            }[];
        };
        PluginResolveDefinitionRequest: {
            configuration: {
                [key: string]: unknown;
            };
            upstreamContracts: {
                [key: string]: unknown;
            };
        };
        NodeProviderOptionsPage: {
            items: ({
                value: string;
                label: string;
                description?: string | null;
                manifest?: unknown;
            } & {
                [key: string]: unknown;
            })[];
            nextCursor?: string | null;
        };
        WebhookReplyConfig: {
            enabled: boolean;
            outputField: string;
            template?: string;
        };
        DeliverySummary: {
            id: string;
            applicationId?: string | null;
            invocationId?: string | null;
            executionId: string;
            channelBindingId: string;
            provider: string;
            origin: string;
            /** @enum {string} */
            status: "pending" | "delivering" | "delivered" | "failed" | "dead";
            attemptCount: number;
            lastErrorCode?: string | null;
            lastErrorMessage?: string | null;
            providerMessageId?: string | null;
            payload: Record<string, never>;
            createdAt: string;
            updatedAt: string;
        };
        DeliveryDetail: {
            id: string;
            applicationId?: string | null;
            invocationId?: string | null;
            executionId: string;
            channelBindingId: string;
            provider: string;
            origin: string;
            /** @enum {string} */
            status: "pending" | "delivering" | "delivered" | "failed" | "dead";
            attemptCount: number;
            lastErrorCode?: string | null;
            lastErrorMessage?: string | null;
            providerMessageId?: string | null;
            payload: Record<string, never>;
            createdAt: string;
            updatedAt: string;
            target: Record<string, never>;
            hasCredentialSnapshot?: boolean;
        };
        DeliveryPage: {
            apiVersion: number;
            items: components["schemas"]["DeliverySummary"][];
        };
        KnowledgeDocument: {
            id: string;
            ragResourceId: string;
            name: string;
            contentType: string;
            sizeBytes: number;
            externalDocumentId?: string | null;
            /** @enum {string} */
            status: "uploading" | "indexing" | "indexed" | "failed";
            errorCode?: string | null;
            errorMessage?: string | null;
            indexedAt?: string | null;
            version: number;
            createdAt: string;
        };
        RetrievalTestResult: {
            text?: string;
            documents?: Record<string, never>[];
            citations?: unknown[];
            recordIds?: unknown[];
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
