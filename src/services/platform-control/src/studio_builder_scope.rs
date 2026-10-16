use agentx_runtime_contracts::{StudioDocumentV1, StudioEditScopeV1};
use serde_json::json;

use crate::api_error::{ApiError, ApiResult};

pub(crate) fn check(
    scope: &StudioEditScopeV1,
    before: &StudioDocumentV1,
    after: &StudioDocumentV1,
    changed: &[String],
) -> ApiResult<()> {
    let StudioEditScopeV1::Nodes { node_ids } = scope else {
        return Ok(());
    };
    let unrelated_edges = |document: &StudioDocumentV1| {
        document
            .definition
            .connections
            .iter()
            .filter(|edge| {
                !node_ids.contains(&edge.source_node_id) && !node_ids.contains(&edge.target_node_id)
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let unrelated_layouts = |document: &StudioDocumentV1| {
        document
            .editor_document
            .node_layouts
            .iter()
            .filter(|layout| !node_ids.contains(&layout.node_id))
            .cloned()
            .collect::<Vec<_>>()
    };
    let contracts = |document: &StudioDocumentV1| json!({"start":document.definition.start,"end":document.definition.end,"settings":document.definition.settings,"viewport":document.editor_document.viewport,"boundaries":document.editor_document.boundary_layouts,"groups":document.editor_document.groups,"annotations":document.editor_document.annotations});
    if changed.iter().any(|id| !node_ids.contains(id))
        || contracts(before) != contracts(after)
        || json!(unrelated_edges(before)) != json!(unrelated_edges(after))
        || json!(unrelated_layouts(before)) != json!(unrelated_layouts(after))
    {
        return Err(ApiError::conflict(
            "BUILDER_EDIT_OUTSIDE_SCOPE",
            "This run can modify only the nodes selected when it started. Ask the user to start a canvas-scoped run for a larger change.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_scope_preserves_unrelated_nodes_and_global_contracts() {
        let before = StudioDocumentV1 {
            definition: agentx_domain::WorkflowDefinition::empty(),
            editor_document: agentx_domain::EditorDocument::default(),
        };
        let scope = StudioEditScopeV1::Nodes {
            node_ids: vec![before.definition.nodes[0].id.clone()],
        };
        let mut after = before.clone();
        after.definition.nodes[0].name = "Renamed".into();
        assert!(
            check(
                &scope,
                &before,
                &after,
                &[after.definition.nodes[0].id.clone()]
            )
            .is_ok()
        );
        assert!(check(&scope, &before, &after, &["other".into()]).is_err());
        after.definition.start.inputs =
            json!({"type":"object","properties":{"added":{"type":"string"}}});
        assert!(check(&scope, &before, &after, &[]).is_err());
        assert!(check(&StudioEditScopeV1::Canvas, &before, &after, &[]).is_ok());
    }
}
