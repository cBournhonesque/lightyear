# Interpolation

## Introduction
Interpolation means that we will store replicated entities in a buffer, and then interpolate between the last two states to get a smoother movement.

See this excellent explanation from Valve: [link](https://developer.valvesoftware.com/wiki/Source_Multiplayer_Networking)
or this one from Gabriel Gambetta: [link](https://www.gabrielgambetta.com/entity-interpolation.html)


## Implementation

In lightyear, interpolation can be automatically managed for you.

Every replicated entity can specify to which clients it should be interpolated to:
```rust,noplayground
Replicate {
    interpolation_target: NetworkTarget::AllExcept(vec![id]),
    ..default()
},
```

This means that all clients except for the one with id `id` will interpolate this entity.
In practice, it means that they will store in a buffer the history for all components that are enabled for Interpolation.


## Component Sync Mode

Not all components in the protocol are necessarily interpolated.
Each component can implement a `ComponentSyncMode` that defines how it gets handled for the `Predicted` and `Interpolated` entities.

Only components that have `ComponentSyncMode::Full` will be interpolated.


## Interpolation function

By default, the implementation function for a given component will be linear interpolation.
It is also possibly to override this behaviour by implementing a custom interpolation function.

Here is an example:

```rust,noplayground
    #[derive(Component, Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Component1(pub f32);
    #[derive(Component, Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Component2(pub f32);

    #[component_protocol(protocol = "MyProtocol")]
    pub enum MyComponentProtocol {
        #[sync(full)]
        Component1(Component1),
        #[sync(full, lerp = "MyCustomInterpFn")]
        Component2(Component2),
    }

    // custom interpolation logic
    pub struct MyCustomInterpFn;
    impl<C> InterpFn<C> for MyCustomInterpFn {
        fn lerp(start: C, _other: C, _t: f32) -> C {
            start
        }
    }
```

You will have to add the attribute `lerp = "TYPE_NAME"` to the component.
The `TYPE_NAME` must be a type that implements the `InterpFn` trait.
```rust,noplayground
pub trait InterpFn<C> {
    fn lerp(start: C, other: C, t: f32) -> C;
}
```


## Complex interpolation

In some cases, the interpolation logic can be more complex than a simple linear interpolation.
For example, we might want to have different interpolation functions for different entities, even if they have the same component type.
Or we might want to do interpolation based on multiple comments (applying some cubic spline interpolation that relies not only on the position,
but also on the velocity and acceleration).

In those cases, you can disable the default per-component interpolation logic and provide your own custom logic.
```rust,noplayground```



