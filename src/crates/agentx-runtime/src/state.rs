use std::collections::{BTreeMap, BTreeSet, VecDeque};

use agentx_domain::{AttemptId, NodeExecutionId};
use agentx_node_protocol::{Item, ItemSource, ReadinessPolicy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::{CompiledTerminalConnection, CompiledWorkflow, ExpressionContext, ExpressionEngine};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeExecutionStatus {
    Created,
    Running,
    Waiting,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationStatus {
    Ready,
    Running,
    Waiting,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Running,
    Succeeded,
    Failed,
    Suspended,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PartialExecutionMode {
    Whole,
    Node,
    ToNode,
    FromNode,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAttempt {
    pub id: AttemptId,
    pub attempt_number: u16,
    pub status: AttemptStatus,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeActivation {
    pub id: NodeExecutionId,
    pub node_index: usize,
    pub generation: u32,
    pub slot: u32,
    pub run_index: u32,
    pub status: ActivationStatus,
    pub inputs: BTreeMap<String, Vec<Item>>,
    pub input_delivery_sequences: Vec<u64>,
    pub attempts: Vec<RuntimeAttempt>,
    /// Loop container frame (`{"item":…,"index":…}`) this activation runs in;
    /// only set for activations inside a loop body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_frame: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", content = "items", rename_all = "snake_case")]
pub enum DeliveryKind {
    Data(Vec<Item>),
    ClosedWithoutData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeDelivery {
    pub id: Uuid,
    pub sequence: u64,
    pub connection_index: usize,
    pub source_node_execution_id: NodeExecutionId,
    pub target_node: usize,
    pub target_generation: u32,
    pub kind: DeliveryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_frame: Option<Value>,
}

/// Aggregation state of one loop-container activation: every input item fans
/// out into one body round at a fresh generation; the body sinks converge back
/// here and the collected rounds flush as an ordered array on `main`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingLoop {
    pub source: NodeExecutionId,
    pub node_index: usize,
    pub inputs: Vec<Item>,
    pub next_index: usize,
    /// Active generation -> original input index.
    pub generations: BTreeMap<u32, usize>,
    pub remaining_sinks: BTreeMap<u32, usize>,
    pub buckets: BTreeMap<u32, BTreeMap<usize, BTreeMap<String, Vec<Item>>>>,
    pub results: BTreeMap<usize, Item>,
    pub failures: BTreeSet<usize>,
    pub error_mode: String,
}

#[derive(Debug, Error, PartialEq)]
pub enum MachineError {
    #[error("activation {0} was not found")]
    ActivationNotFound(NodeExecutionId),
    #[error("activation {id} cannot transition from {from:?} to {to:?}")]
    InvalidTransition {
        id: NodeExecutionId,
        from: ActivationStatus,
        to: ActivationStatus,
    },
    #[error("execution is already terminal")]
    ExecutionTerminal,
    #[error("workflow activation budget {0} was exhausted")]
    ActivationBudgetExceeded(u32),
    #[error("resume output port '{0}' is not declared")]
    InvalidResumePort(String),
    #[error("partial execution node '{0}' was not found")]
    PartialNodeNotFound(String),
    #[error("partial execution node '{0}' has no inputs at this checkpoint")]
    PartialInputUnavailable(String),
    #[error("loop outputSelector failed: {0}")]
    LoopOutput(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecutionMachine {
    workflow: CompiledWorkflow,
    status: RuntimeExecutionStatus,
    activations: BTreeMap<NodeExecutionId, NodeActivation>,
    activation_keys: BTreeMap<String, NodeExecutionId>,
    ready: VecDeque<NodeExecutionId>,
    deliveries: Vec<EdgeDelivery>,
    end_deliveries: Vec<EndDelivery>,
    #[serde(default)]
    partial_completion: bool,
    next_delivery_sequence: u64,
    run_counts: Vec<u32>,
    #[serde(default)]
    pending_loops: Vec<PendingLoop>,
    #[serde(default)]
    generation_watermark: u32,
    #[serde(default)]
    tolerated_failures: BTreeSet<NodeExecutionId>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndDelivery {
    pub sequence: u64,
    pub source_node_execution_id: NodeExecutionId,
    pub source_node: usize,
    pub source_port: String,
    pub target_port: String,
    /// Exit node id the delivery reached; empty for partial-execution
    /// redirected terminals.
    #[serde(default)]
    pub target_exit: String,
    pub items: Vec<Item>,
}

impl ExecutionMachine {
    pub fn new(workflow: CompiledWorkflow, input: Vec<Item>) -> Result<Self, MachineError> {
        let node_count = workflow.nodes.len();
        let mut machine = Self {
            workflow,
            status: RuntimeExecutionStatus::Created,
            activations: BTreeMap::new(),
            activation_keys: BTreeMap::new(),
            pending_loops: Vec::new(),
            generation_watermark: 0,
            tolerated_failures: BTreeSet::new(),
            ready: VecDeque::new(),
            deliveries: Vec::new(),
            end_deliveries: Vec::new(),
            partial_completion: false,
            next_delivery_sequence: 1,
            run_counts: vec![0; node_count],
        };
        let starts = machine.workflow.start_nodes.clone();
        for start in starts {
            let mut inputs = BTreeMap::new();
            inputs.insert("main".into(), input.clone());
            machine.create_activation(start, 0, 0, inputs, Vec::new(), ActivationStatus::Ready)?;
        }
        machine.status = RuntimeExecutionStatus::Running;
        // A start→exit workflow has no activations to run; it completes in the
        // same transaction that started it (the exit resolves against the
        // workflow input), instead of lingering in Running forever.
        machine.update_terminal_status();
        Ok(machine)
    }

    pub fn fork_from_checkpoint(
        &self,
        mode: PartialExecutionMode,
        node_id: Option<&str>,
        whole_input: Vec<Item>,
        input_overrides: &Value,
    ) -> Result<Self, MachineError> {
        if mode == PartialExecutionMode::Whole {
            return Self::new(self.workflow.clone(), whole_input);
        }
        let node_id = node_id.ok_or_else(|| MachineError::PartialNodeNotFound(String::new()))?;
        let selected = self
            .workflow
            .nodes
            .iter()
            .position(|node| node.id == node_id)
            .ok_or_else(|| MachineError::PartialNodeNotFound(node_id.into()))?;
        let included = match mode {
            PartialExecutionMode::Node => BTreeSet::from([selected]),
            PartialExecutionMode::ToNode => reachable_nodes(&self.workflow, selected, true),
            PartialExecutionMode::FromNode => reachable_nodes(&self.workflow, selected, false),
            PartialExecutionMode::Whole => unreachable!(),
        };
        let (workflow, indexes) = subgraph(&self.workflow, &included, mode, selected);
        if mode == PartialExecutionMode::ToNode {
            let mut machine = Self::new(workflow, whole_input)?;
            machine.partial_completion = true;
            return Ok(machine);
        }
        let mut inputs = self
            .activations
            .values()
            .filter(|activation| activation.node_index == selected)
            .max_by_key(|activation| activation.run_index)
            .map(|activation| activation.inputs.clone())
            .ok_or_else(|| MachineError::PartialInputUnavailable(node_id.into()))?;
        apply_input_overrides(&mut inputs, input_overrides);
        let mut machine = Self::new_with_starts(workflow, vec![(indexes[&selected], inputs)])?;
        machine.partial_completion = true;
        Ok(machine)
    }

    pub fn new_partial(
        workflow: CompiledWorkflow,
        mode: PartialExecutionMode,
        node_id: &str,
        input: Vec<Item>,
    ) -> Result<Self, MachineError> {
        if mode == PartialExecutionMode::Whole {
            return Self::new(workflow, input);
        }
        let selected = workflow
            .nodes
            .iter()
            .position(|node| node.id == node_id)
            .ok_or_else(|| MachineError::PartialNodeNotFound(node_id.into()))?;
        let included = match mode {
            PartialExecutionMode::Node => BTreeSet::from([selected]),
            PartialExecutionMode::ToNode => reachable_nodes(&workflow, selected, true),
            PartialExecutionMode::FromNode => reachable_nodes(&workflow, selected, false),
            PartialExecutionMode::Whole => unreachable!(),
        };
        let (workflow, indexes) = subgraph(&workflow, &included, mode, selected);
        if mode == PartialExecutionMode::ToNode {
            let mut machine = Self::new(workflow, input)?;
            machine.partial_completion = true;
            return Ok(machine);
        }
        let mut machine = Self::new_with_starts(
            workflow,
            vec![(indexes[&selected], BTreeMap::from([("main".into(), input)]))],
        )?;
        machine.partial_completion = true;
        Ok(machine)
    }

    fn new_with_starts(
        workflow: CompiledWorkflow,
        starts: Vec<(usize, BTreeMap<String, Vec<Item>>)>,
    ) -> Result<Self, MachineError> {
        let node_count = workflow.nodes.len();
        let mut machine = Self {
            workflow,
            status: RuntimeExecutionStatus::Created,
            activations: BTreeMap::new(),
            activation_keys: BTreeMap::new(),
            pending_loops: Vec::new(),
            generation_watermark: 0,
            tolerated_failures: BTreeSet::new(),
            ready: VecDeque::new(),
            deliveries: Vec::new(),
            end_deliveries: Vec::new(),
            partial_completion: false,
            next_delivery_sequence: 1,
            run_counts: vec![0; node_count],
        };
        for (node, inputs) in starts {
            machine.create_activation(node, 0, 0, inputs, Vec::new(), ActivationStatus::Ready)?;
        }
        machine.status = RuntimeExecutionStatus::Running;
        Ok(machine)
    }

    #[must_use]
    pub const fn status(&self) -> RuntimeExecutionStatus {
        self.status
    }

    #[must_use]
    pub fn workflow(&self) -> &CompiledWorkflow {
        &self.workflow
    }

    pub fn activations(&self) -> impl Iterator<Item = &NodeActivation> {
        self.activations.values()
    }

    #[must_use]
    pub fn activation(&self, id: NodeExecutionId) -> Option<&NodeActivation> {
        self.activations.get(&id)
    }

    #[must_use]
    pub fn deliveries(&self) -> &[EdgeDelivery] {
        &self.deliveries
    }

    #[must_use]
    pub fn end_deliveries(&self) -> &[EndDelivery] {
        &self.end_deliveries
    }

    pub fn next_ready(&mut self) -> Option<NodeExecutionId> {
        self.ready.pop_front()
    }

    pub fn replace_ready_inputs(
        &mut self,
        id: NodeExecutionId,
        inputs: BTreeMap<String, Vec<Item>>,
    ) -> Result<(), MachineError> {
        let activation = self
            .activations
            .get_mut(&id)
            .ok_or(MachineError::ActivationNotFound(id))?;
        if activation.status != ActivationStatus::Ready {
            return Err(MachineError::InvalidTransition {
                id,
                from: activation.status,
                to: ActivationStatus::Ready,
            });
        }
        activation.inputs = inputs;
        Ok(())
    }

    pub fn defer_for_confirmation(&mut self, id: NodeExecutionId) -> Result<(), MachineError> {
        self.ensure_active()?;
        let activation = self
            .activations
            .get_mut(&id)
            .ok_or(MachineError::ActivationNotFound(id))?;
        if activation.status != ActivationStatus::Ready {
            return Err(MachineError::InvalidTransition {
                id,
                from: activation.status,
                to: ActivationStatus::Waiting,
            });
        }
        activation.status = ActivationStatus::Waiting;
        self.ready.push_front(id);
        self.status = RuntimeExecutionStatus::Waiting;
        Ok(())
    }

    pub fn resume_confirmation(&mut self) {
        if self.status == RuntimeExecutionStatus::Waiting
            && let Some(id) = self.ready.front().copied()
            && self.activations[&id].status == ActivationStatus::Waiting
        {
            self.activations
                .get_mut(&id)
                .expect("queued confirmation activation exists")
                .status = ActivationStatus::Ready;
            self.status = RuntimeExecutionStatus::Running;
        }
    }

    pub fn start_attempt(&mut self, id: NodeExecutionId) -> Result<AttemptId, MachineError> {
        self.ensure_active()?;
        let activation = self
            .activations
            .get_mut(&id)
            .ok_or(MachineError::ActivationNotFound(id))?;
        if activation.status != ActivationStatus::Ready {
            return Err(MachineError::InvalidTransition {
                id,
                from: activation.status,
                to: ActivationStatus::Running,
            });
        }
        let attempt_id = AttemptId::new();
        activation.attempts.push(RuntimeAttempt {
            id: attempt_id,
            attempt_number: activation.attempts.len() as u16 + 1,
            status: AttemptStatus::Running,
            error_code: None,
            error_message: None,
        });
        activation.status = ActivationStatus::Running;
        Ok(attempt_id)
    }

    pub fn complete(
        &mut self,
        id: NodeExecutionId,
        mut outputs: BTreeMap<String, Vec<Item>>,
    ) -> Result<(), MachineError> {
        self.transition_running(id, ActivationStatus::Succeeded, AttemptStatus::Succeeded)?;
        let node_index = self.activations[&id].node_index;
        if self.workflow.nodes[node_index].settings.always_output_data
            && outputs.values().all(Vec::is_empty)
        {
            outputs.insert(
                "main".into(),
                vec![Item {
                    json: json!({}),
                    ..Item::default()
                }],
            );
        }
        self.emit_outputs(id, &outputs)?;
        self.update_terminal_status();
        Ok(())
    }

    pub fn fail(
        &mut self,
        id: NodeExecutionId,
        code: &str,
        message: &str,
        retryable: bool,
    ) -> Result<(), MachineError> {
        self.ensure_active()?;
        let (node_index, should_retry, policy, generation) = {
            let activation = self
                .activations
                .get_mut(&id)
                .ok_or(MachineError::ActivationNotFound(id))?;
            if !matches!(
                activation.status,
                ActivationStatus::Running | ActivationStatus::Waiting
            ) {
                return Err(MachineError::InvalidTransition {
                    id,
                    from: activation.status,
                    to: ActivationStatus::Failed,
                });
            }
            let attempt = activation
                .attempts
                .last_mut()
                .expect("running activation has attempt");
            attempt.status = AttemptStatus::Failed;
            attempt.error_code = Some(code.into());
            attempt.error_message = Some(message.into());
            let node = &self.workflow.nodes[activation.node_index];
            let should_retry = retryable
                && node.settings.retry_on_fail
                && activation.attempts.len() < usize::from(node.settings.max_tries);
            (
                activation.node_index,
                should_retry,
                node.routes_error,
                activation.generation,
            )
        };
        if should_retry {
            self.activations
                .get_mut(&id)
                .expect("activation exists")
                .status = ActivationStatus::Ready;
            self.ready.push_back(id);
            return Ok(());
        }
        self.activations
            .get_mut(&id)
            .expect("activation exists")
            .status = ActivationStatus::Failed;
        // Loop-body failures obey the container's errorMode: `continue` keeps
        // a null round, `remove` drops it; only `terminate` fails here.
        if !policy
            && self.workflow.nodes[node_index].container.is_some()
            && let Some(position) = self
                .pending_loops
                .iter()
                .position(|pending| pending.generations.contains_key(&generation))
            && self.pending_loops[position].error_mode != "terminate"
        {
            self.tolerated_failures.insert(id);
            return self.finish_loop_generation(position, generation, true);
        }
        // Wiring is the policy: nodes with an outgoing error edge route the
        // failure as an error item through that branch; everything else stops
        // the workflow.
        if policy {
            let mut outputs = BTreeMap::new();
            outputs.insert(
                "error".into(),
                vec![Item {
                    json: json!({
                        "code":code,
                        "message":message,
                        "details":{},
                        "sourceNodeId":self.workflow.nodes[node_index].id,
                        "nodeExecutionId":id,
                        "retryable":retryable
                    }),
                    ..Item::default()
                }],
            );
            self.emit_outputs(id, &outputs)?;
            self.update_terminal_status();
        } else {
            self.status = RuntimeExecutionStatus::Failed;
        }
        Ok(())
    }

    pub fn suspend(&mut self, id: NodeExecutionId) -> Result<(), MachineError> {
        self.transition_running(id, ActivationStatus::Waiting, AttemptStatus::Suspended)?;
        self.status = RuntimeExecutionStatus::Waiting;
        Ok(())
    }

    /// Requeue a suspended node without emitting its output.  This is used by
    /// durable Agent Session wakeups: the waiting attempt yielded because the
    /// Session had an open Operation, so the node must run again after that
    /// Operation settles rather than being treated as a completed wait node.
    pub fn retry_waiting(&mut self, id: NodeExecutionId) -> Result<(), MachineError> {
        self.ensure_active()?;
        let activation = self
            .activations
            .get_mut(&id)
            .ok_or(MachineError::ActivationNotFound(id))?;
        if activation.status != ActivationStatus::Waiting {
            return Err(MachineError::InvalidTransition {
                id,
                from: activation.status,
                to: ActivationStatus::Ready,
            });
        }
        let attempt = activation
            .attempts
            .last_mut()
            .expect("waiting activation has attempt");
        if attempt.status != AttemptStatus::Suspended {
            return Err(MachineError::InvalidTransition {
                id,
                from: activation.status,
                to: ActivationStatus::Ready,
            });
        }
        attempt.status = AttemptStatus::Failed;
        activation.status = ActivationStatus::Ready;
        self.ready.push_back(id);
        self.status = RuntimeExecutionStatus::Running;
        Ok(())
    }

    pub fn resume(
        &mut self,
        id: NodeExecutionId,
        port: &str,
        items: Vec<Item>,
    ) -> Result<(), MachineError> {
        self.ensure_active()?;
        let node_index = self
            .activations
            .get(&id)
            .ok_or(MachineError::ActivationNotFound(id))?
            .node_index;
        let node = &self.workflow.nodes[node_index];
        let port_valid = node.output_ports.iter().any(|value| value == port)
            || port.split_once(':').is_some_and(|(base, _)| {
                node.variadic_output_ports.iter().any(|value| value == base)
            });
        if !port_valid {
            return Err(MachineError::InvalidResumePort(port.into()));
        }
        let activation = self.activations.get_mut(&id).expect("activation exists");
        if activation.status != ActivationStatus::Waiting {
            return Err(MachineError::InvalidTransition {
                id,
                from: activation.status,
                to: ActivationStatus::Succeeded,
            });
        }
        activation.status = ActivationStatus::Succeeded;
        let attempt = activation
            .attempts
            .last_mut()
            .expect("waiting activation has attempt");
        attempt.status = AttemptStatus::Succeeded;
        self.status = RuntimeExecutionStatus::Running;
        self.emit_outputs(id, &BTreeMap::from([(port.into(), items)]))?;
        self.update_terminal_status();
        Ok(())
    }

    pub fn cancel(&mut self) {
        if is_terminal(self.status) {
            return;
        }
        self.status = RuntimeExecutionStatus::Cancelled;
        self.ready.clear();
        for activation in self.activations.values_mut() {
            if matches!(
                activation.status,
                ActivationStatus::Ready | ActivationStatus::Running | ActivationStatus::Waiting
            ) {
                activation.status = ActivationStatus::Cancelled;
                if let Some(attempt) = activation.attempts.last_mut()
                    && matches!(
                        attempt.status,
                        AttemptStatus::Running | AttemptStatus::Suspended
                    )
                {
                    attempt.status = AttemptStatus::Cancelled;
                }
            }
        }
    }

    pub fn timeout(&mut self) {
        if is_terminal(self.status) {
            return;
        }
        self.status = RuntimeExecutionStatus::TimedOut;
        self.ready.clear();
        for activation in self.activations.values_mut() {
            if matches!(
                activation.status,
                ActivationStatus::Ready | ActivationStatus::Running | ActivationStatus::Waiting
            ) {
                activation.status = ActivationStatus::Failed;
                if let Some(attempt) = activation.attempts.last_mut()
                    && matches!(
                        attempt.status,
                        AttemptStatus::Running | AttemptStatus::Suspended
                    )
                {
                    attempt.status = AttemptStatus::Failed;
                    attempt.error_code = Some("NODE_EXECUTION_TIMED_OUT".into());
                    attempt.error_message =
                        Some("Node execution exceeded its operation deadline".into());
                }
            }
        }
    }

    fn transition_running(
        &mut self,
        id: NodeExecutionId,
        target: ActivationStatus,
        attempt_target: AttemptStatus,
    ) -> Result<(), MachineError> {
        self.ensure_active()?;
        let activation = self
            .activations
            .get_mut(&id)
            .ok_or(MachineError::ActivationNotFound(id))?;
        if activation.status != ActivationStatus::Running {
            return Err(MachineError::InvalidTransition {
                id,
                from: activation.status,
                to: target,
            });
        }
        activation.status = target;
        activation
            .attempts
            .last_mut()
            .expect("running activation has attempt")
            .status = attempt_target;
        Ok(())
    }

    fn emit_outputs(
        &mut self,
        source_id: NodeExecutionId,
        outputs: &BTreeMap<String, Vec<Item>>,
    ) -> Result<(), MachineError> {
        let source = self.activations[&source_id].clone();
        let source_node = self.workflow.nodes[source.node_index].clone();

        // Loop container: fan each item into one body round instead of
        // propagating the batch; the aggregate flushes on convergence.
        if let Some(body) = &source_node.loop_body
            && !body.entries.is_empty()
        {
            return self.spawn_loop_iterations(source_id, outputs);
        }
        // Body sinks terminate inside the container: attribute their output
        // to the owning pending loop instead of routing anything onward.
        if let Some(container_id) = source_node.container.as_deref()
            && let Some(position) = self
                .pending_loops
                .iter()
                .position(|pending| pending.generations.contains_key(&source.generation))
        {
            let is_sink = self
                .workflow
                .nodes
                .iter()
                .find(|node| node.id == container_id)
                .and_then(|node| node.loop_body.as_ref())
                .is_some_and(|body| body.sinks.contains(&source.node_index));
            let pending = &mut self.pending_loops[position];
            pending
                .buckets
                .entry(source.generation)
                .or_default()
                .insert(source.node_index, outputs.clone());
            if is_sink {
                if let Some(remaining) = pending.remaining_sinks.get_mut(&source.generation) {
                    *remaining = remaining.saturating_sub(1);
                    if *remaining == 0 {
                        return self.finish_loop_generation(position, source.generation, false);
                    }
                }
                return Ok(());
            }
        }
        self.propagate_outputs(source_id, outputs)
    }

    fn propagate_outputs(
        &mut self,
        source_id: NodeExecutionId,
        outputs: &BTreeMap<String, Vec<Item>>,
    ) -> Result<(), MachineError> {
        let source = self.activations[&source_id].clone();
        let terminal_connections = self
            .workflow
            .terminal_connections
            .iter()
            .filter(|connection| connection.source_node == source.node_index)
            .cloned()
            .collect::<Vec<_>>();
        let mut failed_at_end = false;
        let mut main_at_end = false;
        for connection in terminal_connections {
            let items = outputs
                .get(&connection.source_port)
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                continue;
            }
            self.end_deliveries.push(EndDelivery {
                sequence: self.next_delivery_sequence,
                source_node_execution_id: source_id,
                source_node: source.node_index,
                source_port: connection.source_port,
                target_port: connection.target_port.clone(),
                target_exit: connection.target_exit.clone(),
                items,
            });
            self.next_delivery_sequence += 1;
            failed_at_end |= connection.target_port == "error";
            main_at_end |= connection.target_port == "main";
        }
        let outgoing = self.workflow.nodes[source.node_index]
            .outgoing_connections
            .clone();
        let mut targets = Vec::new();
        for connection_index in outgoing {
            let connection = self.workflow.connections[connection_index].clone();
            let generation = source.generation + u32::from(connection.back_edge);
            let mut items = outputs
                .get(&connection.source_port)
                .cloned()
                .unwrap_or_default();
            for (item_index, item) in items.iter_mut().enumerate() {
                item.lineage.push(ItemSource {
                    node_execution_id: source_id,
                    node_id: self.workflow.nodes[source.node_index].id.clone(),
                    run_index: source.run_index,
                    output_index: connection.branch_order,
                    item_index: item_index as u32,
                });
            }
            let kind = if items.is_empty() {
                DeliveryKind::ClosedWithoutData
            } else {
                DeliveryKind::Data(items)
            };
            let frame = if self.workflow.nodes[connection.target_node]
                .container
                .is_some()
            {
                source.loop_frame.clone()
            } else {
                None
            };
            self.deliveries.push(EdgeDelivery {
                id: Uuid::now_v7(),
                sequence: self.next_delivery_sequence,
                connection_index,
                source_node_execution_id: source_id,
                target_node: connection.target_node,
                target_generation: generation,
                kind,
                loop_frame: frame,
            });
            self.next_delivery_sequence += 1;
            targets.push((connection.target_node, generation, connection_index));
        }
        for (target, generation, edge) in targets {
            self.evaluate_target(target, generation, edge)?;
        }
        if failed_at_end {
            // An error reaching an exit finalizes the execution immediately in
            // both completion modes: the losing branches are cancelled.
            self.fail_fast_at_end();
        } else if main_at_end
            && self.workflow.end.completion == agentx_domain::WorkflowCompletion::FirstReturn
            && !is_terminal(self.status)
        {
            // first_return: the first exit delivery wins; cancel the rest.
            self.status = RuntimeExecutionStatus::Succeeded;
            self.ready.clear();
            for activation in self.activations.values_mut() {
                if matches!(
                    activation.status,
                    ActivationStatus::Ready | ActivationStatus::Running | ActivationStatus::Waiting
                ) {
                    activation.status = ActivationStatus::Cancelled;
                    if let Some(attempt) = activation.attempts.last_mut()
                        && matches!(
                            attempt.status,
                            AttemptStatus::Running | AttemptStatus::Suspended
                        )
                    {
                        attempt.status = AttemptStatus::Cancelled;
                    }
                }
            }
        }
        Ok(())
    }

    fn fail_fast_at_end(&mut self) {
        self.status = RuntimeExecutionStatus::Failed;
        self.ready.clear();
        for activation in self.activations.values_mut() {
            if matches!(
                activation.status,
                ActivationStatus::Ready | ActivationStatus::Running | ActivationStatus::Waiting
            ) {
                activation.status = ActivationStatus::Cancelled;
                if let Some(attempt) = activation.attempts.last_mut()
                    && matches!(
                        attempt.status,
                        AttemptStatus::Running | AttemptStatus::Suspended
                    )
                {
                    attempt.status = AttemptStatus::Cancelled;
                }
            }
        }
    }

    fn spawn_loop_iterations(
        &mut self,
        source_id: NodeExecutionId,
        outputs: &BTreeMap<String, Vec<Item>>,
    ) -> Result<(), MachineError> {
        let source = self.activations[&source_id].clone();
        let body = self.workflow.nodes[source.node_index]
            .loop_body
            .clone()
            .expect("loop body checked by caller");
        let items = outputs
            .get("main")
            .and_then(|items| items.first())
            .and_then(|item| item.json.get("items"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .cloned()
                    .map(|json| Item {
                        json,
                        ..Item::default()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.pending_loops.push(PendingLoop {
            source: source_id,
            node_index: source.node_index,
            inputs: items,
            next_index: 0,
            generations: BTreeMap::new(),
            remaining_sinks: BTreeMap::new(),
            buckets: BTreeMap::new(),
            results: BTreeMap::new(),
            failures: BTreeSet::new(),
            error_mode: body.error_mode.clone(),
        });
        let position = self.pending_loops.len() - 1;
        self.schedule_loop_iterations(position)
    }

    fn schedule_loop_iterations(&mut self, position: usize) -> Result<(), MachineError> {
        let node_index = self.pending_loops[position].node_index;
        let body = self.workflow.nodes[node_index]
            .loop_body
            .clone()
            .expect("pending loop has compiled body");
        let source_id = self.pending_loops[position].source;
        let source = self.activations[&source_id].clone();
        let all_items = self.pending_loops[position]
            .inputs
            .iter()
            .map(|item| item.json.clone())
            .collect::<Vec<_>>();
        while self.pending_loops[position].generations.len() < body.parallelism as usize
            && self.pending_loops[position].next_index < self.pending_loops[position].inputs.len()
        {
            let input_index = self.pending_loops[position].next_index;
            self.pending_loops[position].next_index += 1;
            self.generation_watermark = self.generation_watermark.saturating_add(1);
            let generation = self.generation_watermark;
            self.pending_loops[position]
                .generations
                .insert(generation, input_index);
            self.pending_loops[position]
                .remaining_sinks
                .insert(generation, body.sinks.len());
            let mut item = self.pending_loops[position].inputs[input_index].clone();
            item.lineage.push(ItemSource {
                node_execution_id: source_id,
                node_id: self.workflow.nodes[node_index].id.clone(),
                run_index: source.run_index,
                output_index: 0,
                item_index: input_index as u32,
            });
            let frame = json!({"item":item.json,"items":all_items.clone(),"index":input_index});
            for &entry in &body.entries {
                let id = self.create_activation(
                    entry,
                    generation,
                    0,
                    BTreeMap::from([("main".into(), vec![item.clone()])]),
                    Vec::new(),
                    ActivationStatus::Ready,
                )?;
                self.activations
                    .get_mut(&id)
                    .expect("entry activation")
                    .loop_frame = Some(frame.clone());
            }
        }
        if self.pending_loops[position].inputs.is_empty()
            || self.pending_loops[position].next_index == self.pending_loops[position].inputs.len()
                && self.pending_loops[position].generations.is_empty()
        {
            return self.flush_loop(position);
        }
        Ok(())
    }

    fn finish_loop_generation(
        &mut self,
        position: usize,
        generation: u32,
        failed: bool,
    ) -> Result<(), MachineError> {
        let Some(input_index) = self.pending_loops[position].generations.remove(&generation) else {
            return Ok(());
        };
        self.pending_loops[position]
            .remaining_sinks
            .remove(&generation);
        if failed {
            self.pending_loops[position].failures.insert(input_index);
            let cancelled = self
                .activations
                .iter()
                .filter(|(_, activation)| {
                    activation.generation == generation
                        && matches!(
                            activation.status,
                            ActivationStatus::Ready
                                | ActivationStatus::Running
                                | ActivationStatus::Waiting
                        )
                })
                .map(|(id, _)| *id)
                .collect::<BTreeSet<_>>();
            self.ready.retain(|id| !cancelled.contains(id));
            for id in cancelled {
                if let Some(activation) = self.activations.get_mut(&id) {
                    activation.status = ActivationStatus::Cancelled;
                    if let Some(attempt) = activation.attempts.last_mut()
                        && matches!(
                            attempt.status,
                            AttemptStatus::Running | AttemptStatus::Suspended
                        )
                    {
                        attempt.status = AttemptStatus::Cancelled;
                    }
                }
            }
        } else {
            let result = self.resolve_loop_result(position, generation, input_index)?;
            self.pending_loops[position]
                .results
                .insert(input_index, result);
        }
        self.pending_loops[position].buckets.remove(&generation);
        self.schedule_loop_iterations(position)
    }

    fn resolve_loop_result(
        &self,
        position: usize,
        generation: u32,
        input_index: usize,
    ) -> Result<Item, MachineError> {
        let pending = &self.pending_loops[position];
        let body = self.workflow.nodes[pending.node_index]
            .loop_body
            .as_ref()
            .expect("pending loop body");
        let buckets = pending
            .buckets
            .get(&generation)
            .cloned()
            .unwrap_or_default();
        let mut outputs = serde_json::Map::new();
        let mut output_node_keys = BTreeMap::new();
        let mut current = pending.inputs[input_index].json.clone();
        for (node_index, ports) in &buckets {
            let node = &self.workflow.nodes[*node_index];
            output_node_keys.insert(node.id.clone(), node.key.clone());
            let mut port_values = serde_json::Map::new();
            for (port, items) in ports {
                let values = items
                    .iter()
                    .map(|item| json!({"json":item.json}))
                    .collect::<Vec<_>>();
                if let Some(item) = items.last() {
                    current = item.json.clone();
                }
                let first = values.first().cloned().unwrap_or(Value::Null);
                let last = values.last().cloned().unwrap_or(Value::Null);
                port_values.insert(
                    port.clone(),
                    json!({"current":last,"first":first,"last":last,"all":values}),
                );
            }
            outputs.insert(node.key.clone(), Value::Object(port_values));
        }
        let loop_context = json!({
            "item":pending.inputs[input_index].json,
            "items":pending.inputs.iter().map(|item| item.json.clone()).collect::<Vec<_>>(),
            "index":input_index
        });
        let value = ExpressionEngine
            .resolve_reference(
                &body.output_selector,
                &ExpressionContext {
                    json: current.clone(),
                    input: current,
                    outputs: Value::Object(outputs),
                    loop_context,
                    output_node_keys,
                    ..ExpressionContext::default()
                },
            )
            .map_err(|error| MachineError::LoopOutput(error.to_string()))?;
        Ok(Item {
            json: value,
            lineage: pending.inputs[input_index].lineage.clone(),
            ..Item::default()
        })
    }

    /// Emits one ExactlyOne Item containing `{ "items": [...] }`.
    fn flush_loop(&mut self, position: usize) -> Result<(), MachineError> {
        let pending = self.pending_loops.remove(position);
        let mut values = Vec::new();
        let mut lineage = Vec::new();
        for input_index in 0..pending.inputs.len() {
            if pending.failures.contains(&input_index) {
                if pending.error_mode == "continue" {
                    values.push(Value::Null);
                }
                continue;
            }
            if let Some(item) = pending.results.get(&input_index) {
                values.push(item.json.clone());
                lineage.extend(item.lineage.clone());
            }
        }
        self.propagate_outputs(
            pending.source,
            &BTreeMap::from([(
                "main".into(),
                vec![Item {
                    json: json!({"items":values}),
                    lineage,
                    ..Item::default()
                }],
            )]),
        )
    }

    fn evaluate_target(
        &mut self,
        node_index: usize,
        generation: u32,
        triggering_edge: usize,
    ) -> Result<(), MachineError> {
        let node = self.workflow.nodes[node_index].clone();
        let deliveries = node
            .incoming_connections
            .iter()
            .filter_map(|edge| {
                self.deliveries.iter().rev().find(|delivery| {
                    delivery.connection_index == *edge && delivery.target_generation == generation
                })
            })
            .collect::<Vec<_>>();
        match node.readiness {
            ReadinessPolicy::Any => {
                let Some(delivery) = self.deliveries.iter().rev().find(|delivery| {
                    delivery.connection_index == triggering_edge
                        && delivery.target_generation == generation
                }) else {
                    return Ok(());
                };
                if matches!(delivery.kind, DeliveryKind::Data(_)) {
                    let slot = triggering_edge as u32 + 1;
                    if !self
                        .activation_keys
                        .contains_key(&activation_key(node_index, generation, slot))
                    {
                        let (inputs, sequences) =
                            inputs_from_deliveries(&self.workflow, [delivery]);
                        self.create_activation(
                            node_index,
                            generation,
                            slot,
                            inputs,
                            sequences,
                            ActivationStatus::Ready,
                        )?;
                    }
                } else if deliveries.len() == node.incoming_connections.len()
                    && deliveries
                        .iter()
                        .all(|delivery| matches!(delivery.kind, DeliveryKind::ClosedWithoutData))
                {
                    self.skip_activation(node_index, generation)?;
                }
            }
            ReadinessPolicy::All | ReadinessPolicy::Required => {
                if deliveries.len() != node.incoming_connections.len() {
                    return Ok(());
                }
                let has_data = deliveries
                    .iter()
                    .any(|delivery| matches!(delivery.kind, DeliveryKind::Data(_)));
                let required_ready = node.required_input_ports.iter().all(|required| {
                    deliveries.iter().any(|delivery| {
                        let connection = &self.workflow.connections[delivery.connection_index];
                        port_family(&connection.target_port) == required
                            && matches!(delivery.kind, DeliveryKind::Data(_))
                    })
                });
                if has_data && (node.readiness == ReadinessPolicy::All || required_ready) {
                    if !self
                        .activation_keys
                        .contains_key(&activation_key(node_index, generation, 0))
                    {
                        let (inputs, sequences) =
                            inputs_from_deliveries(&self.workflow, deliveries.iter().copied());
                        self.create_activation(
                            node_index,
                            generation,
                            0,
                            inputs,
                            sequences,
                            ActivationStatus::Ready,
                        )?;
                    }
                } else {
                    self.skip_activation(node_index, generation)?;
                }
            }
        }
        Ok(())
    }

    fn skip_activation(&mut self, node_index: usize, generation: u32) -> Result<(), MachineError> {
        if self
            .activation_keys
            .contains_key(&activation_key(node_index, generation, 0))
        {
            return Ok(());
        }
        let id = self.create_activation(
            node_index,
            generation,
            0,
            BTreeMap::new(),
            Vec::new(),
            ActivationStatus::Skipped,
        )?;
        self.emit_outputs(id, &BTreeMap::new())
    }

    fn create_activation(
        &mut self,
        node_index: usize,
        generation: u32,
        slot: u32,
        inputs: BTreeMap<String, Vec<Item>>,
        sequences: Vec<u64>,
        status: ActivationStatus,
    ) -> Result<NodeExecutionId, MachineError> {
        if self.activations.len() >= self.workflow.activation_budget as usize {
            self.status = RuntimeExecutionStatus::Failed;
            return Err(MachineError::ActivationBudgetExceeded(
                self.workflow.activation_budget,
            ));
        }
        let id = NodeExecutionId::new();
        let run_index = self.run_counts[node_index];
        self.run_counts[node_index] += 1;
        self.activations.insert(
            id,
            NodeActivation {
                id,
                node_index,
                generation,
                slot,
                loop_frame: self
                    .deliveries
                    .iter()
                    .rev()
                    .find(|delivery| {
                        delivery.target_node == node_index
                            && delivery.target_generation == generation
                            && delivery.loop_frame.is_some()
                    })
                    .and_then(|delivery| delivery.loop_frame.clone()),
                run_index,
                status,
                inputs,
                input_delivery_sequences: sequences,
                attempts: Vec::new(),
            },
        );
        self.activation_keys
            .insert(activation_key(node_index, generation, slot), id);
        if status == ActivationStatus::Ready {
            self.ready.push_back(id);
        }
        Ok(id)
    }

    fn ensure_active(&self) -> Result<(), MachineError> {
        if is_terminal(self.status) {
            Err(MachineError::ExecutionTerminal)
        } else {
            Ok(())
        }
    }

    fn update_terminal_status(&mut self) {
        if is_terminal(self.status) || self.status == RuntimeExecutionStatus::Waiting {
            return;
        }
        if !self.pending_loops.is_empty() {
            return;
        }
        let open = self.activations.values().any(|activation| {
            matches!(
                activation.status,
                ActivationStatus::Ready | ActivationStatus::Running | ActivationStatus::Waiting
            )
        });
        if !open && self.ready.is_empty() {
            self.status = if self.activations.iter().any(|(id, activation)| {
                activation.status == ActivationStatus::Failed
                    && !self.tolerated_failures.contains(id)
            }) {
                RuntimeExecutionStatus::Failed
            } else if self.partial_completion
                || self.workflow.start_to_exit.is_some()
                || self
                    .end_deliveries
                    .iter()
                    .any(|delivery| delivery.target_port == "main")
            {
                RuntimeExecutionStatus::Succeeded
            } else {
                RuntimeExecutionStatus::Failed
            };
        }
    }
}

fn reachable_nodes(workflow: &CompiledWorkflow, selected: usize, reverse: bool) -> BTreeSet<usize> {
    let mut included = BTreeSet::from([selected]);
    let mut frontier = VecDeque::from([selected]);
    while let Some(node) = frontier.pop_front() {
        let connections = if reverse {
            &workflow.nodes[node].incoming_connections
        } else {
            &workflow.nodes[node].outgoing_connections
        };
        for connection in connections {
            let connection = &workflow.connections[*connection];
            let next = if reverse {
                connection.source_node
            } else {
                connection.target_node
            };
            if included.insert(next) {
                frontier.push_back(next);
            }
        }
    }
    included
}

fn subgraph(
    source: &CompiledWorkflow,
    included: &BTreeSet<usize>,
    mode: PartialExecutionMode,
    selected: usize,
) -> (CompiledWorkflow, BTreeMap<usize, usize>) {
    let selected_source = selected;
    let indexes = included
        .iter()
        .enumerate()
        .map(|(new, old)| (*old, new))
        .collect::<BTreeMap<_, _>>();
    let mut nodes = included
        .iter()
        .map(|old| {
            let mut node = source.nodes[*old].clone();
            node.index = indexes[old];
            node.incoming_connections.clear();
            node.outgoing_connections.clear();
            node
        })
        .collect::<Vec<_>>();
    let mut connections = Vec::new();
    for connection in &source.connections {
        if let (Some(&from), Some(&to)) = (
            indexes.get(&connection.source_node),
            indexes.get(&connection.target_node),
        ) {
            let mut connection = connection.clone();
            connection.index = connections.len();
            connection.source_node = from;
            connection.target_node = to;
            nodes[from].outgoing_connections.push(connection.index);
            nodes[to].incoming_connections.push(connection.index);
            connections.push(connection);
        }
    }
    let selected = indexes[&selected_source];
    let start_nodes = if matches!(
        mode,
        PartialExecutionMode::Node | PartialExecutionMode::FromNode
    ) {
        vec![selected]
    } else {
        source
            .start_nodes
            .iter()
            .filter_map(|old| indexes.get(old).copied())
            .collect()
    };
    let components: Vec<Vec<usize>> = source
        .strongly_connected_components
        .iter()
        .filter_map(|component| {
            let mapped = component
                .iter()
                .filter_map(|old| indexes.get(old).copied())
                .collect::<Vec<_>>();
            (!mapped.is_empty()).then_some(mapped)
        })
        .collect();
    for (component_index, component) in components.iter().enumerate() {
        for node in component {
            nodes[*node].component_index = component_index;
        }
    }
    let mut workflow = source.clone();
    workflow.canonical_hash = format!(
        "{}:partial:{mode:?}:{}",
        source.canonical_hash, source.nodes[selected_source].id
    );
    workflow.nodes = nodes;
    workflow.connections = connections;
    let mut terminal_connections = source
        .terminal_connections
        .iter()
        .filter_map(|connection| {
            Some(CompiledTerminalConnection {
                id: connection.id.clone(),
                source_node: *indexes.get(&connection.source_node)?,
                source_port: connection.source_port.clone(),
                target_port: connection.target_port.clone(),
                target_exit: connection.target_exit.clone(),
                branch_order: connection.branch_order,
            })
        })
        .collect::<Vec<_>>();
    if matches!(
        mode,
        PartialExecutionMode::Node | PartialExecutionMode::ToNode
    ) {
        let source_port = workflow.nodes[selected]
            .output_ports
            .iter()
            .find(|port| port.as_str() == "main")
            .or_else(|| workflow.nodes[selected].output_ports.first())
            .cloned()
            .unwrap_or_else(|| "main".into());
        terminal_connections.clear();
        terminal_connections.push(CompiledTerminalConnection {
            id: format!("__partial_end__:{}", source.nodes[selected_source].id),
            source_node: selected,
            source_port,
            target_port: "main".into(),
            target_exit: String::new(),
            branch_order: 0,
        });
        workflow.end.outputs.clear();
        for exit in workflow.exits.values_mut() {
            exit.outputs.clear();
        }
    }
    workflow.terminal_connections = terminal_connections;
    workflow.start_to_exit = source
        .start_to_exit
        .clone()
        .filter(|_| start_nodes.is_empty());
    workflow.start_nodes = start_nodes;
    workflow.strongly_connected_components = components;
    (workflow, indexes)
}

fn apply_input_overrides(inputs: &mut BTreeMap<String, Vec<Item>>, overrides: &Value) {
    if overrides.is_null() {
        return;
    }
    if inputs.values().all(Vec::is_empty) {
        inputs.insert(
            "main".into(),
            vec![Item {
                json: overrides.clone(),
                ..Item::default()
            }],
        );
        return;
    }
    for item in inputs.values_mut().flatten() {
        if let (Some(target), Some(values)) = (item.json.as_object_mut(), overrides.as_object()) {
            target.extend(values.clone());
        } else {
            item.json = overrides.clone();
        }
    }
}

fn inputs_from_deliveries<'a>(
    workflow: &CompiledWorkflow,
    deliveries: impl IntoIterator<Item = &'a EdgeDelivery>,
) -> (BTreeMap<String, Vec<Item>>, Vec<u64>) {
    let mut inputs = BTreeMap::<String, Vec<Item>>::new();
    let mut sequences = BTreeSet::new();
    for delivery in deliveries {
        sequences.insert(delivery.sequence);
        if let DeliveryKind::Data(items) = &delivery.kind {
            let port = workflow.connections[delivery.connection_index]
                .target_port
                .clone();
            inputs.entry(port).or_default().extend(items.clone());
        }
    }
    (inputs, sequences.into_iter().collect())
}

fn port_family(port: &str) -> &str {
    port.split_once(':').map_or(port, |(family, _)| family)
}

fn activation_key(node_index: usize, generation: u32, slot: u32) -> String {
    format!("{node_index}:{generation}:{slot}")
}

const fn is_terminal(status: RuntimeExecutionStatus) -> bool {
    matches!(
        status,
        RuntimeExecutionStatus::Succeeded
            | RuntimeExecutionStatus::Failed
            | RuntimeExecutionStatus::Cancelled
            | RuntimeExecutionStatus::TimedOut
    )
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
