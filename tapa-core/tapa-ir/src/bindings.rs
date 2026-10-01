//! Borrowed logical connections shared by graph consumers.

use crate::{Arg, EndpointRef, Port, Task, TaskGraph, TaskInstance};

/// A child's bound argument and its declaration, when metadata is available.
#[derive(Debug, Clone, Copy)]
pub struct PortBinding<'a> {
    pub port_name: &'a str,
    pub argument: &'a Arg,
    /// Exact channel declarations take precedence over an array's base port.
    pub port: Option<&'a Port>,
}

/// A declared FIFO endpoint, which may have incomplete task/port metadata.
/// Keeping the reference distinguishes missing metadata from an external end.
#[derive(Debug, Clone, Copy)]
pub struct FifoEndpoint<'a> {
    pub reference: &'a EndpointRef,
    pub task: Option<&'a Task>,
    pub binding: Option<PortBinding<'a>>,
}

fn instance_bindings<'a>(
    definition: Option<&'a Task>,
    instance: &'a TaskInstance,
) -> impl Iterator<Item = PortBinding<'a>> {
    instance
        .args
        .iter()
        .map(move |(port_name, argument)| PortBinding {
            port_name,
            argument,
            port: definition.and_then(|task| task.port(port_name)),
        })
}

impl TaskGraph {
    /// Child bindings in definition, instance, and argument order.
    pub fn bindings<'a>(&'a self, parent: &'a Task) -> impl Iterator<Item = PortBinding<'a>> {
        parent.tasks.iter().flat_map(|(name, instances)| {
            let definition = self.tasks.get(name);
            instances
                .iter()
                .flat_map(move |instance| instance_bindings(definition, instance))
        })
    }

    /// Resolve one FIFO endpoint's instance argument. `None` denotes an
    /// external end; a present reference remains present even when metadata
    /// cannot be resolved, so consumers can apply their own error/fallback policy.
    #[must_use]
    pub fn fifo_endpoint<'a>(
        &'a self,
        parent: &'a Task,
        fifo_name: &str,
        reference: Option<&'a EndpointRef>,
    ) -> Option<FifoEndpoint<'a>> {
        let reference = reference?;
        let task = self.tasks.get(&reference.0);
        let binding = parent
            .tasks
            .get(&reference.0)
            .and_then(|instances| instances.get(reference.1 as usize))
            .and_then(|instance| {
                instance_bindings(task, instance)
                    .find(|binding| binding.argument.name() == Some(fifo_name))
            });
        Some(FifoEndpoint {
            reference,
            task,
            binding,
        })
    }
}
