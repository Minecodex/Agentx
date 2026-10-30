import type { WaitInputNode, WaitInputs } from "../../shared/components/wait-inputs-editor";
import { useAuth } from '../../app/providers/auth-provider';
import { DesignEffectsContext } from "./model/design-effects";
import { BuilderPanel } from "./panels/builder-panel";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useParams } from "react-router-dom";
import { useShallow } from "zustand/react/shallow";

import { ApiClientError, apiRequest, jsonBody } from "../../shared/api/client";
import type {
  Workflow,
  WorkflowDraft,
  WorkflowVersion,
  WorkflowEditorContext,
} from "../../shared/api/types";
import { localizedNodeLabel } from "./model/manifest-localization";
import { Button } from "../../shared/ui/button";
import { Dialog, DialogContent } from "../../shared/ui/dialog";
import { useToast } from "../../shared/ui/toast";
import {
  loadNodeCatalog,
  loadNodeDefinition,
  saveDraft,
  startDebugExecution,
  validateDraft,
  cancelExecution,
} from "./api/studio-api";
import { useExecutionEvents } from "./api/use-execution-events";
import { useProviderOptions } from "./api/use-provider-options";
import { useResolvedPluginManifests } from "./api/use-resolved-plugin-manifests";
import { useResourceOptions, type ResourceOptionRequest } from "./api/use-resource-options";
import { WorkflowFlow, type WorkflowFlowHandle } from "./canvas/workflow-flow";
import { deserializeDraft, serializeStudio } from "./model/serializer";
import type { ExitNodeData, NodeManifest, ResourceOption, ResourceRequestContext, ResourceType, StudioDocument } from "./model/types";
import {
  findOpenCanvasPosition,
  type CanvasPlacementRect,
} from "./nodes/node-appearance";
import { NodeInspector } from "./panels/node-inspector";
import { ExitPanel } from "./panels/inspectors/exit-panel";
import { StartPanel } from "./panels/inspectors/start-panel";
import { NodePalette } from "./panels/node-palette";
import { RuntimePanel } from "./panels/runtime-panel";
import {
  RunParametersDialog,
  hasRunParameters,
} from "./panels/run-parameters-dialog";
import { StudioToolbar } from "./panels/studio-toolbar";
import {
  DebugRunDialog,
  type DebugInputSource,
} from "./panels/debug-run-dialog";
import { buildReferenceCatalog } from "./forms/reference-picker/reference-path";
import { useEditorStore, useEditorStoreApi, EditorStoreProvider } from "./store/editor-store";
import { layoutStudioDocument } from "./utils/layout";
import { LOOP_FIRST_CHILD_POSITION } from "./utils/layout-constants";
import { boundaryGeometry, studioNodeGeometry } from "./nodes/canvas-node-geometry";
import { createDefinitionSelector } from "./store/definition-selector";
import { useStudioShortcuts } from "./utils/use-studio-shortcuts";
import { configurationIssues } from "./utils/configuration";
import { definitionIssues } from "./utils/definition-validation";
import { includedActionNodeIds } from "./utils/debug-plan";
import { iterationEndId } from "./utils/connections";
import { manifestForNode } from "./utils/manifest-lookup";

type DebugMode = "full" | "single_node" | "to_node" | "from_node";
type LocalRecovery = {
  revision: number;
  savedAt: string;
  document: StudioDocument;
  localDocumentRevision: number;
  history: Pick<import("./store/editor-store").EditorState,"past"|"future"|"appliedCommands">;
};
type StudioValidationIssue = {
  code: string;
  message: string;
  nodeId?: string | null;
  fieldPath?: string | null;
  values?: Record<string, string>;
};

function combineFrozenManifests(results: Array<{ data?: { manifest: NodeManifest } }>) {
  return results.flatMap((result) => result.data ? [result.data.manifest] : []);
}

export function WorkflowCanvas({ readOnly = false }: { readOnly?: boolean }) {
  const { workflowId = "", versionId = "" } = useParams();
  const { t } = useTranslation();
  const context = useQuery({ enabled: !readOnly, queryKey: ["workflow-editor-context", workflowId, versionId], queryFn: () => apiRequest<WorkflowEditorContext>(`/workflows/${workflowId}/versions/${versionId}/edit`, { method: "POST" }), staleTime: 0, refetchOnMount: "always", refetchOnWindowFocus: false });
  if (!readOnly && (context.isFetching || !context.data)) return <div className="grid h-dvh place-items-center text-sm"><p role={context.error ? "alert" : "status"}>{context.error?.message ?? t("workflows.loading")}</p></div>;
  return <DesignEffectsContext.Provider value><EditorStoreProvider key={`${workflowId}:${versionId}:${readOnly}`} readOnly={readOnly}><WorkflowCanvasEditor viewOnly={readOnly} documentId={readOnly ? undefined : context.data?.document.id} /></EditorStoreProvider></DesignEffectsContext.Provider>;
}
export function WorkflowCanvasEditor({ viewOnly = false, documentId = "" }: { viewOnly?: boolean; documentId?: string }) {
  const storeApi = useEditorStoreApi();
  const { workflowId = "", versionId = "" } = useParams();
  const draftId = documentId;
  const auth = useAuth();
  const documentKey = `${auth.user?.companyId}:${auth.user?.id}:${workflowId}:${versionId}:${viewOnly ? "view" : "edit"}`;
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const { showToast } = useToast();
  const hydrated = useRef(false);
  const [revision, setRevision] = useState(0);
  const [mode, setMode] = useState<DebugMode>("full");
  const [aiOpen, setAiOpen] = useState(false);
  const [executionId, setExecutionId] = useState<string>();
  const [conflictOpen, setConflictOpen] = useState(false);
  const [recovery, setRecovery] = useState<LocalRecovery>();
  const [issues, setIssues] = useState<StudioValidationIssue[]>([]);
  const [locatedIssue, setLocatedIssue] = useState<StudioValidationIssue>();
  const [restoreOpen, setRestoreOpen] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const lifecyclePending = useRef(false);
  const restoreOperation = useRef<{ revision: number; key: string } | undefined>(undefined);
  const [overlayIds, setOverlayIds] = useState<Record<string, string>>({});
  const [debugDialogOpen, setDebugDialogOpen] = useState(false);
  const [runParametersOpen, setRunParametersOpen] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [interfaceBoundary, setInterfaceBoundary] = useState<"start">();
  const [exitPanelId, setExitPanelId] = useState<string>();
  const [paletteCollapsed, setPaletteCollapsed] = useState(false);
  const [creatorSource, setCreatorSource] = useState<{
    nodeId: string;
    handleId: string;
    manifestPortName?: string;
  }>();
  const flowRef = useRef<WorkflowFlowHandle>(null);
  useEffect(() => {
    if ((detailsOpen || interfaceBoundary || exitPanelId) && window.innerWidth < 1440) setPaletteCollapsed(true);
  }, [detailsOpen, exitPanelId, interfaceBoundary]);
  const openCreatorFromSource = useCallback(
    (nodeId: string, handleId: string, manifestPortName?: string) => {
      setCreatorSource({ nodeId, handleId, manifestPortName });
      setPaletteCollapsed(false);
      setInterfaceBoundary(undefined);
      setExitPanelId(undefined);
      setDetailsOpen(false);
    },
    [],
  );

  const workflow = useQuery({
    queryKey: ["workflow", workflowId],
    queryFn: () => apiRequest<Workflow>(`/workflows/${workflowId}`),
  });
  const draft = useQuery({
    queryKey: ["workflow-document", workflowId, draftId || versionId],
    refetchOnMount: "always",
    queryFn: async (): Promise<WorkflowDraft & {nodeManifests?: Record<string, NodeManifest>}> => {
      if (!viewOnly) return apiRequest<WorkflowDraft>(`/workflows/${workflowId}/drafts/${draftId}`);
      const value = await apiRequest<WorkflowVersion>(`/workflows/${workflowId}/versions/${versionId}`);
      return { id: value.id, workflowId: value.workflowId, versionId: value.id, versionNumber: value.versionNumber, publicationStatus: value.publicationStatus, archived: value.archived, generation: 0, baseVersionId: value.id, revision: value.sourceRevision, schemaVersion: value.schemaVersion, definition: value.definition, editorDocument: value.editorDocument, definitionHash: value.contentHash, editorHash: value.editorHash, updatedAt: value.updatedAt, nodeManifests: value.nodeManifests as unknown as Record<string,NodeManifest> };
    },
  });
  const catalog = useQuery({
    queryKey: ["node-definitions"],
    queryFn: loadNodeCatalog,
    staleTime: 60_000,
  });
  const missingManifestKeys = useMemo(() => {
    const available = new Set((catalog.data ?? []).map((entry) => `${entry.manifest.nodeType}@${entry.manifest.version}`));
    const definition = draft.data?.definition as { nodes?: Array<{ type?: string; typeVersion?: number }> } | undefined;
    return [...new Set((definition?.nodes ?? [])
      .filter((node) => node.type && node.type !== "exit" && Number.isInteger(node.typeVersion))
      .map((node) => `${node.type}@${node.typeVersion}`)
      .filter((key) => !available.has(key)))]
      .map((key) => { const separator = key.lastIndexOf("@"); return { nodeType: key.slice(0, separator), version: Number(key.slice(separator + 1)) }; });
  }, [catalog.data, draft.data?.definition]);
  const documentManifests = useQueries({
    queries: missingManifestKeys.map(({ nodeType, version }) => ({
      queryKey: ["node-definition-frozen", nodeType, version],
      queryFn: () => loadNodeDefinition(nodeType, version),
      staleTime: Infinity,
    })),
    combine: combineFrozenManifests,
  });
  const resourceOptionRequests = useMemo(
    () => collectResourceOptionRequests((catalog.data ?? []).map((entry) => entry.manifest)),
    [catalog.data],
  );
  const resources = useResourceOptions(workflowId, draftId, resourceOptionRequests);
  const authorizeResource = useCallback(async (option: ResourceOption) => {
    await apiRequest(`/workflows/${workflowId}/resource-authorizations`, {
      method: "POST",
      headers: { "Idempotency-Key": crypto.randomUUID() },
      body: jsonBody({ resourceType: option.resourceType ?? "model", resourceId: option.value, resourceVersionId: option.versionId ?? null, operation: option.operation ?? "use" }),
    });
    await queryClient.invalidateQueries({ queryKey: ["studio-resource-options", workflowId] });
    showToast(t("studio.toasts.resourceAuthorized"));
  }, [queryClient, showToast, t, workflowId]);
  const manifests = useMemo(
    () => (catalog.data ?? []).map((item) => item.manifest),
    [catalog.data],
  );
  const subworkflowOptions = useProviderOptions(
    manifests.find((manifest) => manifest.nodeType === "sub_workflow"),
  );
  const baseManifestMap = useMemo(
    () => {
      const available = [
        ...manifests,
        ...documentManifests,
        ...(subworkflowOptions.workflowVersionId ?? []).flatMap((option) => option.manifest ? [option.manifest] : []),
      ];
      return new Map(
        available.map((manifest) => [
          `${manifest.nodeType}@${manifest.version}`,
          manifest,
        ]),
      );
    },
    [documentManifests, manifests, subworkflowOptions.workflowVersionId],
  );
  const definitionSelector = useMemo(createDefinitionSelector, []);
  const resolvingDocument = useEditorStore(definitionSelector);
  const pluginDefinitions = useResolvedPluginManifests(
    workflowId,
    draftId,
    resolvingDocument,
    baseManifestMap,
  );
  const manifestMap = useMemo(() => {
    const values = new Map(baseManifestMap);
    for (const [nodeId, manifest] of pluginDefinitions.resolved) values.set(`node:${nodeId}`, manifest);
    for (const [nodeId, manifest] of Object.entries(draft.data?.nodeManifests ?? {})) values.set(`node:${nodeId}`,manifest);
    return values;
  }, [baseManifestMap, pluginDefinitions.resolved, draft.data?.nodeManifests]);

  const editor = useEditorStore(
    useShallow((state) => ({
      hydrate: state.hydrate,
      markSaved: state.markSaved,
      alignSelected: state.alignSelected,
      replaceNodes: state.replaceNodes,
      undo: state.undo,
      redo: state.redo,
      addAction: state.addAction,
      addConnectedAction: state.addConnectedAction,
      insertActionOnEdge: state.insertActionOnEdge,
      clearEdgeInsertRequest: state.clearEdgeInsertRequest,
      addExit: state.addExit,
      addAnnotation: state.addAnnotation,
      addGroup: state.addGroup,
      select: state.select,
      updateNode: state.updateNode,
      removeSelected: state.removeSelected,
      setStart: state.setStart,
      setEnd: state.setEnd,
    })),
  );
  const selectedId = useEditorStore((state) => state.selectedId);
  const selected = resolvingDocument.nodes.find((node) => node.id === selectedId);
  const dirty = useEditorStore((state) => state.dirty);
  const readOnly = viewOnly || !draft.data || restoreOpen || restoring || draft.data.archived || workflow.data?.canEdit === false || workflow.data?.status === "archived";
  useEffect(() => {storeApi.setState({readOnly});}, [readOnly, storeApi]);
  const canUndo = useEditorStore((state) => state.past.length > 0);
  const canRedo = useEditorStore((state) => state.future.length > 0);
  const selectedCount = useEditorStore(
    (state) =>
      state.nodes.filter(
        (node) => node.selected || node.id === state.selectedId,
      ).length,
  );
  const edgeInsertRequest = useEditorStore((state) => state.edgeInsertRequest);
  const canvasNodes = resolvingDocument.nodes;
  const exitNodes = useMemo(
    () =>
      canvasNodes
        .filter((node) => node.data.editorKind === "exit")
        .map((node) => ({ id: node.id, data: node.data as ExitNodeData })),
    [canvasNodes],
  );
  const referenceSource = resolvingDocument;
  const waitDefinition = useMemo(() => serializeStudio(referenceSource).definition, [referenceSource]);
  const waitScope = useMemo(() => [...includedActionNodeIds(referenceSource, mode, selected?.data.editorKind === "action" ? selected.id : undefined)], [referenceSource, mode, selected]);
  const waitNodes = useQuery({
    queryKey: ["studio-wait-nodes", workflowId, waitDefinition, waitScope],
    queryFn: () => apiRequest<WaitInputNode[]>(`/workflows/${workflowId}/wait-input-nodes`, { method: "POST", body: JSON.stringify({ definition:waitDefinition, includedNodeIds:waitScope }), headers: { "Content-Type":"application/json" } }),
    enabled: Boolean(workflowId) && !readOnly,
  });

  const selectedManifest = selected ? manifestForNode(manifestMap, selected) : undefined;
  const pluginVersionContext = useMemo(() => {
    if (!selected || !selectedManifest?.plugin) return undefined;
    const packageId = selectedManifest.plugin.packageId;
    return {
      affectedNodes: canvasNodes.filter((node) => manifestForNode(manifestMap, node)?.plugin?.packageId === packageId).length,
      connectedInputs: referenceSource.edges.filter((edge) => edge.target === selected.id).map((edge) => edge.targetHandle ?? 'main'),
      connectedOutputs: referenceSource.edges.filter((edge) => edge.source === selected.id).map((edge) => edge.sourceHandle ?? 'main'),
      dependentReferences: canvasNodes.filter((node) => node.id !== selected.id && JSON.stringify(node.data).includes(selected.id)).length,
    };
  }, [canvasNodes, manifestMap, referenceSource.edges, selected, selectedManifest?.plugin]);
  const selectedPluginFieldErrors = Object.fromEntries(
    (selected ? pluginDefinitions.states.get(selected.id)?.issues ?? [] : [])
      .flatMap((issue) => [[issue.path, issue.message], [`parameters.${issue.path}`, issue.message]]),
  );
  const executionReferenceRoot = t("studio.executionReferences.root");
  const referenceCatalog = useMemo(
    () => buildReferenceCatalog(referenceSource, manifestMap, selected?.id, (key, fallback) => key === "studio.executionReferences.root" ? executionReferenceRoot : t(key, fallback)),
    [executionReferenceRoot, manifestMap, referenceSource, selected?.id, t],
  );
  const loopOutputCatalog = useMemo(
    () => selected?.data.editorKind === "action" && selected.data.nodeType === "loop_over_items"
      ? buildReferenceCatalog(referenceSource, manifestMap, iterationEndId(selected.id), (key, fallback) => key === "studio.executionReferences.root" ? executionReferenceRoot : t(key, fallback))
      : undefined,
    [executionReferenceRoot, manifestMap, referenceSource, selected, t],
  );
  const exitMainCatalog = useMemo(
    () => exitPanelId ? buildReferenceCatalog(referenceSource, manifestMap, exitPanelId, (key, fallback) => key === "studio.executionReferences.root" ? executionReferenceRoot : t(key, fallback), "main") : undefined,
    [executionReferenceRoot, exitPanelId, manifestMap, referenceSource, t],
  );
  const exitErrorCatalog = useMemo(
    () => exitPanelId ? buildReferenceCatalog(referenceSource, manifestMap, exitPanelId, (key, fallback) => key === "studio.executionReferences.root" ? executionReferenceRoot : t(key, fallback), "error") : undefined,
    [executionReferenceRoot, exitPanelId, manifestMap, referenceSource, t],
  );
  const creatorSourceManifest = creatorSource
    ? (() => {
        const node = storeApi
          .getState()
          .nodes.find((item) => item.id === creatorSource.nodeId);
        return node ? manifestForNode(manifestMap, node) : undefined;
      })()
    : undefined;
  const runtime = useExecutionEvents(executionId);
  const revalidatePaste = useCallback(
    async (nodes: StudioDocument["nodes"], edges: StudioDocument["edges"]) => {
      const state = storeApi.getState();
      const candidate = studioDocument({
        ...state,
        nodes: [...state.nodes, ...nodes],
        edges: [...state.edges, ...edges],
      });
      const pastedIds = new Set(nodes.map((node) => node.id));
      const local = configurationIssues(candidate, manifestMap).filter(
        (issue) => issue.nodeId && pastedIds.has(issue.nodeId),
      );
      if (local.length) return local.map((issue) => issue.message);
      const serialized = serializeStudio(candidate);
      const result = await validateDraft(
        workflowId,
        draftId,
        serialized.definition,
        serialized.editorDocument,
      );
      return result.issues
        .filter(
          (issue) =>
            issue.severity === "error" &&
            (!issue.nodeId || pastedIds.has(issue.nodeId)),
        )
        .map((issue) => issue.message);
    },
    [manifestMap, storeApi, draftId, workflowId],
  );
  useStudioShortcuts({
    workflowId,
    revalidatePaste,
    onPasteRejected: (messages) =>
      setIssues(
        messages.map((message) => ({
          code: "CROSS_WORKFLOW_PASTE_REJECTED",
          message,
        })),
      ),
    onOpenNode: () => setDetailsOpen(true),
    onEscape: () => {
      setDetailsOpen(false);
      setCreatorSource(undefined);
      storeApi.getState().clearEdgeInsertRequest();
    },
  });

  useEffect(() => {
    if (!draft.data || draft.isFetching || draft.error || hydrated.current) return;
    hydrated.current = true;
    setRevision(draft.data.revision);
    editor.hydrate(deserializeDraft(draft.data));
    const local = readRecovery(documentKey);
    if (
      local &&
      local.revision === draft.data.revision &&
      new Date(local.savedAt) > new Date(draft.data.updatedAt)
    )
      setRecovery(local);
  }, [draft.data, draft.isFetching, draft.error, editor, documentKey]);

  useEffect(() => {
    const edge = edgeInsertRequest;
    if (!edge?.sourceHandle) return;
    setCreatorSource({
      nodeId: edge.source,
      handleId: edge.sourceHandle,
    });
    setPaletteCollapsed(false);
    setInterfaceBoundary(undefined);
  }, [edgeInsertRequest]);

  useEffect(() => {
    let timer: number | undefined;
    const unsubscribe = storeApi.subscribe((state, previous) => {
      const documentChanged =
        state.start !== previous.start ||
        state.nodes !== previous.nodes ||
        state.edges !== previous.edges ||
        state.end !== previous.end ||
        state.viewport !== previous.viewport ||
        state.annotations !== previous.annotations ||
        state.groups !== previous.groups ||
        state.settings !== previous.settings;
      if (!state.dirty || !documentChanged) return;
      if (timer) window.clearTimeout(timer);
      timer = window.setTimeout(
        () =>
          writeRecovery(
            documentKey,
            revision,
            studioDocument(storeApi.getState()),
            storeApi.getState(),
          ),
        750,
      );
    });
    return () => {
      unsubscribe();
      if (timer) window.clearTimeout(timer);
    };
  }, [revision, documentKey, storeApi]);

  useEffect(() => {
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (storeApi.getState().dirty) {writeRecovery(documentKey,revision,studioDocument(storeApi.getState()),storeApi.getState());event.preventDefault();}
    };
    window.addEventListener("beforeunload", beforeUnload);
    return () => window.removeEventListener("beforeunload", beforeUnload);
  }, [storeApi, documentKey, revision]);

  const saveInFlight = useRef<ReturnType<typeof saveDraft> | undefined>(undefined);
  const saveMutation = useMutation({
    mutationFn: ({
      expectedRevision,
      document,
    }: {
      expectedRevision: number;
      localRevision: number;
      document: StudioDocument;
    }) => {
      const serialized = serializeStudio(document);
      return saveDraft(
        workflowId,
        draftId,
        expectedRevision,
        serialized.definition,
        serialized.editorDocument,
      );
    },
    onSuccess: async (value, submitted) => {
      setRevision(value.revision);
      setLocatedIssue(undefined);
      if (editor.markSaved(submitted.localRevision)) clearRecovery(documentKey);
      else writeRecovery(documentKey, value.revision, studioDocument(storeApi.getState()), storeApi.getState());
      queryClient.setQueryData(["workflow-document", workflowId, draftId], value);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["workflow", workflowId] }),
        queryClient.invalidateQueries({
          queryKey: ["workflow-validation", workflowId],
        }),
      ]);
      showToast(t(storeApi.getState().dirty ? "studio.toasts.savedSnapshot" : "studio.toasts.saved"));
    },
    onError: (error: Error, submitted) => {
      if (
        error instanceof ApiClientError &&
        error.detail.code === "DRAFT_REVISION_CONFLICT"
      )
        setConflictOpen(true);
      else if (storeApi.getState().localDocumentRevision !== submitted.localRevision) showToast(error.message);
      else if (
        error instanceof ApiClientError &&
        error.detail.code === "INVALID_WORKFLOW_DRAFT"
      ) {
        const document = submitted.document;
        setIssues(
          (error.detail.fieldErrors ?? []).map((fieldError) => {
            const match = /^nodes\[(\d+)]\.(.+)$/.exec(fieldError.field);
            const node = match ? document.nodes[Number(match[1])] : undefined;
            return {
              code: fieldError.code,
              message: fieldError.message,
              nodeId: node?.id,
              fieldPath: match?.[2] ?? fieldError.field,
            };
          }),
        );
        showToast(error.message);
      } else showToast(error.message);
    },
  });
  const persistDocument = useCallback((input: Parameters<typeof saveMutation.mutateAsync>[0]) => {
    if (saveInFlight.current) return saveInFlight.current;
    const pending = saveMutation.mutateAsync(input);
    saveInFlight.current = pending;
    const clear = () => {
      if (saveInFlight.current === pending) saveInFlight.current = undefined;
    };
    void pending.then(clear, clear);
    return pending;
  }, [saveMutation]);
  const saveDocument = useCallback(
    (showValidation = true) => {
      if (readOnly || lifecyclePending.current || saveInFlight.current || saveMutation.isPending) return;
      const document = studioDocument(storeApi.getState());
      const structuralIssues = definitionIssues(document);
      if (structuralIssues.length) {
        if (showValidation) setIssues(structuralIssues);
        return;
      }
      void persistDocument({ expectedRevision: revision, document, localRevision: storeApi.getState().localDocumentRevision }).catch(() => undefined);
    },
    [persistDocument, readOnly, revision, saveMutation, storeApi],
  );
  const saveNow = useCallback(() => saveDocument(), [saveDocument]);

  const requestResource = useCallback(async (option: ResourceOption, context?: ResourceRequestContext) => {
    // A request is reviewed against a saved revision. Serialize with any
    // autosave, then persist edits made while that earlier save was pending.
    if (saveInFlight.current) await saveInFlight.current;
    let sourceRevision: number | undefined = queryClient.getQueryData<WorkflowDraft>(["workflow-document", workflowId, draftId])?.revision ?? revision;
    while (storeApi.getState().dirty) {
      const document = studioDocument(storeApi.getState());
      // An incomplete Agent may need this resource before it can be saved.
      // Such requests use the existing subject/package scope, without
      // claiming an earlier saved draft as their source.
      if (definitionIssues(document).length) {
        sourceRevision = undefined;
        break;
      }
      const saved = await persistDocument({
        expectedRevision: sourceRevision,
        localRevision: storeApi.getState().localDocumentRevision,
        document,
      });
      sourceRevision = saved.revision;
    }
    await apiRequest(`/workflows/${workflowId}/resource-grant-requests`, {
      method: "POST",
      headers: { "Idempotency-Key": crypto.randomUUID() },
      body: jsonBody({ resourceType: option.resourceType ?? "model", resourceId: option.value, resourceVersionId: option.versionId ?? null, operation: option.operation ?? "use", sourceNodeId: context?.sourceNodeId, sourceRevision, sourceDraftId: sourceRevision === undefined ? undefined : draftId, message: context?.message }),
    });
    await queryClient.invalidateQueries({ queryKey: ["studio-resource-options", workflowId] });
    showToast(t("studio.toasts.resourceRequested"));
  }, [draftId, persistDocument, queryClient, revision, showToast, storeApi, t, workflowId]);

  useEffect(() => {
    let timer: number | undefined;
    const schedule = () => {
      if (timer) window.clearTimeout(timer);
      if (
        !storeApi.getState().dirty ||
        saveMutation.isPending ||
        conflictOpen ||
        recovery || readOnly || lifecyclePending.current
      )
        return;
      timer = window.setTimeout(() => saveDocument(false), 1400);
    };
    const unsubscribe = storeApi.subscribe((state, previous) => {
      if (
        state.nodes !== previous.nodes ||
        state.edges !== previous.edges ||
        state.viewport !== previous.viewport ||
        state.annotations !== previous.annotations ||
        state.groups !== previous.groups ||
        state.settings !== previous.settings ||
        state.dirty !== previous.dirty
      )
        schedule();
    });
    schedule();
    return () => {
      unsubscribe();
      if (timer) window.clearTimeout(timer);
    };
  }, [conflictOpen, recovery, readOnly, saveDocument, saveMutation.isPending, storeApi]);

  const validateCurrent = async () => {
    const document = studioDocument(storeApi.getState());
    const clientIssues = configurationIssues(document, manifestMap);
    if (clientIssues.length) {
      setIssues(clientIssues);
      return false;
    }
    const serialized = serializeStudio(document);
    const result = await validateDraft(
      workflowId,
      draftId,
      serialized.definition,
      serialized.editorDocument,
    );
    const errors = result.issues.filter((issue) => issue.severity === "error");
    setIssues(errors);
    return errors.length === 0;
  };

  const executeRun = async (
    inputSource?: DebugInputSource,
    input: unknown = {},
    debugMode: DebugMode = mode,
    context: Record<string, unknown> = {},
    waitInputs: WaitInputs = {},
  ) => {
    if (readOnly) return;
    if (dirty || saveMutation.isPending)
      return showToast(t("studio.toasts.saveBeforeRun"));
    const target =
      selected?.data.editorKind === "action" ? selected.id : undefined;
    try {
      if (!(await validateCurrent())) return;
      const document = studioDocument(storeApi.getState());
      const included = includedActionNodeIds(document, debugMode, target);
      const irreversible = document.nodes.filter(
        (node) =>
          included.has(node.id) &&
          node.data.editorKind === "action" &&
          manifestForNode(manifestMap, node)?.sideEffectLevel === "irreversible",
      );
      const accepted = await startDebugExecution(workflowId, draftId, {
        expectedRevision: revision,
        mode: debugMode,
        targetNodeId: target,
        input,
        context,
        waitInputs,
        inputSource,
        overlayIds: Object.entries(overlayIds)
          .filter(([nodeId]) => included.has(nodeId))
          .map(([, overlayId]) => overlayId),
        sideEffectDecisions: Object.fromEntries(
          irreversible.map((node) => [node.id, "execute"]),
        ),
        idempotencyKey: crypto.randomUUID(),
      });
      setExecutionId(accepted.executionId);
      runtime.setRunning(true);
      setDebugDialogOpen(false);
      setRunParametersOpen(false);
    } catch (error) {
      showToast((error as Error).message);
    }
  };

  const run = async () => {
    if (readOnly) return;
    if (dirty || saveMutation.isPending)
      return showToast(t("studio.toasts.saveBeforeRun"));
    const target =
      selected?.data.editorKind === "action" ? selected.id : undefined;
    if (mode !== "full" && !target)
      return showToast(t("studio.toasts.partialSelect"));
    const discovered = await waitNodes.refetch();
    if (discovered.error) return showToast(discovered.error.message);
    if (mode === "single_node" || mode === "from_node")
      return setDebugDialogOpen(true);
    if (hasRunParameters(referenceSource.start, discovered.data))
      return setRunParametersOpen(true);
    void executeRun();
  };

  const stop = async () => {
    if (!executionId) return;
    try {
      await cancelExecution(executionId);
      runtime.setRunning(false);
    } catch (error) {
      showToast((error as Error).message);
    }
  };

  const restoreCanvas = async () => {
    if (viewOnly || draft.data?.archived || runtime.running || restoring) return;
    lifecyclePending.current = true;
    setRestoring(true);
    setAiOpen(false);
    try {
      if (saveInFlight.current) await saveInFlight.current;
      const expected = queryClient.getQueryData<WorkflowDraft>(["workflow-document", workflowId, draftId])?.revision ?? revision;
      if (restoreOperation.current?.revision !== expected) restoreOperation.current = { revision: expected, key: crypto.randomUUID() };
      await apiRequest(`/workflows/${workflowId}/versions/${versionId}/restore`, { method: "POST", headers: { "Idempotency-Key": restoreOperation.current.key }, body: jsonBody({ expectedRevision: expected }) });
      const value = await apiRequest<WorkflowDraft>(`/workflows/${workflowId}/drafts/${draftId}`);
      queryClient.setQueryData(["workflow-document", workflowId, draftId], value);
      setRevision(value.revision);
      editor.hydrate(deserializeDraft(value));
      clearRecovery(documentKey);
      setRecovery(undefined);
      setOverlayIds({}); setExecutionId(undefined); setIssues([]); setLocatedIssue(undefined);
      await queryClient.invalidateQueries({ queryKey: ["workflow-versions", workflowId] });
      showToast(t("studio.restoreCanvas.success"));
    } catch (failure) {
      if (failure instanceof ApiClientError && failure.detail.code === "DRAFT_REVISION_CONFLICT") setConflictOpen(true);
      showToast((failure as Error).message);
    } finally { lifecyclePending.current = false; setRestoring(false); setRestoreOpen(false); }
  };

  const actionData = (manifest: NodeManifest) => ({
    editorKind: "action" as const,
    nodeType: manifest.nodeType,
    typeVersion: manifest.version,
    label: manifest.displayName,
    key: uniqueNodeKey(manifest.nodeType, storeApi.getState().nodes),
    parameters: defaults(manifest),
    contextWrites: [],
    resourceReferences: [],
    settings: {},
    disabled: false,
  });
  const occupiedCanvasRects = (): CanvasPlacementRect[] => [
    { ...(storeApi.getState().boundaryLayouts.find(item => item.boundary === 'start') ?? { x: 40, y: 220 }), width: boundaryGeometry('start').width, height: boundaryGeometry('start').height },
    ...storeApi.getState().nodes.map((node) => {
      const nodeManifest = manifestForNode(manifestMap, node);
      const metrics = studioNodeGeometry(node, nodeManifest);
      // Container children carry parent-relative positions: resolve before occupancy checks.
      const parentId =
        node.data.editorKind === "action" ? node.data.parentId : undefined;
      const parent = parentId
        ? storeApi.getState().nodes.find((item) => item.id === parentId)
        : undefined;
      return {
        x: node.position.x + (parent?.position.x ?? 0),
        y: node.position.y + (parent?.position.y ?? 0),
        width: metrics.width,
        height: metrics.height,
      };
    }),
    ...storeApi
      .getState()
      .annotations.map((annotation) => ({
        x: annotation.x,
        y: annotation.y,
        width: annotation.width ?? 240,
        height: annotation.height ?? 160,
      })),
  ];
  const defaultCanvasPosition = (
    size: { width: number; height: number },
    keepVisible = false,
  ) => {
    const center = flowRef.current?.getViewportCenter();
    return center
      ? findOpenCanvasPosition(
          center,
          size,
          occupiedCanvasRects(),
          keepVisible ? flowRef.current?.getViewportBounds() : undefined,
        )
      : undefined;
  };
  const addAction = (
    manifest: NodeManifest,
    position?: { x: number; y: number },
    parentId?: string,
  ) => {
    const metrics = studioNodeGeometry({ data: actionData(manifest) }, manifest);
    return editor.addAction(
      { ...actionData(manifest), parentId },
      position ?? defaultCanvasPosition(metrics),
      metrics,
    );
  };
  const openNodePosition = (position: { x: number; y: number }, size: { width: number; height: number }, parentId?: string) => {
    const occupied = parentId ? storeApi.getState().nodes.filter(node => node.data.editorKind === 'action' && node.data.parentId === parentId).map(node => ({ ...node.position, ...studioNodeGeometry(node, manifestForNode(manifestMap, node)) })) : occupiedCanvasRects();
    return findOpenCanvasPosition({ x: position.x + size.width / 2, y: position.y + size.height / 2 }, size, occupied);
  };
  const addActionFromCreator = (
    manifest: NodeManifest,
    position?: { x: number; y: number },
    targetHandle?: string,
    parentId?: string,
  ) => {
    const state = storeApi.getState();
    if (creatorSource?.nodeId.endsWith("::iteration-start")) {
      const loopId = creatorSource.nodeId.slice(0, -"::iteration-start".length);
      return addAction(manifest, position ?? LOOP_FIRST_CHILD_POSITION, loopId);
    }
    const insertion = state.edgeInsertRequest;
    if (insertion) {
      const sourceNode = state.nodes.find(
        (node) => node.id === insertion.source,
      );
      const targetNode = state.nodes.find(
        (node) => node.id === insertion.target,
      );
      const sourceManifest = sourceNode ? manifestForNode(manifestMap, sourceNode) : undefined;
      const targetManifest = targetNode ? manifestForNode(manifestMap, targetNode) : undefined;
      const sourceKind =
        sourceManifest?.outputPorts.find(
          (port) => port.name === insertion.sourceHandle,
        )?.kind ?? insertion.data?.sourcePortKind;
      const targetKind =
        targetManifest?.inputPorts.find(
          (port) =>
            port.name === insertion.targetHandle ||
            (port.name === "main" &&
              insertion.targetHandle?.startsWith("main:")),
        )?.kind ?? sourceKind;
      const input =
        manifest.inputPorts.find(
          (port) => port.name === targetHandle && port.kind === sourceKind,
        ) ?? manifest.inputPorts.find((port) => port.kind === sourceKind);
      const output = manifest.outputPorts.find(
        (port) => port.kind === targetKind,
      );
      if (sourceNode && targetNode && input && output) {
        const owner = sourceNode.data.editorKind === 'action' ? sourceNode.data.parentId : undefined;
        const nextData = { ...actionData(manifest), parentId: owner };
        const size = studioNodeGeometry({ data: nextData }, manifest);
        return editor.insertActionOnEdge(
          nextData,
          insertion.id,
          { input: input.name, output: output.name },
          position ?? openNodePosition({
            x: (sourceNode.position.x + studioNodeGeometry(sourceNode, sourceManifest).width + targetNode.position.x - size.width) / 2,
            y: (sourceNode.position.y + targetNode.position.y) / 2,
          }, size, owner),
          size,
        );
      }
      editor.clearEdgeInsertRequest();
    }
    if (!creatorSource || !creatorSourceManifest)
      return addAction(manifest, position, parentId);
    const sourceNode = state.nodes.find(
      (node) => node.id === creatorSource.nodeId,
    );
    const sourceParentId = sourceNode?.data.editorKind === "action" ? sourceNode.data.parentId : undefined;
    const nextParentId = parentId ?? sourceParentId;
    const sourcePort = creatorSourceManifest.outputPorts.find(
      (port) => port.name === (creatorSource.manifestPortName ?? creatorSource.handleId),
    );
    const targetPort = sourcePort
      ? (manifest.inputPorts.find(
          (port) => port.name === targetHandle && port.kind === sourcePort.kind,
        ) ?? manifest.inputPorts.find((port) => port.kind === sourcePort.kind))
      : undefined;
    if (!sourceNode || !targetPort) return addAction(manifest, position, nextParentId);
    const sourceMetrics = studioNodeGeometry(sourceNode, creatorSourceManifest);
    const nextData = { ...actionData(manifest), parentId: nextParentId };
    const size = studioNodeGeometry({ data: nextData }, manifest);
    const sourceY = sourceMetrics.ports.find(port => port.direction === 'output' && port.id === creatorSource.handleId)?.y ?? 24;
    const targetY = size.ports.find(port => port.direction === 'input' && port.id === targetPort.name)?.y ?? 24;
    return editor.addConnectedAction(
      nextData,
      {
        nodeId: sourceNode.id,
        handleId: creatorSource.handleId,
        targetHandle: targetPort.name,
      },
      position ?? openNodePosition({
        x: sourceNode.position.x + sourceMetrics.width + 96,
        y: sourceNode.position.y + sourceY - targetY,
      }, size, nextParentId),
      size,
    );
  };
  const loadServer = async () => {
    const result = await draft.refetch();
    if (result.data) {
      editor.hydrate(deserializeDraft(result.data));
      setRevision(result.data.revision);
      clearRecovery(documentKey);
    }
    setConflictOpen(false);
  };
  const overwriteServer = async () => {
    const local = studioDocument(storeApi.getState());
    const localRevision = storeApi.getState().localDocumentRevision;
    const result = await draft.refetch();
    if (result.data)
      void persistDocument({
        localRevision,
        expectedRevision: result.data.revision,
        document: local,
      }).catch(() => undefined);
    setConflictOpen(false);
  };
  const locateIssue = (issue: StudioValidationIssue) => {
    setLocatedIssue(issue);
    setIssues([]);
    if (!issue.nodeId) return;
    editor.select(issue.nodeId);
    setDetailsOpen(true);
    window.setTimeout(() => {
      const field = [
        ...document.querySelectorAll<HTMLElement>("[data-field-path]"),
      ].find((element) => element.dataset.fieldPath === issue.fieldPath);
      if (!field) return;
      field.scrollIntoView({ block: "center" });
      field.classList.add("studio-field-located");
      const control = field.querySelector<HTMLElement>(
        'input, textarea, button, [role="combobox"], [contenteditable="true"]',
      );
      if (control) control.focus();
      else {
        field.tabIndex = -1;
        field.focus();
      }
      window.setTimeout(
        () => field.classList.remove("studio-field-located"),
        1800,
      );
    }, 50);
  };

  if (draft.isLoading || (!hydrated.current && draft.isFetching) || catalog.isLoading)
    return (
      <div className="grid h-full place-items-center text-sm text-muted-foreground">
        {t("studio.loading")}
      </div>
    );
  if (!draft.data || draft.error || catalog.error)
    return (
      <div className="grid h-full place-items-center px-8 text-sm text-danger">
        {String(draft.error ?? catalog.error ?? t("studio.unavailable"))}
      </div>
    );
  return (
    <div className="flex h-dvh min-h-0 flex-col overflow-hidden">
      <StudioToolbar
        onAI={() => setAiOpen(value => !value)}
        canRedo={canRedo}
        canUndo={canUndo}
        dirty={dirty}
        mode={mode}
        name={`${workflow.data?.name ?? t("studio.workflow")} · v${draft.data.versionNumber} · ${t(viewOnly ? "studio.restoreCanvas.readOnly" : draft.data.publicationStatus === "published" ? "studio.restoreCanvas.draft" : "studio.restoreCanvas.unpublished")}`}
        onAlign={editor.alignSelected}
        onLayout={() => {
          const state = storeApi.getState();
          const expectedRevision = state.localDocumentRevision;
          void layoutStudioDocument(studioDocument(state), manifestMap).then(document => {
            storeApi.getState().applyEditorCommand(crypto.randomUUID(), expectedRevision, document);
          }).catch((error: Error) => showToast(error.message));
        }}
        onMode={(value) => setMode(value as DebugMode)}
        onRedo={editor.redo}
        onRun={() => void run()}
        onSave={saveNow}
        onStop={() => void stop()}
        onUndo={editor.undo}
        revision={revision}
        running={runtime.running}
        readOnly={readOnly}
        canRun={auth.hasPermission("execution:run") && !readOnly}
        onRestore={() => setRestoreOpen(true)}
        canRestore={!readOnly && !runtime.running}
        restoreTarget={t(draft.data.publicationStatus === "published" ? "studio.restoreCanvas.targetVersion" : "studio.restoreCanvas.targetInitial", { version: draft.data.versionNumber })}
        saving={saveMutation.isPending}
        selectedCount={selectedCount}
        workflowId={workflowId}
      />
      <main className="relative flex min-h-0 flex-1">
        {!viewOnly && <NodePalette
          collapsed={paletteCollapsed}
          manifests={readOnly ? [] : manifests}
          onAddAction={(manifest, targetHandle) => {
            addActionFromCreator(manifest, undefined, targetHandle);
            setCreatorSource(undefined);
            setInterfaceBoundary(undefined);
            setDetailsOpen(true);
          }}
          onAddAnnotation={() => {
            editor.addAnnotation(
              defaultCanvasPosition({ width: 240, height: 160 }, true),
              t("studio.note.default"),
            );
            setCreatorSource(undefined);
          }}
          onAddExit={() => {
            const id = editor.addExit();
            if (creatorSource && creatorSourceManifest) {
              const sourcePort = creatorSourceManifest.outputPorts.find(
                (port) => port.name === (creatorSource.manifestPortName ?? creatorSource.handleId),
              );
              if (sourcePort) {
                const state = storeApi.getState();
                const order = state.edges.filter((edge) => edge.source === creatorSource.nodeId && edge.sourceHandle === creatorSource.handleId && edge.data?.edgeKind === 'execution').length;
                state.connect({ source: creatorSource.nodeId, sourceHandle: creatorSource.handleId, target: id, targetHandle: sourcePort.kind === 'error' ? 'error' : 'main' }, { edgeKind: 'execution', order, sourcePortKind: sourcePort.kind });
              }
            }
            setCreatorSource(undefined);
            setInterfaceBoundary(undefined);
            setExitPanelId(id);
            setDetailsOpen(false);
          }}
          onAddGroup={() => {
            if (!editor.addGroup(t("studio.group.default"))) showToast(t("studio.group.selectionHelp"));
            setCreatorSource(undefined);
          }}
          onCollapsedChange={(collapsed) => {
            setPaletteCollapsed(collapsed);
            if (collapsed) {
              setCreatorSource(undefined);
              editor.clearEdgeInsertRequest();
            }
          }}
          sourceConnection={
            creatorSourceManifest && creatorSource
              ? {
                  manifest: creatorSourceManifest,
                  handleId: creatorSource.handleId,
                  manifestPortName: creatorSource.manifestPortName,
                }
              : undefined
          }
        />}
        <div className="flex min-w-0 flex-1 flex-col">
          <WorkflowFlow
            ref={flowRef}
            manifests={manifestMap}
            readOnly={readOnly}
            onQuickAdd={openCreatorFromSource}
            onDropAction={(manifest, position, parentId) => {
              addActionFromCreator(manifest, position, undefined, parentId);
              setCreatorSource(undefined);
              setDetailsOpen(true);
            }}
            onBoundaryOpen={(boundary) => {
              setInterfaceBoundary(boundary);
              setExitPanelId(undefined);
              setDetailsOpen(false);
            }}
            onNodeOpen={(nodeId) => {
              setInterfaceBoundary(undefined);
              editor.select(nodeId);
              const node = storeApi.getState().nodes.find((item) => item.id === nodeId);
              if (node?.data.editorKind === "exit") {
                setExitPanelId(nodeId);
                setDetailsOpen(false);
              } else {
                setExitPanelId(undefined);
                setDetailsOpen(true);
              }
            }}
            onPaneClear={() => {
              setDetailsOpen(false);
              setInterfaceBoundary(undefined);
              setExitPanelId(undefined);
            }}
            runtimeStatuses={runtime.nodeStatuses}
          />
          <RuntimePanel
            error={runtime.error}
            events={runtime.events}
            executionId={executionId}
            onExecutionChange={setExecutionId}
            onNodeSelect={(nodeId) => {
              editor.select(nodeId);
              setDetailsOpen(true);
            }}
            workflowId={workflowId}
          />
        </div>
        <NodeInspector
          data={
            selected?.data.editorKind === "action" ? selected.data : undefined
          }
          executionId={executionId}
          fieldErrors={{
            ...selectedPluginFieldErrors,
            ...Object.fromEntries(
              [...issues, ...(locatedIssue ? [locatedIssue] : [])]
                .filter(
                  (issue) => issue.nodeId === selected?.id && issue.fieldPath,
                )
                .map((issue) => [issue.fieldPath!, issue.message]),
            ),
          }}
          manifest={selectedManifest}
          pluginVersions={selectedManifest?.plugin ? manifests.filter((manifest) => manifest.nodeType === selectedManifest.nodeType && manifest.plugin?.packageId === selectedManifest.plugin?.packageId) : []}
          pluginVersionContext={pluginVersionContext}
          nodeId={selected?.id}
          onChange={(patch) => {
            if (readOnly) return;
            setLocatedIssue(undefined);
            if (!selected || selected.data.editorKind !== "action") return;
            if (patch.typeVersion && selectedManifest?.plugin) {
              const target = manifests.find((candidate) => candidate.nodeType === patch.nodeType && candidate.version === patch.typeVersion && candidate.plugin?.packageId === selectedManifest.plugin?.packageId);
              if (target?.plugin) {
                const state = storeApi.getState();
                editor.replaceNodes(state.nodes.map((node) => {
                  if (node.data.editorKind !== "action") return node;
                  const data = node.data;
                  const current = manifestForNode(manifestMap, node);
                  if (current?.plugin?.packageId !== target.plugin?.packageId) return node;
                  const matching = manifests.find((candidate) => candidate.nodeType === data.nodeType && candidate.plugin?.packageId === target.plugin?.packageId && candidate.plugin?.packageVersion === target.plugin?.packageVersion);
                  return matching ? { ...node, data: { ...data, typeVersion: matching.version } } : node;
                }));
                return;
              }
            }
            const nextMode = patch.parameters?.mode;
            const currentMode = selected.data.parameters.mode ?? "append";
            if (selected.data.nodeType === "merge" && nextMode && nextMode !== currentMode) {
              const hasInputs = storeApi.getState().edges.some((edge) => edge.target === selected.id);
              if (hasInputs && !window.confirm(t("studio.panels.merge.modeChangeWarning"))) return;
            }
            editor.updateNode(selected.id, patch);
          }}
          onClose={() => setDetailsOpen(false)}
          onDelete={() => {
            editor.removeSelected();
            setDetailsOpen(false);
          }}
          onOverlayChange={(nodeId, id) =>
            setOverlayIds((current) => {
              const next = { ...current };
              if (id) next[nodeId] = id;
              else delete next[nodeId];
              return next;
            })
          }
          onResourceAuthorize={authorizeResource}
          onResourceRequest={requestResource}
          onRun={() => { setMode("single_node"); setDebugDialogOpen(true); }}
          open={detailsOpen && !interfaceBoundary}
          referenceCatalog={referenceCatalog}
          parameterCatalogs={loopOutputCatalog ? { outputSelector: loopOutputCatalog } : undefined}
          readOnly={readOnly}
          resources={resources.options}
          workflowId={workflowId}
          draftId={draftId}
        />
        {interfaceBoundary && (
          <StartPanel
            readOnly={readOnly}
            onClose={() => setInterfaceBoundary(undefined)}
            onStartChange={editor.setStart}
            start={referenceSource.start}
          />
        )}
        {exitPanelId && selected?.data.editorKind === "exit" && selected.id === exitPanelId && (
          <ExitPanel
            readOnly={readOnly}
            end={referenceSource.end}
            exitId={exitPanelId}
            exits={exitNodes}
            onClose={() => setExitPanelId(undefined)}
            onEndChange={editor.setEnd}
            onExitUpdate={(id, patch) => editor.updateNode(id, patch)}
            errorReferenceCatalog={exitErrorCatalog}
            referenceCatalog={exitMainCatalog}
          />
        )}
        {aiOpen && !readOnly && <BuilderPanel manifests={manifestMap} workflowId={workflowId} draftId={draftId} serverRevision={revision} executionId={executionId} onClose={() => setAiOpen(false)} persistRecovery={() => writeRecovery(documentKey, revision, studioDocument(storeApi.getState()), storeApi.getState())} onLocate={locateIssue} checkDocument={document => definitionIssues(document).map(issue => issue.message)} />}
      </main>
      <ConflictDialog
        onClose={() => setConflictOpen(false)}
        onLoad={() => void loadServer()}
        onOverwrite={() => void overwriteServer()}
        open={conflictOpen}
      />
      <RecoveryDialog
        onDiscard={() => {
          clearRecovery(documentKey);
          setRecovery(undefined);
        }}
        onRestore={() => {
          if (recovery) {
            editor.hydrate(recovery.document);
          storeApi.setState({localDocumentRevision:recovery.localDocumentRevision,...recovery.history,dirty:true});
            setRevision(recovery.revision);
          }
          setRecovery(undefined);
        }}
        open={Boolean(recovery)}
      />
      <IssuesDialog
        issues={issues}
        onClose={() => setIssues([])}
        onLocate={locateIssue}
      />
      <RunParametersDialog
        onClose={() => setRunParametersOpen(false)}
        onRun={(input, context, waitInputs) =>
          void executeRun(undefined, input, mode, context, waitInputs)
        }
        open={runParametersOpen}
        running={runtime.running}
        start={referenceSource.start}
        waitNodes={waitNodes.data}
      />
      {selected?.data.editorKind === "action" &&
        (mode === "single_node" || mode === "from_node") && (
          <DebugRunDialog
            executionId={executionId}
            waitNodes={waitNodes.data}
            discoveryPending={waitNodes.isFetching}
            discoveryError={waitNodes.error?.message}
            mode={mode}
            onClose={() => setDebugDialogOpen(false)}
            onRun={(source, input, waitInputs) => void executeRun(source, input, mode, {}, waitInputs)}
            open={debugDialogOpen}
            running={runtime.running}
            targetName={localizedNodeLabel(
              manifestForNode(manifestMap, selected),
              selected.data.label,
              i18n.language,
            )}
          />
        )}
      <Dialog open={restoreOpen} onOpenChange={open => { if (!restoring) setRestoreOpen(open); }}><DialogContent title={t("studio.restoreCanvas.title")}>
        <div className="p-6"><h2 className="text-lg font-semibold">{t("studio.restoreCanvas.title")}</h2><p className="mt-3 text-sm text-muted-foreground">{t(draft.data.publicationStatus === "published" ? "studio.restoreCanvas.confirmVersion" : "studio.restoreCanvas.confirmInitial", { version: draft.data.versionNumber })}</p><div className="mt-6 flex justify-end gap-2"><Button variant="ghost" disabled={restoring} onClick={() => setRestoreOpen(false)}>{t("common.cancel")}</Button><Button disabled={restoring} onClick={() => void restoreCanvas()}>{t(restoring ? "studio.restoreCanvas.restoring" : "studio.restoreCanvas.confirm")}</Button></div></div>
      </DialogContent></Dialog>
    </div>
  );
}

function ConflictDialog({
  open,
  onClose,
  onLoad,
  onOverwrite,
}: {
  open: boolean;
  onClose: () => void;
  onLoad: () => void;
  onOverwrite: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Dialog onOpenChange={(value) => !value && onClose()} open={open}>
      <DialogContent
        description={t("studio.conflict.description")}
        title={t("studio.conflict.title")}
      >
        <div className="p-5">
          <h2 className="text-sm font-semibold">
            {t("studio.conflict.title")}
          </h2>
          <p className="mt-2 text-xs leading-5 text-muted-foreground">
            {t("studio.conflict.description")}
          </p>
          <div className="mt-5 flex flex-wrap justify-end gap-2">
            <Button onClick={onClose} variant="ghost">
              {t("studio.conflict.keep")}
            </Button>
            <Button onClick={onLoad} variant="secondary">
              {t("studio.conflict.load")}
            </Button>
            <Button onClick={onOverwrite}>
              {t("studio.conflict.overwrite")}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
function RecoveryDialog({
  open,
  onDiscard,
  onRestore,
}: {
  open: boolean;
  onDiscard: () => void;
  onRestore: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Dialog open={open}>
      <DialogContent
        description={t("studio.recovery.description")}
        title={t("studio.recovery.title")}
      >
        <div className="p-5">
          <h2 className="text-sm font-semibold">
            {t("studio.recovery.title")}
          </h2>
          <p className="mt-2 text-xs text-muted-foreground">
            {t("studio.recovery.description")}
          </p>
          <div className="mt-5 flex justify-end gap-2">
            <Button onClick={onDiscard} variant="ghost">
              {t("studio.recovery.discard")}
            </Button>
            <Button onClick={onRestore}>{t("studio.recovery.restore")}</Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
function IssuesDialog({
  issues,
  onClose,
  onLocate,
}: {
  issues: StudioValidationIssue[];
  onClose: () => void;
  onLocate: (issue: StudioValidationIssue) => void;
}) {
  const { t, i18n } = useTranslation();
  return (
    <Dialog
      onOpenChange={(open) => !open && onClose()}
      open={issues.length > 0}
    >
      <DialogContent
        description={t("studio.validation.description")}
        title={t("studio.validation.title")}
      >
        <div className="p-5">
          <div className="flex items-center gap-2">
            <AlertTriangle className="size-4 text-warning" />
            <h2 className="text-sm font-semibold">
              {t("studio.validation.title")}
            </h2>
          </div>
          <div className="mt-4 max-h-80 divide-y divide-border overflow-auto">
            {issues.map((issue, index) => (
              <button
                className="block w-full py-3 text-left text-xs hover:bg-muted/50"
                disabled={!issue.nodeId}
                key={`${issue.code}-${index}`}
                onClick={() => onLocate(issue)}
                type="button"
              >
                <strong>{issue.code}</strong>
                <p className="mt-1 text-muted-foreground">
                  {issue.nodeId ??
                    issue.fieldPath ??
                    t("studio.validation.workflow")}{" "}
                  · {i18n.exists(`studio.validation.issues.${issue.code}`)
                    ? t(`studio.validation.issues.${issue.code}`, { ...(issue.values ?? {}) })
                    : t("studio.validation.issueFallback", { code: issue.code })}
                </p>
                {issue.nodeId && (
                  <span className="mt-1 block text-[10px] text-primary">
                    {t("studio.validation.locate")}
                  </span>
                )}
              </button>
            ))}
          </div>
          <div className="mt-5 flex justify-end">
            <Button onClick={onClose}>{t("studio.validation.close")}</Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function defaults(manifest: NodeManifest) {
  return Object.fromEntries(
    Object.entries(manifest.parameterSchema.properties ?? {}).flatMap(
      ([name, schema]) =>
        schema.default === undefined
          ? []
          : [[name, structuredClone(schema.default)]],
    ),
  );
}
function studioDocument(
  state: Pick<
    StudioDocument,
    | "start"
    | "nodes"
    | "edges"
    | "end"
    | "boundaryLayouts"
    | "viewport"
    | "annotations"
    | "groups"
    | "settings"
  >,
): StudioDocument {
  return {
    start: state.start,
    nodes: state.nodes,
    edges: state.edges,
    end: state.end,
    boundaryLayouts: state.boundaryLayouts,
    viewport: state.viewport,
    annotations: state.annotations,
    groups: state.groups,
    settings: state.settings,
  };
}
function uniqueNodeKey(nodeType: string, nodes: StudioDocument["nodes"]) {
  const keys = new Set(
    nodes.flatMap((node) =>
        node.data.editorKind === "action" ? [node.data.key] : [],
      ),
  );
  const base = nodeType.replace(/[^a-z0-9_]+/g, "_");
  let candidate = base;
  let index = 2;
  while (keys.has(candidate)) candidate = `${base}_${index++}`;
  return candidate;
}
function recoveryKey(workflowId: string) {
  return `agentx:studio:${workflowId}`;
}
function writeRecovery(
  workflowId: string,
  revision: number,
  document: StudioDocument,
  state: import("./store/editor-store").EditorState,
) {
  try {
    localStorage.setItem(
      recoveryKey(workflowId),
      JSON.stringify({ revision, savedAt: new Date().toISOString(), document, localDocumentRevision: state.localDocumentRevision, history: {past:state.past,future:state.future,appliedCommands:state.appliedCommands} }),
    );
    return true;
  } catch {
    return false;
  }
}
function readRecovery(workflowId: string): LocalRecovery | undefined {
  try {
    const value = JSON.parse(
      localStorage.getItem(recoveryKey(workflowId)) ?? "null",
    ) as LocalRecovery | null;
    return value?.document?.nodes && value?.document?.edges && Number.isInteger(value.localDocumentRevision) && value.history ? value : undefined;
  } catch {
    return undefined;
  }
}
function clearRecovery(workflowId: string) {
  try {
    localStorage.removeItem(recoveryKey(workflowId));
  } catch {
    /* Local recovery is best effort. */
  }
}

const defaultResourceOptionRequests: ResourceOptionRequest[] = [
  { resourceType: "credential", operation: "use" },
  { resourceType: "model", operation: "use" },
  { resourceType: "mcp_server", operation: "use" },
  { resourceType: "mcp_tool", operation: "use" },
  { resourceType: "skill", operation: "use" },
  { resourceType: "rag", operation: "read" },
  { resourceType: "memory", operation: "use" },
  { resourceType: "sandbox_profile", operation: "use" },
];

function collectResourceOptionRequests(manifests: NodeManifest[]): ResourceOptionRequest[] {
  const requests = [...defaultResourceOptionRequests];
  for (const manifest of manifests) {
    for (const value of manifest.uiSchema.resourceSelectors ?? []) {
      if (!value || typeof value !== "object") continue;
      const selector = value as Partial<ResourceOptionRequest>;
      if (isResourceType(selector.resourceType) && isResourceOperation(selector.operation)) {
        requests.push({ resourceType: selector.resourceType, operation: selector.operation });
      }
    }
  }
  return [...new Map(requests.map((request) => [`${request.resourceType}:${request.operation}`, request])).values()];
}

function isResourceType(value: unknown): value is ResourceType {
  return ["credential", "model", "mcp_server", "mcp_tool", "skill", "rag", "memory", "sandbox_profile"].includes(String(value));
}

function isResourceOperation(value: unknown): value is ResourceOptionRequest["operation"] {
  return ["view", "use", "read", "write", "manage"].includes(String(value));
}
