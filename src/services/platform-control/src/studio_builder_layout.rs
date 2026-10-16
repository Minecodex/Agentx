use agentx_runtime_contracts::{StudioDocumentV1, StudioEditScopeV1, StudioEditorCommandV1};
use serde_json::json;

use crate::api_error::{ApiError, ApiResult};

pub(crate) fn needs_layout(
    before: &StudioDocumentV1,
    after: &StudioDocumentV1,
    scope: &StudioEditScopeV1,
    requested: bool,
) -> ApiResult<bool> {
    if matches!(scope, StudioEditScopeV1::Nodes { .. }) {
        if requested {
            return Err(ApiError::bad_request(
                "BUILDER_EDIT_OUTSIDE_SCOPE",
                "Automatic layout requires canvas scope",
            ));
        }
        return Ok(false);
    }
    // Parameter/name edits leave the user's arrangement intact. Topology changes need geometry.
    let topology = |document: &StudioDocumentV1| {
        json!({
            "nodes": document.definition.nodes.iter().map(|node| (&node.id, &node.node_type, &node.parent_id)).collect::<Vec<_>>(),
            "connections": document.definition.connections,
        })
    };
    Ok(requested || topology(before) != topology(after))
}

pub(crate) fn matches_result(command: &StudioEditorCommandV1, result: &StudioDocumentV1) -> bool {
    if !command.auto_layout {
        return json!(command.document) == json!(result);
    }
    if json!(command.document.definition) != json!(result.definition)
        || !agentx_domain::validate_editor_document(&result.definition, &result.editor_document)
            .is_empty()
    {
        return false;
    }
    let before = &command.document.editor_document;
    let after = &result.editor_document;
    if before.node_layouts.len() != after.node_layouts.len()
        || before
            .node_layouts
            .iter()
            .zip(&after.node_layouts)
            .any(|(left, right)| left.node_id != right.node_id || left.collapsed != right.collapsed)
        || after.boundary_layouts.len() != 1
    {
        return false;
    }
    // Only coordinates and dimensions are delegated to the editor's ELK implementation.
    let mut comparable = after.clone();
    comparable.node_layouts = before.node_layouts.clone();
    comparable.boundary_layouts = before.boundary_layouts.clone();
    &comparable == before
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentx_domain::{
        BoundaryLayout, EditorDocument, NodeLayout, WorkflowBoundary, WorkflowDefinition,
    };

    fn command() -> StudioEditorCommandV1 {
        StudioEditorCommandV1 {
            command_id: uuid::Uuid::nil(),
            run_id: uuid::Uuid::nil(),
            draft_id: uuid::Uuid::nil(),
            editor_instance_id: uuid::Uuid::nil(),
            expected_local_document_revision: 0,
            expected_document_hash: String::new(),
            result_document_hash: String::new(),
            auto_layout: true,
            changed_node_ids: vec![],
            document: StudioDocumentV1 {
                definition: WorkflowDefinition::empty(),
                editor_document: EditorDocument {
                    node_layouts: vec![NodeLayout {
                        node_id: "exit".into(),
                        x: 0.0,
                        y: 0.0,
                        width: None,
                        height: None,
                        collapsed: false,
                    }],
                    ..Default::default()
                },
            },
        }
    }

    #[test]
    fn layout_receipt_changes_geometry_only() {
        let command = command();
        let mut result = command.document.clone();
        result.editor_document.node_layouts[0].x = 480.0;
        result
            .editor_document
            .boundary_layouts
            .push(BoundaryLayout {
                boundary: WorkflowBoundary::Start,
                x: 0.0,
                y: 0.0,
            });
        assert!(matches_result(&command, &result));
        let mut strict = command.clone();
        strict.auto_layout = false;
        assert!(!matches_result(&strict, &result));
        let mut changed = result.clone();
        changed.definition.nodes[0].parameters =
            json!({"outputs":{"secret":{"kind":"literal","value":"changed"}}});
        assert!(!matches_result(&command, &changed));
        let mut changed = result.clone();
        changed.editor_document.viewport.zoom = 2.0;
        assert!(!matches_result(&command, &changed));
        let mut changed = result.clone();
        changed.editor_document.node_layouts[0].collapsed = true;
        assert!(!matches_result(&command, &changed));
        let mut changed = result.clone();
        changed.editor_document.node_layouts[0].width = Some(-1.0);
        assert!(!matches_result(&command, &changed));
        result.editor_document.node_layouts.clear();
        assert!(!matches_result(&command, &result));
    }

    #[test]
    fn topology_and_explicit_layout_preserve_parameter_only_edits_and_scopes() {
        let before = command().document;
        let mut after = before.clone();
        after.definition.nodes[0].name = "Renamed".into();
        assert!(!needs_layout(&before, &after, &StudioEditScopeV1::Canvas, false).unwrap());
        assert!(needs_layout(&before, &after, &StudioEditScopeV1::Canvas, true).unwrap());
        after.definition.connections.clear();
        assert!(needs_layout(&before, &after, &StudioEditScopeV1::Canvas, false).unwrap());
        let scope = StudioEditScopeV1::Nodes {
            node_ids: vec!["exit".into()],
        };
        assert!(!needs_layout(&before, &after, &scope, false).unwrap());
        assert!(needs_layout(&before, &after, &scope, true).is_err());
    }
}
