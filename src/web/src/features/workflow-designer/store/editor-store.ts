import { CanvasRenderProvider } from './canvas-render-store';
import { serializeStudio } from "../model/serializer";
import {
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  type Connection,
  type EdgeChange,
  type NodeChange,
  type Viewport,
} from "@xyflow/react";
import { createStore, useStore, type StoreApi } from "zustand";
import { createContext, createElement, useContext, useState, type ReactNode } from "react";

import type {
  ActionNodeData,
  ExitNodeData,
  StudioDocument,
  StudioEdge,
  StudioNode,
} from "../model/types";
import { isIterationChipId, loopIdOfIterationChip } from "../utils/connections";
import {
  LOOP_CONTAINER_MIN_HEIGHT,
  LOOP_CONTAINER_MIN_WIDTH,
  LOOP_CONTAINER_PADDING,
} from "../utils/layout";
import {
  applyHistoryPatch,
  createHistoryPatch,
  type HistoryPatch,
} from "./history-patch";

type DocumentSlice = StudioDocument & { dirty: boolean; graphRevision: number; localDocumentRevision: number; readOnly: boolean; appliedCommands: Record<string, number> };
type GestureSnapshot = {
  nodes: StudioNode[];
  annotations: StudioDocument["annotations"][number][];
  boundaryLayouts: StudioDocument["boundaryLayouts"];
  includeBoundaryLayouts: boolean;
};
type InteractionSlice = {
  selectedId?: string;
  gestureSnapshot?: GestureSnapshot;
  edgeInsertRequest?: StudioEdge;
  edgeReconnectRequest?: string;
};
type HistorySlice = { past: HistoryPatch[]; future: HistoryPatch[] };
type EditTargets = { nodeIds?: string[]; annotationIds?: string[]; boundary?: "start" };

export type EditorState = DocumentSlice &
  InteractionSlice &
  HistorySlice & {
    hydrate: (value: StudioDocument) => void;
    applyEditorCommand: (commandId: string, expectedRevision: number, document: StudioDocument) => "applied" | "already_applied" | "stale" | "read_only";
    onNodesChange: (changes: NodeChange<StudioNode>[]) => void;
    onEdgesChange: (changes: EdgeChange<StudioEdge>[]) => void;
    removeSelectedEdges: () => void;
    removeEdge: (id: string) => void;
    requestEdgeInsert: (id: string) => void;
    clearEdgeInsertRequest: () => void;
    requestEdgeReconnect: (id: string) => void;
    clearEdgeReconnectRequest: () => void;
    connect: (
      connection: Connection,
      data: StudioEdge["data"],
      replaceEdgeId?: string,
    ) => void;
    reconnectEdge: (edgeId: string, connection: Connection) => void;
    addAction: (
      data: ActionNodeData,
      position?: { x: number; y: number },
    ) => string;
    addConnectedAction: (
      data: ActionNodeData,
      source: { nodeId: string; handleId: string; targetHandle: string },
      position: { x: number; y: number },
    ) => string;
    insertActionOnEdge: (
      data: ActionNodeData,
      edgeId: string,
      handles: { input: string; output: string },
      position: { x: number; y: number },
    ) => string | undefined;
    addExit: (position?: { x: number; y: number }) => string;
    addAnnotation: (position?: { x: number; y: number }, text?: string) => void;
    updateAnnotation: (
      id: string,
      patch: Partial<StudioDocument["annotations"][number]>,
    ) => void;
    updateAnnotationFrame: (
      id: string,
      patch: Partial<
        Pick<
          StudioDocument["annotations"][number],
          "x" | "y" | "width" | "height"
        >
      >,
    ) => void;
    removeAnnotation: (id: string) => void;
    addGroup: (label?: string) => void;
    toggleGroup: (id: string) => void;
    removeGroup: (id: string) => void;
    moveGroup: (id: string, delta: { x: number; y: number }) => void;
    select: (id?: string) => void;
    updateNode: (
      id: string,
      data: Partial<ActionNodeData> | Partial<ExitNodeData>,
    ) => void;
    updateLoopFrame: (id: string, frame: { x: number; y: number; width: number; height: number }) => void;
    setStart: (start: StudioDocument["start"] | ((current: StudioDocument["start"]) => StudioDocument["start"])) => void;
    setEnd: (end: StudioDocument["end"] | ((current: StudioDocument["end"]) => StudioDocument["end"])) => void;
    updateBoundaryPosition: (boundary: "start", position: { x: number; y: number }) => void;
    detachFromContainer: (nodeId: string) => void;
    removeSelected: () => void;
    setViewport: (viewport: Viewport) => void;
    replaceNodes: (nodes: StudioNode[]) => void;
    alignSelected: (direction: "left" | "top") => void;
    beginEdit: (targets?: EditTargets) => void;
    commitEdit: () => void;
    paste: (nodes: StudioNode[], edges: StudioEdge[]) => void;
    markSaved: (expectedDocumentRevision: number) => boolean;
    undo: () => void;
    redo: () => void;
  };

type DocumentState = Pick<
  StudioDocument,
  "start" | "nodes" | "edges" | "end" | "boundaryLayouts" | "annotations" | "groups" | "settings"
>;

const documentOf = (
  state: Pick<
    EditorState,
    "start" | "nodes" | "edges" | "end" | "boundaryLayouts" | "annotations" | "groups" | "settings"
  >,
): DocumentState => ({
  start: state.start,
  nodes: state.nodes,
  edges: state.edges,
  end: state.end,
  boundaryLayouts: state.boundaryLayouts,
  annotations: state.annotations,
  groups: state.groups,
  settings: state.settings,
});
const commit = (state: EditorState, next: Partial<EditorState>) => {
  const before = documentOf(state);
  const after = documentOf({ ...state, ...next });
  const patch = createHistoryPatch(before, after);
  return patch
    ? {
        ...next,
        past: [...state.past.slice(-49), patch],
        future: [],
        dirty: true,
      }
    : next;
};

const documentSlice = (): DocumentSlice => ({
  start: {
    inputs: { type: "object", properties: {}, additionalProperties: false },
    contexts: {},
  },
  nodes: [],
  edges: [],
  end: { completion: "first_return", outputs: {}, error: { outputs: { } } },
  boundaryLayouts: [{ boundary: "start", x: 40, y: 220 }],
  viewport: { x: 0, y: 0, zoom: 1 },
  annotations: [],
  groups: [],
  settings: { executionOrder: "deterministic", activationBudget: 10_000 },
  dirty: false,
  graphRevision: 0,
  localDocumentRevision: 0,
  readOnly: false,
  appliedCommands: {},
});
const interactionSlice = (): InteractionSlice => ({
  selectedId: undefined,
  gestureSnapshot: undefined,
  edgeInsertRequest: undefined,
  edgeReconnectRequest: undefined,
});
const historySlice = (): HistorySlice => ({ past: [], future: [] });

const nodeParentId = (node: StudioNode) =>
  node.data.editorKind === "action" ? node.data.parentId : undefined;

/** Size estimate for a freshly dropped child before React Flow measures it. */
const CHILD_SIZE_ESTIMATE = { width: 240, height: 140 };

/** Keeps the container frame large enough to hold its body children. */
function growContainers(nodes: StudioNode[]): StudioNode[] {
  const loops = new Map<string, StudioNode>();
  for (const node of nodes)
    if (node.data.editorKind === "action" && node.data.nodeType === "loop_over_items")
      loops.set(node.id, node);
  if (!loops.size) return nodes;
  const required = new Map<string, { width: number; height: number }>();
  for (const node of nodes) {
    const parentId = nodeParentId(node);
    if (!parentId || !loops.get(parentId)) continue;
    const width = node.width ?? node.measured?.width ?? CHILD_SIZE_ESTIMATE.width;
    const height = node.height ?? node.measured?.height ?? CHILD_SIZE_ESTIMATE.height;
    const bounds = required.get(parentId) ?? { width: 0, height: 0 };
    bounds.width = Math.max(bounds.width, node.position.x + width);
    bounds.height = Math.max(bounds.height, node.position.y + height);
    required.set(parentId, bounds);
  }
  let changed = false;
  const grown = nodes.map((node) => {
    const bounds = required.get(node.id);
    if (!bounds || !loops.has(node.id)) return node;
    const width = Math.max(node.width ?? LOOP_CONTAINER_MIN_WIDTH, bounds.width + LOOP_CONTAINER_PADDING.right, LOOP_CONTAINER_MIN_WIDTH);
    const height = Math.max(node.height ?? LOOP_CONTAINER_MIN_HEIGHT, bounds.height + LOOP_CONTAINER_PADDING.bottom, LOOP_CONTAINER_MIN_HEIGHT);
    if (width === node.width && height === node.height) return node;
    changed = true;
    return { ...node, width, height };
  });
  return changed ? grown : nodes;
}

/**
 * Children of deleted loop nodes lose their membership but keep their absolute
 * position (store positions of parented nodes are container-relative).
 */
function releaseContainerChildren(
  nodes: StudioNode[],
  loopOrigins: Map<string, { x: number; y: number }>,
): StudioNode[] {
  if (!loopOrigins.size) return nodes;
  return nodes.map((node) => {
    const parentId = nodeParentId(node);
    const origin = parentId ? loopOrigins.get(parentId) : undefined;
    if (!origin || node.data.editorKind !== "action") return node;
    return {
      ...node,
      position: { x: node.position.x + origin.x, y: node.position.y + origin.y },
      data: { ...node.data, parentId: undefined },
    };
  });
}

function loopOriginsOf(nodes: StudioNode[], removedIds: Set<string>) {
  const origins = new Map<string, { x: number; y: number }>();
  for (const node of nodes)
    if (removedIds.has(node.id) && node.data.editorKind === "action" && node.data.nodeType === "loop_over_items")
      origins.set(node.id, node.position);
  return origins;
}

function matchesDynamicPortNode(data: ActionNodeData) {
  return data.nodeType === "if" || data.nodeType === "approval" || data.nodeType === "merge";
}

function isDynamicOutputHandle(handle: string | null | undefined) {
  return Boolean(handle?.startsWith("case:") || handle?.startsWith("decision:"));
}

function dynamicOutputHandles(data: ActionNodeData) {
  if (data.nodeType === "if") {
    const cases = Array.isArray(data.parameters.cases) ? data.parameters.cases : [];
    return new Set(cases.flatMap((branch) => branch && typeof branch === "object" && typeof (branch as { id?: unknown }).id === "string" ? [`case:${(branch as { id: string }).id}`] : []));
  }
  const configured = Array.isArray(data.parameters.buttons) ? data.parameters.buttons : [];
  const buttons = configured.length ? configured : [{ id: "approved" }, { id: "rejected" }];
  return new Set(buttons.flatMap((button) => button && typeof button === "object" && typeof (button as { id?: unknown }).id === "string" ? [`decision:${(button as { id: string }).id}`] : []));
}

function mergeInputHandleValid(data: ActionNodeData, handle: string | null | undefined) {
  if (data.nodeType !== "merge") return true;
  const mode = data.parameters.mode ?? "append";
  return mode === "append"
    ? handle === "main" || Boolean(handle?.startsWith("main:"))
    : handle === "left" || handle === "right";
}

export function createEditorStore(readOnly = false) {
  return createStore<EditorState>((rawSet, get) => {
    const set: typeof rawSet = (update, replace) => {
      rawSet((state) => {
        const next = typeof update === "function" ? update(state) : update;
        const candidate = { ...state, ...next };
        if (state.readOnly) {
          const graphChanged = persistedDocumentChanged(state, candidate, true);
          if (graphChanged) return state;
          return {...next, dirty: false};
        }
        const changed = persistedDocumentChanged(state, candidate);
        return changed ? { ...next, localDocumentRevision: state.localDocumentRevision + 1 } : next;
      }, replace as false);
    };
    return ({
  ...documentSlice(),
  ...interactionSlice(),
  ...historySlice(),
  readOnly,
  applyEditorCommand: (commandId, expectedRevision, document) => {
    const state = get();
    if (state.readOnly) return "read_only";
    if (commandId in state.appliedCommands) return "already_applied";
    if (state.localDocumentRevision !== expectedRevision || state.gestureSnapshot) return "stale";
    const protectedIds = state.nodes.filter(node => node.data.editorKind === "exit" && node.data.protected).map(node => node.id);
    if (protectedIds.some(id => !document.nodes.some(node => node.id === id && node.data.editorKind === "exit"))) return "read_only";
    set(current => ({ ...commit(current, document), graphRevision: current.graphRevision + 1, appliedCommands: { ...current.appliedCommands, [commandId]: current.localDocumentRevision + 1 } }));
    return "applied";
  },
  hydrate: (value) =>
    rawSet((state) => ({
      ...value,
      dirty: false,
      selectedId: undefined,
      graphRevision: state.graphRevision + 1,
      localDocumentRevision: state.localDocumentRevision + 1,
      past: [],
      future: [],
      gestureSnapshot: undefined,
    })),
  onNodesChange: (changes) =>
    set((state) => {
      const protectedExits = new Set(
        state.nodes
          .filter(
            (node) =>
              node.data.editorKind === "exit" &&
              (node.data as ExitNodeData).protected,
          )
          .map((node) => node.id),
      );
      const guarded = changes.filter(
        (change) => !(change.type === "remove" && protectedExits.has(change.id)),
      );
      const structural = guarded.some(
        (change) =>
          change.type === "remove" ||
          change.type === "add" ||
          change.type === "replace",
      );
      const documentChange = guarded.some(
        (change) => change.type !== "select" && change.type !== "dimensions",
      );
      const resizedLoopFrames = new Map(
        guarded.flatMap((change) => {
          if (change.type !== "dimensions" || !state.gestureSnapshot) return [];
          const node = state.nodes.find((candidate) => candidate.id === change.id);
          if (node?.data.editorKind !== "action" || node.data.nodeType !== "loop_over_items") return [];
          return [[change.id, change.dimensions] as const];
        }),
      );
      const removedIds = new Set(
        guarded
          .filter((change) => change.type === "remove")
          .map((change) => change.id),
      );
      const loopOrigins = loopOriginsOf(state.nodes, removedIds);
      const nodes = releaseContainerChildren(
        applyNodeChanges(guarded, state.nodes).map((node) => {
          const dimensions = resizedLoopFrames.get(node.id);
          return dimensions ? { ...node, width: dimensions.width, height: dimensions.height } : node;
        }),
        loopOrigins,
      );
      return structural
        ? {
            ...commit(state, { nodes }),
            graphRevision: state.graphRevision + 1,
          }
        : { nodes, dirty: state.dirty || documentChange || resizedLoopFrames.size > 0 };
    }),
  onEdgesChange: (changes) =>
    set((state) => {
      const structural = changes.some((change) => change.type !== "select");
      const edges = applyEdgeChanges(changes, state.edges);
      return structural
        ? {
            ...commit(state, { edges }),
            graphRevision: state.graphRevision + 1,
          }
        : { edges };
    }),
  removeSelectedEdges: () =>
    set((state) => {
      if (!state.edges.some((edge) => edge.selected)) return state;
      return {
        ...commit(state, {
          edges: state.edges.filter((edge) => !edge.selected),
        }),
        graphRevision: state.graphRevision + 1,
      };
    }),
  removeEdge: (id) =>
    set((state) =>
      state.edges.some((edge) => edge.id === id)
        ? {
            ...commit(state, {
              edges: state.edges.filter((edge) => edge.id !== id),
            }),
            graphRevision: state.graphRevision + 1,
          }
        : state,
    ),
  requestEdgeInsert: (id) =>
    set((state) => ({
      edgeInsertRequest: state.edges.find((edge) => edge.id === id),
    })),
  requestEdgeReconnect: (id) =>
    set((state) => ({
      edgeReconnectRequest: id,
      edges: state.edges.map((edge) => ({ ...edge, selected: edge.id === id })),
    })),
  clearEdgeReconnectRequest: () => set({ edgeReconnectRequest: undefined }),
  clearEdgeInsertRequest: () => set({ edgeInsertRequest: undefined }),
  connect: (rawConnection, data, replaceEdgeId) =>
    set((state) => {
      // Chip handles are edit-only anchors: loop→body edges live on the loop node.
      const chipSource =
        rawConnection.source && isIterationChipId(rawConnection.source)
          ? loopIdOfIterationChip(rawConnection.source)
          : undefined;
      const connection = chipSource
        ? { ...rawConnection, source: chipSource, sourceHandle: "main" as string | null }
        : rawConnection;
      const sourceId = connection.source ?? "";
      const nodes =
        data?.sourcePortKind === "error"
          ? state.nodes.map((node) =>
              node.id === sourceId && node.data.editorKind === "action"
                ? {
                    ...node,
                    data: {
                      ...node.data,
                      settings: {
                        ...node.data.settings,
                      },
                    },
                  }
                : node,
            )
          : state.nodes;
      return {
        ...commit(state, {
          nodes,
          edges: addEdge(
            { ...connection, id: crypto.randomUUID(), type: "studio", data },
            replaceEdgeId
              ? state.edges.filter((edge) => edge.id !== replaceEdgeId)
              : state.edges,
          ),
        }),
        graphRevision: state.graphRevision + 1,
      };
    }),
  reconnectEdge: (edgeId, rawConnection) =>
    set((state) => {
      const edge = state.edges.find((candidate) => candidate.id === edgeId);
      if (!edge || !rawConnection.source || !rawConnection.target) return state;
      const connection =
        isIterationChipId(rawConnection.source)
          ? {
              ...rawConnection,
              source: loopIdOfIterationChip(rawConnection.source),
              sourceHandle: "main" as string | null,
            }
          : rawConnection;
      const sourcePortKind = connection.sourceHandle === "error" ? "error" : "main";
      const nodes = sourcePortKind === "error"
        ? state.nodes.map((node) => node.id === connection.source && node.data.editorKind === "action" ? { ...node, data: { ...node.data, settings: { ...node.data.settings } } } : node)
        : state.nodes;
      return {
        ...commit(state, {
          nodes,
          graphRevision: state.graphRevision + 1,
          edges: state.edges.map((candidate) =>
            candidate.id === edgeId
              ? {
                  ...candidate,
                  source: connection.source!,
                  sourceHandle: connection.sourceHandle,
                  target: connection.target!,
                  targetHandle: connection.targetHandle,
                  data: {
                    ...candidate.data,
                    edgeKind: candidate.data?.edgeKind ?? "execution",
                    sourcePortKind,
                  },
                }
              : candidate,
          ),
        }),
      };
    }),
  addAction: (data, position) => {
    const id = crypto.randomUUID();
    set((state) => {
      const count = state.nodes.filter(
        (node) => node.data.editorKind === "action",
      ).length;
      const nextPosition = position ?? {
        x: 60 + (count % 3) * 250,
        y: 60 + Math.floor(count / 3) * 170,
      };
      return {
        ...commit(state, {
          nodes: growContainers([
            ...state.nodes.map((node) => ({ ...node, selected: false })),
            {
              id,
              type: "manifest",
              position: nextPosition,
              data,
              selected: true,
            },
          ]),
        }),
        graphRevision: state.graphRevision + 1,
        selectedId: id,
      };
    });
    return id;
  },
  addConnectedAction: (data, source, position) => {
    const id = crypto.randomUUID();
    set((state) => {
      const order = state.edges.filter(
        (edge) =>
          edge.source === source.nodeId &&
          edge.sourceHandle === source.handleId &&
          edge.data?.edgeKind === "execution",
      ).length;
      const sourcePortKind = source.handleId === "error" ? "error" : "main";
      const edge: StudioEdge = {
        id: crypto.randomUUID(),
        source: source.nodeId,
        sourceHandle: source.handleId,
        target: id,
        targetHandle: source.targetHandle,
        type: "studio",
        data: { edgeKind: "execution", order, sourcePortKind },
      };
      const sourceNodes =
        sourcePortKind === "error"
          ? state.nodes.map((node) =>
              node.id === source.nodeId && node.data.editorKind === "action"
                ? {
                    ...node,
                    data: {
                      ...node.data,
                      settings: {
                        ...node.data.settings,
                      },
                    },
                  }
                : node,
            )
          : state.nodes;
      return {
        ...commit(state, {
          selectedId: id,
          nodes: growContainers([
            ...sourceNodes.map((node) => ({ ...node, selected: false })),
            { id, type: "manifest", position, data, selected: true },
          ]),
          edges: [...state.edges, edge],
        }),
        graphRevision: state.graphRevision + 1,
      };
    });
    return id;
  },
  insertActionOnEdge: (data, edgeId, handles, position) => {
    const id = crypto.randomUUID();
    let inserted = false;
    set((state) => {
      const replaced = state.edges.find(
        (edge) => edge.id === edgeId && edge.data?.edgeKind === "execution",
      );
      if (!replaced) return state;
      inserted = true;
      const incoming: StudioEdge = {
        id: crypto.randomUUID(),
        source: replaced.source,
        sourceHandle: replaced.sourceHandle,
        target: id,
        targetHandle: handles.input,
        type: "studio",
        data: {
          edgeKind: "execution",
          order: replaced.data?.order ?? 0,
          sourcePortKind: replaced.data?.sourcePortKind ?? "main",
        },
      };
      const outgoing: StudioEdge = {
        id: crypto.randomUUID(),
        source: id,
        sourceHandle: handles.output,
        target: replaced.target,
        targetHandle: replaced.targetHandle,
        type: "studio",
        data: { edgeKind: "execution", order: 0, sourcePortKind: "main" },
      };
      return {
        ...commit(state, {
          edgeInsertRequest: undefined,
          selectedId: id,
          nodes: [
            ...state.nodes.map((node) => ({ ...node, selected: false })),
            { id, type: "manifest", position, data, selected: true },
          ],
          edges: [
            ...state.edges.filter((edge) => edge.id !== edgeId),
            incoming,
            outgoing,
          ],
        }),
        graphRevision: state.graphRevision + 1,
      };
    });
    return inserted ? id : undefined;
  },
  addExit: (position) => {
    const id = crypto.randomUUID();
    set((state) => {
      const count = state.nodes.filter(
        (node) => node.data.editorKind === "exit",
      ).length;
      const suffix = count === 0 ? "" : ` ${count + 1}`;
      const nextPosition = position ?? { x: 560, y: 120 + count * 140 };
      const data: ExitNodeData = {
        editorKind: "exit",
        key: `exit${suffix ? `_${count + 1}` : ""}`,
        label: `End${suffix}`,
        protected: false,
        parameters: { outputs: {}, errorOutputs: {} },
      };
      return {
        ...commit(state, {
          nodes: [
            ...state.nodes.map((node) => ({ ...node, selected: false })),
            { id, type: "exit", position: nextPosition, data, selected: true },
          ],
        }),
        graphRevision: state.graphRevision + 1,
        selectedId: id,
      };
    });
    return id;
  },
  addAnnotation: (position, text = "New note") =>
    set((state) => {
      const id = crypto.randomUUID();
      const next = position ?? { x: 120, y: 120 };
      return commit(state, {
        annotations: [
          ...state.annotations,
          { id, text, x: next.x, y: next.y, width: 240, height: 160 },
        ],
      });
    }),
  updateAnnotation: (id, patch) =>
    set((state) =>
      commit(state, {
        annotations: state.annotations.map((annotation) =>
          annotation.id === id ? { ...annotation, ...patch } : annotation,
        ),
      }),
    ),
  updateAnnotationFrame: (id, patch) =>
    set((state) => ({
      annotations: state.annotations.map((annotation) =>
        annotation.id === id ? { ...annotation, ...patch } : annotation,
      ),
      dirty: true,
    })),
  removeAnnotation: (id) =>
    set((state) =>
      commit(state, {
        annotations: state.annotations.filter(
          (annotation) => annotation.id !== id,
        ),
      }),
    ),
  addGroup: (label = "Group") =>
    set((state) => {
      const grouped = new Set(state.groups.flatMap((group) => group.nodeIds));
      const nodeIds = state.nodes
        .filter(
          (node) =>
            (node.selected || node.id === state.selectedId) &&
            !grouped.has(node.id),
        )
        .map((node) => node.id);
      if (nodeIds.length < 2) return state;
      return commit(state, {
        groups: [
          ...state.groups,
          { id: crypto.randomUUID(), label, nodeIds, collapsed: false },
        ],
      });
    }),
  toggleGroup: (id) =>
    set((state) =>
      commit(state, {
        groups: state.groups.map((group) =>
          group.id === id ? { ...group, collapsed: !group.collapsed } : group,
        ),
      }),
    ),
  removeGroup: (id) =>
    set((state) =>
      commit(state, {
        groups: state.groups.filter((group) => group.id !== id),
      }),
    ),
  moveGroup: (id, delta) =>
    set((state) => {
      const memberIds = new Set(
        state.groups.find((group) => group.id === id)?.nodeIds ?? [],
      );
      if (!memberIds.size || (!delta.x && !delta.y)) return state;
      return {
        nodes: state.nodes.map((node) =>
          memberIds.has(node.id)
            ? {
                ...node,
                position: {
                  x: node.position.x + delta.x,
                  y: node.position.y + delta.y,
                },
              }
            : node,
        ),
        dirty: true,
      };
    }),
  select: (selectedId) => set({ selectedId }),
  updateNode: (id, data) =>
    set((state) => {
      const current = state.nodes.find((node) => node.id === id);
      const mergedData = current?.data.editorKind === "action" && "parameters" in data && data.parameters
        ? { ...data, parameters: { ...current.data.parameters, ...data.parameters } }
        : data;
      const nextData = current?.data.editorKind === "action"
        ? ({ ...current.data, ...mergedData } as ActionNodeData)
        : undefined;
      const dynamicPortsChanged = Boolean(nextData && "parameters" in data && matchesDynamicPortNode(nextData));
      const graphChanged =
        "nodeType" in data ||
        "typeVersion" in data ||
        "resourceReferences" in data ||
        "label" in data ||
        dynamicPortsChanged;
      const nextNodes = state.nodes.map((node) => {
        if (node.id === id)
          return {
            ...node,
                data: { ...node.data, ...mergedData } as typeof node.data,
          };
        return node;
      });
      const validHandles = nextData && dynamicPortsChanged && nextData.nodeType !== "merge" ? dynamicOutputHandles(nextData) : undefined;
      const nextEdges = state.edges.filter((edge) => {
        if (validHandles && edge.source === id && isDynamicOutputHandle(edge.sourceHandle) && !validHandles.has(edge.sourceHandle!)) return false;
        if (nextData && dynamicPortsChanged && edge.target === id && !mergeInputHandleValid(nextData, edge.targetHandle)) return false;
        return true;
      });
      return {
        ...commit(state, {
          graphRevision: state.graphRevision + Number(graphChanged),
          nodes: nextNodes,
          edges: nextEdges,
        }),
      };
    }),
  updateLoopFrame: (id, frame) =>
    set((state) => {
      const current = state.nodes.find((node) => node.id === id);
      if (current?.data.editorKind !== "action" || current.data.nodeType !== "loop_over_items") return state;
      const nodes = state.nodes.map((node) => node.id === id ? {
        ...node,
        position: { x: frame.x, y: frame.y },
        width: frame.width,
        height: frame.height,
      } : node);
      return { nodes, dirty: true };
    }),
  setStart: (start) => set((state) => commit(state, { start: typeof start === "function" ? start(state.start) : start })),
  setEnd: (end) => set((state) => commit(state, { end: typeof end === "function" ? end(state.end) : end })),
  updateBoundaryPosition: (boundary, position) =>
    set((state) => {
      const current = state.boundaryLayouts.find((layout) => layout.boundary === boundary);
      if (current?.x === position.x && current.y === position.y) return state;
      const boundaryLayouts = current
        ? state.boundaryLayouts.map((layout) => layout.boundary === boundary ? { ...layout, ...position } : layout)
        : [...state.boundaryLayouts, { boundary, ...position }];
      return state.gestureSnapshot
        ? { boundaryLayouts, dirty: true }
        : commit(state, { boundaryLayouts });
    }),
  detachFromContainer: (nodeId) =>
    set((state) => {
      const node = state.nodes.find((candidate) => candidate.id === nodeId);
      const parentId = node && nodeParentId(node);
      const parent = parentId
        ? state.nodes.find((candidate) => candidate.id === parentId)
        : undefined;
      if (!node || !parent) return state;
      const nodes = state.nodes.map((candidate) =>
        candidate.id === nodeId && candidate.data.editorKind === "action"
          ? {
              ...candidate,
              position: {
                x: parent.position.x + candidate.position.x,
                y: parent.position.y + candidate.position.y,
              },
              data: { ...candidate.data, parentId: undefined },
            }
          : candidate,
      );
      return state.gestureSnapshot
        ? { nodes, dirty: true }
        : commit(state, { nodes });
    }),
  removeSelected: () =>
    set((state) => {
      if (state.selectedId?.startsWith("annotation:")) {
        const annotationId = state.selectedId.slice("annotation:".length);
        return commit(state, {
          annotations: state.annotations.filter(
            (annotation) => annotation.id !== annotationId,
          ),
          selectedId: undefined,
        });
      }
      if (state.selectedId?.startsWith("group:")) {
        const groupId = state.selectedId.slice("group:".length);
        return commit(state, {
          groups: state.groups.filter((group) => group.id !== groupId),
          selectedId: undefined,
        });
      }
      const ids = new Set(
        state.nodes.filter((node) => node.selected).map((node) => node.id),
      );
      if (state.selectedId) ids.add(state.selectedId);
      for (const node of state.nodes) {
        if (node.data.editorKind === "exit" && (node.data as ExitNodeData).protected) {
          ids.delete(node.id);
        }
      }
      const loopOrigins = loopOriginsOf(state.nodes, ids);
      return ids.size
        ? {
            ...commit(state, {
              nodes: releaseContainerChildren(
                state.nodes.filter((node) => !ids.has(node.id)),
                loopOrigins,
              ),
              edges: state.edges.filter(
                (edge) => !ids.has(edge.source) && !ids.has(edge.target),
              ),
              groups: state.groups
                .map((group) => ({
                  ...group,
                  nodeIds: group.nodeIds.filter((id) => !ids.has(id)),
                }))
                .filter((group) => group.nodeIds.length),
              selectedId: undefined,
            }),
            graphRevision: state.graphRevision + 1,
          }
        : state;
    }),
  setViewport: (viewport) => set({ viewport, dirty: true }),
  replaceNodes: (nodes) =>
    set((state) => ({
      ...commit(state, { nodes }),
      graphRevision: state.graphRevision + 1,
    })),
  alignSelected: (direction) =>
    set((state) => {
      // Container children use parent-relative positions; they never align with free nodes.
      const selected = state.nodes.filter(
        (node) =>
          (node.selected || node.id === state.selectedId) && !nodeParentId(node),
      );
      if (selected.length < 2) return state;
      const value = Math.min(
        ...selected.map((node) =>
          direction === "left" ? node.position.x : node.position.y,
        ),
      );
      const selectedIds = new Set(selected.map((node) => node.id));
      return commit(state, {
        nodes: state.nodes.map((node) =>
          selectedIds.has(node.id)
            ? {
                ...node,
                position:
                  direction === "left"
                    ? { ...node.position, x: value }
                    : { ...node.position, y: value },
              }
            : node,
        ),
      });
    }),
  beginEdit: (targets) =>
    set((state) => {
      if (state.gestureSnapshot) return state;
      const defaultNodeIds = state.nodes
        .filter((node) => node.selected || node.id === state.selectedId)
        .map((node) => node.id);
      const nodeIds = new Set(targets?.nodeIds ?? defaultNodeIds);
      const annotationIds = new Set(targets?.annotationIds ?? []);
      return {
        gestureSnapshot: {
          nodes: state.nodes.filter((node) => nodeIds.has(node.id)),
          annotations: state.annotations.filter((annotation) =>
            annotationIds.has(annotation.id),
          ),
          boundaryLayouts: state.boundaryLayouts,
          includeBoundaryLayouts: Boolean(targets?.boundary),
        },
      };
    }),
  commitEdit: () =>
    set((state) => {
      if (!state.gestureSnapshot) return state;
      const previousNodes = new Map(
        state.gestureSnapshot.nodes.map((node) => [node.id, node]),
      );
      const previousAnnotations = new Map(
        state.gestureSnapshot.annotations.map((annotation) => [
          annotation.id,
          annotation,
        ]),
      );
      const before = documentOf({
        ...state,
        nodes: state.nodes.map((node) => previousNodes.get(node.id) ?? node),
        annotations: state.annotations.map(
          (annotation) => previousAnnotations.get(annotation.id) ?? annotation,
        ),
        boundaryLayouts: state.gestureSnapshot.includeBoundaryLayouts
          ? state.gestureSnapshot.boundaryLayouts
          : state.boundaryLayouts,
      });
      const patch = createHistoryPatch(before, documentOf(state));
      return patch
        ? {
            past: [...state.past.slice(-49), patch],
            future: [],
            dirty: true,
            gestureSnapshot: undefined,
          }
        : { gestureSnapshot: undefined };
    }),
  paste: (nodes, edges) =>
    set((state) => ({
      ...commit(state, {
        nodes: growContainers([
          ...state.nodes.map((node) => ({ ...node, selected: false })),
          ...nodes,
        ]),
        edges: [
          ...state.edges.map((edge) => ({ ...edge, selected: false })),
          ...edges,
        ],
        selectedId: nodes.length === 1 ? nodes[0].id : undefined,
      }),
      graphRevision: state.graphRevision + 1,
    })),
  markSaved: (expectedDocumentRevision) => {
    if (get().localDocumentRevision !== expectedDocumentRevision) return false;
    set({ dirty: false });
    return true;
  },
  undo: () =>
    set((state) => {
      const patch = state.past.at(-1);
      if (!patch) return state;
      return {
        ...applyHistoryPatch(documentOf(state), patch, "undo"),
        selectedId: undefined,
        dirty: true,
        graphRevision: state.graphRevision + Number(patch.graphChanged),
        past: state.past.slice(0, -1),
        future: [patch, ...state.future],
        gestureSnapshot: undefined,
      };
    }),
  redo: () =>
    set((state) => {
      const patch = state.future[0];
      if (!patch) return state;
      return {
        ...applyHistoryPatch(documentOf(state), patch, "redo"),
        selectedId: undefined,
        dirty: true,
        graphRevision: state.graphRevision + Number(patch.graphChanged),
        past: [...state.past, patch],
        future: state.future.slice(1),
        gestureSnapshot: undefined,
      };
    }),
});
  });
}

// Pointer movement changes only layout. Compare those fields directly, while
// retaining the canonical serializer comparison for semantic document edits.
function persistedDocumentChanged(before: EditorState, after: EditorState, ignoreViewport = false): boolean {
  const slices = ['start', 'end', 'settings', 'annotations', 'groups', 'boundaryLayouts', 'edges'] as const;
  const semanticChange = slices.some(key => before[key] !== after[key]);
  if (!semanticChange && before.viewport === after.viewport && before.nodes === after.nodes) return false;
  const sameViewport = ignoreViewport || before.viewport === after.viewport;
  if (!semanticChange && sameViewport && before.nodes.length === after.nodes.length) {
    let layoutChanged = false;
    for (let index = 0; index < before.nodes.length; index += 1) {
      const previous = before.nodes[index]!;
      const next = after.nodes[index]!;
      if (previous.id !== next.id || previous.data !== next.data) return canonicalChange(before, after, ignoreViewport);
      if (isIterationChipId(previous.id)) continue;
      layoutChanged ||= previous.position.x !== next.position.x || previous.position.y !== next.position.y
        || (previous.editorCollapsed ?? false) !== (next.editorCollapsed ?? false)
        || (previous.width ?? previous.measured?.width) !== (next.width ?? next.measured?.width)
        || (previous.height ?? previous.measured?.height) !== (next.height ?? next.measured?.height);
    }
    return layoutChanged;
  }
  return canonicalChange(before, after, ignoreViewport);
}

function canonicalChange(before: EditorState, after: EditorState, ignoreViewport: boolean): boolean {
  const previous = serializeStudio(before);
  const next = serializeStudio(after);
  if (ignoreViewport) return JSON.stringify({...previous, editorDocument: {...previous.editorDocument, viewport: null}})
    !== JSON.stringify({...next, editorDocument: {...next.editorDocument, viewport: null}});
  return JSON.stringify(previous) !== JSON.stringify(next);
}

const EditorStoreContext = createContext<StoreApi<EditorState> | null>(null);
export function EditorStoreProvider({ children, readOnly = false, store }: { children: ReactNode; readOnly?: boolean; store?: StoreApi<EditorState> }) {
  const [instance] = useState(() => store ?? createEditorStore(readOnly));
  return createElement(EditorStoreContext.Provider, { value: instance }, createElement(CanvasRenderProvider, null, children));
}
export function useEditorStoreApi() {
  const store = useContext(EditorStoreContext);
  if (!store) throw new Error("An editor store provider is required");
  return store;
}
export function useEditorStore<T>(selector: (state: EditorState) => T): T {
  return useStore(useEditorStoreApi(), selector);
}
