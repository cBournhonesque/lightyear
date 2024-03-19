//! Benchmark to measure the performance of replicating Entity spawns
#![allow(unused_imports)]

use bevy::asset::ron;
use bevy::prelude::*;
use bevy::reflect::serde::ReflectSerializer;
use bevy::reflect::TypeRegistry;
use serde::Serialize;

#[derive(Reflect, Serialize, Debug)]
#[reflect(Serialize)]
pub struct A(Vec<i32>);

fn main() {
    let mut registry = TypeRegistry::new();
    registry.register::<A>();

    let a = A(vec![2, 4]);
    dbg!(&a);

    let reflect_serialize = registry
        .get_type_data::<ReflectSerialize>(std::any::TypeId::of::<A>())
        .unwrap();

    let reflect_a = A::as_reflect(&a);
    dbg!(&reflect_a);

    // manually get a dyn Serialize using the reflect serialize type metadata
    let reflect_a_serializable = reflect_serialize.get_serializable(reflect_a);
    let mut serializer = ron::Serializer::new(std::io::stderr(), None).unwrap();
    dbg!("serializing by directly calling reflect_serialize");
    reflect_a_serializable
        .borrow()
        .serialize(&mut serializer)
        .unwrap();
    println!("");

    // serialize using the ReflectSerializer
    let reflect_serializer = ReflectSerializer::new(&a, &registry);
    let serialized_value: String = ron::to_string(&reflect_serializer).unwrap();
    dbg!(&serialized_value);
}
