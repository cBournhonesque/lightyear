use crate::input_buffer::InputBuffer;
use crate::input_message::ActionStateSequence;
use alloc::string::ToString;
use alloc::sync::Arc;
use bevy_ecs::entity::EntityHashMap;
use bevy_ecs::prelude::{Add, Entity, On, Remove, ResMut, Resource};
use core::marker::PhantomData;
use std::sync::OnceLock;

/// Metric handles for one input sequence, including entity-labeled series.
#[derive(Debug, Resource)]
pub(crate) struct InputMetricHandles<S> {
    remote_player_receive: OnceLock<metrics::Counter>,
    entities: EntityHashMap<EntityMetricHandles>,
    marker: PhantomData<fn() -> S>,
}

impl<S> Default for InputMetricHandles<S> {
    fn default() -> Self {
        Self {
            remote_player_receive: OnceLock::new(),
            entities: EntityHashMap::default(),
            marker: PhantomData,
        }
    }
}

impl<S: ActionStateSequence> InputMetricHandles<S> {
    pub(crate) fn insert_entity(&mut self, entity: Entity) {
        self.entities
            .entry(entity)
            .or_insert_with(|| EntityMetricHandles::new(entity));
    }

    fn remove_entity(&mut self, entity: Entity) {
        self.entities.remove(&entity);
    }

    fn entity(&self, entity: Entity) -> &EntityMetricHandles {
        self.entities
            .get(&entity)
            .expect("InputBuffer should have an input metric cache entry")
    }

    pub(crate) fn remote_player_receive(&self) -> &metrics::Counter {
        self.remote_player_receive.get_or_init(|| {
            metrics::counter!(
                "inputs/remote_player/receive",
                "action" => core::any::type_name::<S::Action>(),
            )
        })
    }

    pub(crate) fn buffer_size(&self, entity: Entity) -> &metrics::Gauge {
        self.entity(entity).buffer_size::<S>()
    }

    pub(crate) fn remote_player_buffer_margin(&self, entity: Entity) -> &metrics::Gauge {
        self.entity(entity).remote_player_buffer_margin::<S>()
    }

    pub(crate) fn remote_player_buffer_size(&self, entity: Entity) -> &metrics::Gauge {
        self.entity(entity).remote_player_buffer_size::<S>()
    }
}

/// Metric handles whose identity includes an entity label.
#[derive(Debug)]
struct EntityMetricHandles {
    entity_label: Arc<str>,
    buffer_size: OnceLock<metrics::Gauge>,
    remote_player_buffer_margin: OnceLock<metrics::Gauge>,
    remote_player_buffer_size: OnceLock<metrics::Gauge>,
}

impl EntityMetricHandles {
    fn new(entity: Entity) -> Self {
        Self {
            entity_label: Arc::from(entity.to_string()),
            buffer_size: OnceLock::new(),
            remote_player_buffer_margin: OnceLock::new(),
            remote_player_buffer_size: OnceLock::new(),
        }
    }

    fn buffer_size<S: ActionStateSequence>(&self) -> &metrics::Gauge {
        self.buffer_size.get_or_init(|| {
            metrics::gauge!(
                "inputs/buffer_size",
                "action" => core::any::type_name::<S::Action>(),
                "entity" => Arc::clone(&self.entity_label),
            )
        })
    }

    fn remote_player_buffer_margin<S: ActionStateSequence>(&self) -> &metrics::Gauge {
        self.remote_player_buffer_margin.get_or_init(|| {
            metrics::gauge!(
                "inputs/remote_player/buffer_margin",
                "action" => core::any::type_name::<S::Action>(),
                "entity" => Arc::clone(&self.entity_label),
            )
        })
    }

    fn remote_player_buffer_size<S: ActionStateSequence>(&self) -> &metrics::Gauge {
        self.remote_player_buffer_size.get_or_init(|| {
            metrics::gauge!(
                "inputs/remote_player/buffer_size",
                "action" => core::any::type_name::<S::Action>(),
                "entity" => Arc::clone(&self.entity_label),
            )
        })
    }
}

pub(crate) fn add_input_metric_handles<S: ActionStateSequence>(
    trigger: On<Add, InputBuffer<S::Snapshot, S::Action>>,
    mut metric_handles: ResMut<InputMetricHandles<S>>,
) {
    metric_handles.insert_entity(trigger.entity);
}

pub(crate) fn remove_input_metric_handles<S: ActionStateSequence>(
    trigger: On<Remove, InputBuffer<S::Snapshot, S::Action>>,
    mut metric_handles: ResMut<InputMetricHandles<S>>,
) {
    metric_handles.remove_entity(trigger.entity);
}
