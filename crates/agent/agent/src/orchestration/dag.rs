use std::collections::HashSet;

/// A node in the speculative execution trajectory.
#[derive(Debug, Clone)]
pub struct SpeculativeNode {
    pub name: String,
    pub args: String,
    pub dependencies: HashSet<usize>, // Indices of parent nodes in the execution sequence
}

/// A DAG representing a speculative execution plan.
///
/// Allows the agent to propose multiple actions that can be executed
/// together if their dependencies are met.
#[derive(Debug, Clone, Default)]
pub struct SpeculativeDag {
    pub nodes: Vec<SpeculativeNode>,
}

impl SpeculativeDag {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a new speculative action to the plan.
    pub fn add_step(&mut self, name: String, args: String, deps: Vec<usize>) {
        self.nodes.push(SpeculativeNode {
            name,
            args,
            dependencies: deps.into_iter().collect(),
        });
    }

    /// Returns the number of speculative steps in the plan.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns true if the plan is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Partitions the DAG into parallel execution lanes.
    ///
    /// Returns a list of "Lanes", where each lane contains indices of nodes
    /// that can be executed concurrently because all their dependencies
    /// were satisfied by previous lanes.
    pub fn partition_lanes(&self) -> Vec<Vec<usize>> {
        let mut planes = Vec::new();
        let mut executed = HashSet::new();
        let mut remaining: HashSet<usize> = (0..self.nodes.len()).collect();

        while !remaining.is_empty() {
            let mut current_lane = Vec::new();

            for &idx in &remaining {
                let node = &self.nodes[idx];
                if node.dependencies.is_subset(&executed) {
                    current_lane.push(idx);
                }
            }

            if current_lane.is_empty() {
                tracing::warn!(
                    remaining = %remaining.len(),
                    "DAG partition_lanes: no progress — possible cycle or missing dependency. \
                     {} nodes left unprocessed.",
                    remaining.len()
                );
                break;
            }

            for &idx in &current_lane {
                executed.insert(idx);
                remaining.remove(&idx);
            }
            planes.push(current_lane);
        }

        planes
    }
}

/// Parses a list of actions into a simple sequential DAG.
///
/// In future iterations, this will use LLM markers or structural
/// analysis to detect independent parallel actions.
pub fn parse_sequential_dag(actions: Vec<(String, String)>) -> SpeculativeDag {
    let mut dag = SpeculativeDag::new();
    for (i, (name, args)) in actions.into_iter().enumerate() {
        let deps = if i > 0 { vec![i - 1] } else { vec![] };
        dag.add_step(name, args, deps);
    }
    dag
}
