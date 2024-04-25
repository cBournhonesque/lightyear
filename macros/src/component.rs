use crate::shared::{get_fields, get_inner_generic, strip_attributes};
use darling::ast::NestedMeta;
use darling::util::{Flag, PathList};
use darling::{Error, FromField, FromMeta, FromVariant};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use std::ops::Deref;
use syn::punctuated::Punctuated;
use syn::{
    parse_macro_input, parse_quote, Field, Fields, GenericParam, Generics, ItemEnum, MetaList,
    PathArguments, Token, Type, TypeParam,
};

// TODO: use FromDeriveInput ?
#[derive(Debug, FromMeta)]
/// Struct that will hold the value of attributes passed to the macro itself (component_protocol(...))
struct MacroAttrs {
    protocol: Ident,
    #[darling(default)]
    derive: PathList,
}

const ATTRIBUTES: &[&str] = &["protocol"];

#[derive(Debug, FromField)]
#[darling(attributes(protocol))]
struct AttrField {
    ident: Option<Ident>,
    ty: Type,

    sync: Option<SyncField>,
    #[darling(default)]
    map_entities: MapField,
}

#[derive(Debug, FromMeta, PartialEq, Eq)]
enum SyncMode {
    Full,
    Simple,
    Once,
}

#[derive(Debug, FromMeta)]
struct SyncField {
    mode: SyncMode,
    #[darling(default)]
    lerp: Option<Ident>,
    #[darling(default)]
    corrector: Option<Ident>,
}

#[derive(Debug, Default, FromMeta, PartialEq, Eq)]
enum MapField {
    #[default]
    NoMapEntities,
    Custom,
    #[darling(word)]
    MapWithMapEntity,
}

impl SyncField {
    fn get_mode_tokens(&self) -> TokenStream {
        match self.mode {
            SyncMode::Full => quote! {ComponentSyncMode::Full},
            SyncMode::Simple => quote! {ComponentSyncMode::Simple},
            SyncMode::Once => quote! {ComponentSyncMode::Once},
        }
    }
}

pub fn component_protocol_impl(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
    shared_crate_name: TokenStream,
) -> proc_macro::TokenStream {
    let attr_args = match NestedMeta::parse_meta_list(args.into()) {
        Ok(v) => v,
        Err(e) => {
            return Error::from(e).write_errors().into();
        }
    };
    let attr = match MacroAttrs::from_list(&attr_args) {
        Ok(v) => v,
        Err(e) => {
            return e.write_errors().into();
        }
    };
    let extra_derives = if attr.derive.is_empty() {
        quote! {}
    } else {
        let derives = attr.derive.deref();
        quote! {#[derive(#(#derives),*)]}
    };
    let protocol = &attr.protocol;

    let mut input = parse_macro_input!(input as ItemEnum);

    // Add extra variants
    input.variants.push(parse_quote! {
        ShouldBePredicted(ShouldBePredicted)
    });
    input.variants.push(parse_quote! {
        PrePredicted(PrePredicted)
    });
    input.variants.push(parse_quote! {
        PreSpawnedPlayerObject(PreSpawnedPlayerObject)
    });
    input.variants.push(parse_quote! {
        ShouldBeInterpolated(ShouldBeInterpolated)
    });
    input.variants.push(parse_quote! {
        #[protocol(map_entities)]
        ParentSync(ParentSync)
    });
    #[cfg(feature = "leafwing")]
    for i in 1..3 {
        let variant = Ident::new(&format!("ActionState{}", i), Span::call_site());
        let ty = Ident::new(&format!("LeafwingInput{}", i), Span::call_site());
        input.variants.push(parse_quote! {
            #[protocol(sync(mode="simple"))]
            #variant(ActionState<<#protocol as Protocol>::#ty>)
        });
    }

    // Helper Properties
    let fields = get_fields(&input);
    let input_without_attributes = strip_attributes(&input, ATTRIBUTES);
    let attr_fields: Vec<AttrField> = fields
        .iter()
        .map(|field| FromField::from_field(field).unwrap())
        .collect();

    // Names
    let enum_name = &input.ident;
    let enum_kind_name = format_ident!("{}Kind", enum_name);
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    // Impls
    let sync_component_impl = sync_metadata_impl(&attr_fields, enum_name);

    // Methods
    let add_resource_send_method = add_resource_send_method(&fields, protocol);
    let add_resource_receive_method = add_resource_receive_method(&fields, protocol);
    let add_systems_method = add_per_component_replication_send_systems_method(&fields, protocol);
    let add_events_method = add_events_method(&fields);
    let push_component_events_method = push_component_events_method(&fields, protocol);
    let add_sync_systems_method = add_sync_systems_method(&attr_fields, protocol);
    // let mode_method = mode_method(&input, &fields);
    let encode_method = encode_method();
    let decode_method = decode_method();
    let map_entities_method = map_entities_method(&attr_fields, &input, &enum_kind_name);
    let insert_method = insert_method(&input, &fields);
    let update_method = update_method(&input, &fields);
    let type_ids_method = type_ids_method(&fields, &enum_kind_name);

    // EnumKind methods
    let enum_kind = get_enum_kind(&input, &enum_kind_name);
    let from_method = from_method(&input, &enum_kind_name, &fields);
    let remove_method = remove_method(&input, &fields, &enum_kind_name);

    let gen = quote! {
        #[doc(hidden)]
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use #shared_crate_name::_reexport::*;
            use #shared_crate_name::prelude::*;
            use #shared_crate_name::prelude::client::*;
            use bevy::ecs::entity::{EntityHashSet, MapEntities, EntityMapper};
            use bevy::prelude::{App, Entity, IntoSystemConfigs, EntityWorldMut, World, Reflect};
            use bevy::utils::HashMap;
            use std::any::TypeId;
            use #shared_crate_name::shared::events::components::{ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent};
            #[cfg(feature = "leafwing")]
            use leafwing_input_manager::prelude::*;

            #[derive(Serialize, Deserialize, Clone, PartialEq)]
            #extra_derives
            #[enum_delegate::implement(ComponentBehaviour)]
            #input_without_attributes

            impl ComponentProtocol for #enum_name {
                type Protocol = #protocol;

                #type_ids_method
                #insert_method
                #update_method
                #add_resource_send_method
                #add_resource_receive_method
                #add_systems_method
                #add_events_method
                #push_component_events_method
                #add_sync_systems_method

                // #mode_method
            }

            // impl std::hash::Hash for #enum_name {
            //     fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            //         let kind: #enum_kind_name = self.into();
            //         kind.hash(state);
            //     }
            // }

            impl std::fmt::Debug for #enum_name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                    let kind: #enum_kind_name = self.into();
                    kind.fmt(f)
                }
            }

            impl std::fmt::Display for #enum_name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                    std::fmt::Debug::fmt(self, f)
                }
            }

            #sync_component_impl
            #map_entities_method

            #[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Reflect)]
            #[repr(C)]
            #enum_kind

            impl ComponentProtocolKind for #enum_kind_name {
                type Protocol = #protocol;
            }

            #from_method

            impl ComponentKindBehaviour for #enum_kind_name {
                #remove_method
            }

            impl std::fmt::Display for #enum_kind_name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                    std::fmt::Debug::fmt(self, f)
                }
            }
            // TODO: we don't need to implement for now because we get it for free from Serialize + Deserialize + Clone
            // impl BitSerializable for #enum_name {
            //     #encode_method
            //     #decode_method
            // }
        }
        pub use #module_name::#enum_name as #enum_name;
        pub use #module_name::#enum_kind_name as #enum_kind_name;
    };

    proc_macro::TokenStream::from(gen)
}

fn add_resource_send_method(fields: &Vec<Field>, protocol_name: &Ident) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let ty = &field.ty;
        if !quote!(#ty).to_string().starts_with("ReplicateResource") {
            continue;
        }
        let resource = get_inner_generic(ty)
            .expect("ReplicateResource must have a generic type: ReplicateResource<R>");
        body = quote! {
            #body
            add_resource_send_systems::<#protocol_name, R, #resource>(app);
        };
    }
    quote! {
        fn add_resource_send_systems<R: ReplicationSend<#protocol_name>>(app: &mut App)
        {
            #body
        }
    }
}

fn add_resource_receive_method(fields: &Vec<Field>, protocol_name: &Ident) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let ty = &field.ty;
        if !quote!(#ty).to_string().starts_with("ReplicateResource") {
            continue;
        }
        let resource = get_inner_generic(ty)
            .expect("ReplicateResource must have a generic type: ReplicateResource<R>");
        body = quote! {
            #body
            add_resource_receive_systems::<#protocol_name, R, #resource>(app);
        };
    }
    quote! {
        fn add_resource_receive_systems<R: ReplicationSend<#protocol_name>>(app: &mut App)
        {
            #body
        }
    }
}

fn add_per_component_replication_send_systems_method(
    fields: &Vec<Field>,
    protocol_name: &Ident,
) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let component_type = &field.ty;
        body = quote! {
            #body
            add_per_component_replication_send_systems::<#component_type, #protocol_name, R>(app);
        };
    }
    quote! {
        fn add_per_component_replication_send_systems<R: ReplicationSend<#protocol_name>>(app: &mut App)
        {
            #body
        }
    }
}

fn push_component_events_method(fields: &Vec<Field>, protocol_name: &Ident) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let component_type = &field.ty;
        body = quote! {
            #body
            push_component_insert_events::<#component_type, #protocol_name, E, Ctx>(world, events);
            push_component_remove_events::<#component_type, #protocol_name, E, Ctx>(world, events);
            push_component_update_events::<#component_type, #protocol_name, E, Ctx>(world, events);
        };
    }
    quote! {
        fn push_component_events<
            E: IterComponentInsertEvent<#protocol_name, Ctx>
                + IterComponentRemoveEvent<#protocol_name, Ctx>
                + IterComponentUpdateEvent<#protocol_name, Ctx>,
            Ctx: EventContext,
        >(
            world: &mut World,
            events: &mut E
        )
        {
            #body
        }
    }
}

fn add_events_method(fields: &Vec<Field>) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let component_type = &field.ty;
        body = quote! {
            #body
            app.add_event::<ComponentInsertEvent<#component_type, Ctx>>();
            app.add_event::<ComponentUpdateEvent<#component_type, Ctx>>();
            app.add_event::<ComponentRemoveEvent<#component_type, Ctx>>();
        };
    }
    quote! {
        fn add_events<Ctx: EventContext>(app: &mut App)
        {
            #body
        }
    }
}

fn sync_metadata_impl(fields: &Vec<AttrField>, enum_name: &Ident) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let Some(sync) = &field.sync else { continue };
        let component_type = &field.ty;
        let mode = sync.get_mode_tokens();

        // mode
        // interpolation
        let interpolator = &sync.lerp.clone().unwrap_or_else(|| {
            if sync.mode == SyncMode::Full {
                Ident::new("LinearInterpolator", Span::call_site())
            } else {
                Ident::new("NullInterpolator", Span::call_site())
            }
        });
        // prediction
        let mut corrector = sync
            .corrector
            .clone()
            .unwrap_or(Ident::new("InstantCorrector", Span::call_site()));
        if corrector == "InterpolatedCorrector" {
            corrector = interpolator.clone();
        }
        body = quote! {
            #body
            impl SyncMetadata<#component_type> for #enum_name {
                type Interpolator = #interpolator;
                type Corrector = #corrector;
                fn mode() -> ComponentSyncMode {
                    #mode
                }
            }
        }
    }
    body
}

fn add_sync_systems_method(fields: &Vec<AttrField>, protocol_name: &Ident) -> TokenStream {
    let mut prediction_body = quote! {};
    let mut prepare_interpolation_body = quote! {};
    let mut interpolation_body = quote! {};
    for field in fields {
        let Some(sync) = &field.sync else {
            continue;
        };
        let component_type = &field.ty;
        // we only add sync systems if the SyncComponent is not None
        prediction_body = quote! {
            #prediction_body
            add_prediction_systems::<#component_type, #protocol_name>(app);
        };
        prepare_interpolation_body = quote! {
            #prepare_interpolation_body
            add_prepare_interpolation_systems::<#component_type, #protocol_name>(app);
        };
        if sync.mode == SyncMode::Full {
            interpolation_body = quote! {
                #interpolation_body
                add_interpolation_systems::<#component_type, #protocol_name>(app);
            };
        }
    }
    quote! {
        fn add_prediction_systems(app: &mut App)
        {
            #prediction_body
        }
        fn add_prepare_interpolation_systems(app: &mut App)
        {
            #prepare_interpolation_body
        }
        fn add_interpolation_systems(app: &mut App)
        {
            #interpolation_body
        }
    }
}

fn encode_method() -> TokenStream {
    quote! {
        fn encode(&self, writer: &mut WriteWordBuffer) -> anyhow::Result<()> {
            writer.serialize(&self)
        }
    }
}

fn decode_method() -> TokenStream {
    quote! {
        fn decode(reader: &mut ReadWordBuffer) -> anyhow::Result<Self>
            where Self: Sized{
            reader.deserialize::<Self>()
        }
    }
}

fn get_enum_kind(input: &ItemEnum, enum_kind_name: &Ident) -> TokenStream {
    // we use the original enum's names for the kind enum
    let variants = input.variants.iter().map(|v| v.ident.clone());
    quote! {
        pub enum #enum_kind_name {
            #(#variants),*
        }
    }
}

fn from_method(input: &ItemEnum, enum_kind_name: &Ident, fields: &Vec<Field>) -> TokenStream {
    let enum_name = &input.ident;
    let mut from_type_body = quote! {};
    let mut body = quote! {};
    for field in fields {
        let ident = &field.ident;
        let ty = &field.ty;
        from_type_body = quote! {
            #from_type_body
            impl FromType<#ty> for #enum_kind_name {
                fn from_type() -> Self {
                    #enum_kind_name::#ident
                }
            }
        };
        body = quote! {
            #body
            &#enum_name::#ident(..) => #enum_kind_name::#ident,
        }
    }

    quote! {
        #from_type_body
        impl<'a> From<&'a #enum_name> for #enum_kind_name {
            fn from(value: &'a #enum_name) -> Self {
                match value {
                    #body
                }
            }
        }
        impl From<#enum_name> for #enum_kind_name {
            fn from(value: #enum_name) -> Self {
                #enum_kind_name::from(&value)
            }
        }
    }
}

fn remove_method(input: &ItemEnum, fields: &[Field], enum_kind_name: &Ident) -> TokenStream {
    let component_kind_names = input.variants.iter().map(|v| &v.ident);
    let component_types = fields.iter().map(|field| &field.ty);

    let mut field_body = quote! {};
    for (component_type, component_kind_name) in component_types.zip(component_kind_names) {
        field_body = quote! {
            #field_body
            #enum_kind_name::#component_kind_name => entity.remove::<#component_type>(),
        };
    }
    quote! {
        fn remove(self, entity: &mut EntityWorldMut) {
            match self {
                #field_body
            };
        }
    }
}

// fn mode_method(input: &ItemEnum, fields: &Vec<AttrField>) -> TokenStream {
//     let mut body = quote! {};
//     for field in fields {
//         let ident = &field.ident;
//         let component_type = &field.ty;
//         let mode = field.
//         body = quote! {
//             #body
//             Self::#ident(_) => <#component_type>::mode(),
//         };
//     }
//
//     quote! {
//         fn mode(&self) -> ComponentSyncMode {
//             match self {
//                 #body
//             }
//         }
//     }
// }

fn map_entities_method(
    fields: &Vec<AttrField>,
    input: &ItemEnum,
    enum_kind_name: &Ident,
) -> TokenStream {
    let enum_name = &input.ident;
    let mut map_entities_body = quote! {};
    let mut external_mapper_body = quote! {};
    for field in fields.iter() {
        let component_type = &field.ty;
        let ident = &field.ident;
        match field.map_entities {
            MapField::NoMapEntities => {
                // if there is no mapping, still implement the ExternalMapper trait which is used to perform the mapping on the component directly
                // if there's no map entities, no need to do any mapping.
                external_mapper_body = quote! {
                    #external_mapper_body
                    impl ExternalMapper<#component_type> for #enum_name {
                        fn map_entities_for<M: EntityMapper>(x: &mut #component_type, entity_mapper: &mut M) {}
                    }
                };
                map_entities_body = quote! {
                    #map_entities_body
                    Self::#ident(ref mut x) => {},
                };
            }
            MapField::Custom => {
                map_entities_body = quote! {
                    #map_entities_body
                    Self::#ident(ref mut x) => x.map_entities(entity_mapper),
                };
            }
            MapField::MapWithMapEntity => {
                // if there is an MapEntities defined on the component, we use it for ExternalMapper
                external_mapper_body = quote! {
                    #external_mapper_body
                    impl ExternalMapper<#component_type> for #enum_name {
                        fn map_entities_for<M: EntityMapper>(x: &mut #component_type, entity_mapper: &mut M) {
                            x.map_entities(entity_mapper);
                        }
                    }
                };
                map_entities_body = quote! {
                    #map_entities_body
                    Self::#ident(ref mut x) => x.map_entities(entity_mapper),
                };
            }
        }
    }

    // TODO: make it work with generics (generic components)
    quote! {
        #external_mapper_body
        impl MapEntities for #enum_name {
            fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
                match self {
                    #map_entities_body
                }
            }
        }
    }
}

fn insert_method(input: &ItemEnum, fields: &Vec<Field>) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let ident = &field.ident;
        let component_type = &field.ty;
        body = quote! {
            #body
            Self::#ident(x) => {
                entity.insert(x);
            }
        };

        // let is_wrapped = match component_type {
        //     Type::Path(path) => path.path.segments.first().unwrap().ident.to_string() == "Wrapper",
        //     _ => false,
        // };
        //
        // if is_wrapped {
        //     body = quote! {
        //         #body
        //         Self::#ident(x) => {
        //             entity.insert(x.0);
        //         }
        //     };
        // } else {
        //     body = quote! {
        //         #body
        //         Self::#ident(x) => {
        //             entity.insert(x);
        //         }
        //     };
        // }
    }

    quote! {
        fn insert(self, entity: &mut EntityWorldMut) {
            match self {
                #body
            }
        }
    }
}

fn update_method(input: &ItemEnum, fields: &Vec<Field>) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let ident = &field.ident;
        let component_type = &field.ty;
        body = quote! {
            #body
            Self::#ident(x) => {
                 if let Some(mut c) = entity.get_mut::<#component_type>() {
                    *c = x;
                 } else {
                    entity.insert(x);
                }
            }
        };
        // let is_wrapped = match component_type {
        //     Type::Path(path) => path.path.segments.first().unwrap().ident.to_string() == "Wrapper",
        //     _ => false,
        // };
        //
        // if is_wrapped {
        //     let inner = match component_type {
        //         Type::Path(path) => match &path.path.segments.first().unwrap().arguments {
        //             PathArguments::AngleBracketed(generics) => generics.args.first().unwrap(),
        //             _ => panic!(),
        //         },
        //         _ => panic!(),
        //     };
        //     body = quote! {
        //         #body
        //         Self::#ident(x) => {
        //              if let Some(mut c) = entity.get_mut::<#inner>() {
        //                 *c = x.0;
        //              }
        //         }
        //     };
        // } else {
        //     body = quote! {
        //         #body
        //         Self::#ident(x) => {
        //              if let Some(mut c) = entity.get_mut::<#component_type>() {
        //                 *c = x;
        //              }
        //         }
        //     };
        // }
    }

    quote! {
        fn update(self, entity: &mut EntityWorldMut) {
            match self {
                #body
            }
        }
    }
}

fn type_ids_method(fields: &Vec<Field>, enum_kind_name: &Ident) -> TokenStream {
    let mut body = quote! {
        let mut res = HashMap::default();
    };
    for field in fields {
        let component_type = &field.ty;
        body = quote! {
            #body
            res.insert(TypeId::of::<#component_type>(), <#enum_kind_name as FromType<#component_type>>::from_type());
        };
    }
    quote! {
        fn type_ids() -> HashMap<TypeId, #enum_kind_name> {
            #body
            res
        }
    }
}
