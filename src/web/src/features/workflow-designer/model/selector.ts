import type { ValueSelection, ValueSelector } from "./types";

/** Workflow 8.0 wire defaults; interpreting a selector must not mutate the saved document/hash. */
export function selectorWithDefaults(selector: ValueSelector) {
  return {
    ...selector,
    port: selector.port ?? (selector.namespace === "outputs" ? "main" : undefined),
    run: selector.run ?? { kind: "current" } as ValueSelection,
    item: selector.item ?? { kind: "current" } as ValueSelection,
    path: selector.path ?? [],
  };
}

const selectionEqual = (left: ValueSelection, right: ValueSelection) =>
  left.kind === right.kind && (left.kind !== "index" || right.kind === "index" && left.index === right.index);

export function selectorsEqual(leftValue: ValueSelector, rightValue: ValueSelector) {
  const left = selectorWithDefaults(leftValue);
  const right = selectorWithDefaults(rightValue);
  return left.namespace === right.namespace
    && left.sourceNodeId === right.sourceNodeId
    && left.port === right.port
    && selectionEqual(left.run, right.run)
    && selectionEqual(left.item, right.item)
    && left.path.length === right.path.length
    && left.path.every((segment, index) => segment === right.path[index]);
}
