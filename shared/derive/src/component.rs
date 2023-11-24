use darling::ast::NestedMeta;
use darling::{Error, FromField, FromMeta};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{parse_macro_input, parse_quote, Field, Fields, ItemEnum, Type};

#[derive(Debug, FromMeta)]
/// Struct that will hold the value of attributes passed to the macro
struct MacroAttrs {
    protocol: Ident,
}

const ATTRIBUTES: &'static [&'static str] = &["sync"];

#[derive(Debug, FromField)]
#[darling(attributes(sync))]
struct SyncField {
    // name of the enum field
    ident: Option<Ident>,

    // type of the field
    ty: Type,

    #[darling(default)]
    full: bool,
    #[darling(default)]
    simple: bool,
    #[darling(default)]
    once: bool,

    #[darling(default)]
    lerp: Option<Ident>,
}

impl SyncField {
    fn get_mode_tokens(&self) -> TokenStream {
        let mut tokens = quote! {};
        if self.full {
            tokens = quote! {
                ComponentSyncMode::Full
            };
        } else if self.simple {
            tokens = quote! {
                ComponentSyncMode::Simple
            };
        } else if self.once {
            tokens = quote! {
                ComponentSyncMode::Once
            };
        }
        tokens
    }

    fn check_is_valid(&self) {
        let mut count = 0;
        if self.full {
            count += 1;
        }
        if self.simple {
            count += 1;
        }
        if self.once {
            count += 1;
        }
        if count != 1 {
            panic!(
                "The field {:?} cannot have multiple sync attributes set at the same time",
                self
            );
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
            return TokenStream::from(Error::from(e).write_errors()).into();
        }
    };
    let attr = match MacroAttrs::from_list(&attr_args) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(e.write_errors()).into();
        }
    };
    let protocol = &attr.protocol;

    let mut input = parse_macro_input!(input as ItemEnum);

    // Add extra variants
    input.variants.push(parse_quote! {
        ShouldBePredicted(ShouldBePredicted)
    });
    input.variants.push(parse_quote! {
        ShouldBeInterpolated(ShouldBeInterpolated)
    });

    // Helper Properties
    let fields = get_fields(&input);
    let input_without_attributes = strip_attributes(&input);

    // Use darling to parse the attributes for each field
    let sync_fields: Vec<SyncField> = fields
        .iter()
        .filter(|field| field.attrs.iter().any(|attr| attr.path().is_ident("sync")))
        .map(|field| FromField::from_field(&field).unwrap())
        .collect();
    for field in &sync_fields {
        field.check_is_valid();
    }

    // Names
    let enum_name = &input.ident;
    let enum_kind_name = format_ident!("{}Kind", enum_name);
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    // Impls
    let sync_component_impl = sync_component_impl(&sync_fields, protocol);

    // Methods
    let add_systems_method = add_per_component_replication_send_systems_method(&fields, protocol);
    let add_events_method = add_events_method(&fields);
    let push_component_events_method = push_component_events_method(&fields, protocol);
    let add_sync_systems_method = add_sync_systems_method(&sync_fields, protocol);
    let encode_method = encode_method();
    let decode_method = decode_method();

    // EnumKind methods
    let enum_kind = get_enum_kind(&input, &enum_kind_name);
    let from_method = from_method(&input, &enum_kind_name);
    let into_kind_method = into_kind_method(&input, &fields, &enum_kind_name);
    let remove_method = remove_method(&input, &fields, &enum_kind_name);

    let gen = quote! {
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use #shared_crate_name::{enum_delegate};
            use bevy::prelude::{App, IntoSystemConfigs, EntityWorldMut, World};
            use #shared_crate_name::{ReadBuffer, WriteBuffer, BitSerializable,
                ComponentProtocol, ComponentBehaviour, ComponentProtocolKind, IntoKind, PostUpdate, Protocol,
                ComponentKindBehaviour, ReplicationSet, ReplicationSend};
            use #shared_crate_name::plugin::systems::replication::add_per_component_replication_send_systems;
            use #shared_crate_name::connection::events::{EventContext, IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent};
            use #shared_crate_name::plugin::systems::events::{
                push_component_insert_events, push_component_remove_events, push_component_update_events,
            };
            use #shared_crate_name::plugin::events::{ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent};

            // TODO: write this behind feature?
            // TODO: possibility to rename this? maybe we should put everything in one crate
            use #shared_crate_name::client::components::{ComponentSyncMode, SyncComponent};
            use #shared_crate_name::client::prediction::{add_prediction_systems, ShouldBePredicted};
            use #shared_crate_name::client::interpolation::{add_interpolation_systems, add_lerp_systems, ShouldBeInterpolated,
                InterpolatedComponent, LerpMode};

            #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
            #[enum_delegate::implement(ComponentBehaviour)]
            #input_without_attributes

            impl ComponentProtocol for #enum_name {
                type Protocol = #protocol;

                #add_systems_method
                #add_events_method
                #push_component_events_method
                #add_sync_systems_method
            }

            #sync_component_impl

            #[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
            #enum_kind

            impl ComponentProtocolKind for #enum_kind_name {
                type Protocol = #protocol;
            }

            #into_kind_method
            #from_method

            impl ComponentKindBehaviour for #enum_kind_name {
                #remove_method
            }
            // TODO: we don't need to implement for now because we get it for free from Serialize + Deserialize
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

/// Get a copy of the type inside each enum variants
fn get_fields(input: &ItemEnum) -> Vec<Field> {
    let mut fields = Vec::new();
    for mut variant in input.variants.iter() {
        let Fields::Unnamed(ref unnamed) = variant.fields else {
            panic!("Field must be unnamed");
        };
        assert_eq!(unnamed.unnamed.len(), 1);
        let mut component = unnamed.unnamed.first().unwrap().clone();
        // get the attrs from the variant
        component.attrs = variant.attrs.clone();
        // make field immutable
        fields.push(component);
    }
    fields
}

/// Make a copy of the input enum but remove all the field attributes defined by me
fn strip_attributes(input: &ItemEnum) -> ItemEnum {
    let mut input = input.clone();
    for variant in input.variants.iter_mut() {
        // remove all attributes that are used in this macro
        variant.attrs.retain(|v| {
            v.path().segments.first().map_or(true, |s| {
                ATTRIBUTES
                    .iter()
                    .all(|attr| attr.to_string() != s.ident.to_string())
            })
        })
    }
    input
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

fn sync_component_impl(fields: &Vec<SyncField>, protocol_name: &Ident) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let component_type = &field.ty;
        let mode = field.get_mode_tokens();
        body = quote! {
            #body
            impl SyncComponent for #component_type {
                fn mode() -> ComponentSyncMode {
                    #mode
                }
            }
        };
        if field.full {
            let lerp_mode = if let Some(lerp_fn) = &field.lerp {
                quote! {
                    fn lerp_mode() -> LerpMode<#component_type> {
                        LerpMode::Custom(lerm_fn)
                    }
                }
            } else {
                quote! {
                    fn lerp_mode() -> LerpMode<#component_type> {
                        LerpMode::Linear
                    }
                }
            };
            body = quote! {
                #body
                impl InterpolatedComponent for #component_type {
                    #lerp_mode
                }
            }
        }
    }
    body
}

fn add_sync_systems_method(fields: &Vec<SyncField>, protocol_name: &Ident) -> TokenStream {
    let mut prediction_body = quote! {};
    let mut interpolation_body = quote! {};
    for field in fields {
        let component_type = &field.ty;
        prediction_body = quote! {
            #prediction_body
            add_prediction_systems::<#component_type, #protocol_name>(app);
        };
        interpolation_body = quote! {
            #interpolation_body
            add_interpolation_systems::<#component_type, #protocol_name>(app);
        };
        if field.full {
            interpolation_body = quote! {
                #interpolation_body
                add_lerp_systems::<#component_type, #protocol_name>(app);
            };
        }
    }
    quote! {
        fn add_prediction_systems(app: &mut App)
        {
            #prediction_body
        }
        fn add_interpolation_systems(app: &mut App)
        {
            #interpolation_body
        }
    }
}

fn encode_method() -> TokenStream {
    quote! {
        fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
            writer.serialize(&self)
        }
    }
}

fn decode_method() -> TokenStream {
    quote! {
        fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
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

fn from_method(input: &ItemEnum, enum_kind_name: &Ident) -> TokenStream {
    let enum_name = &input.ident;
    let variants = input.variants.iter().map(|v| v.ident.clone());
    let mut body = quote! {};
    for variant in input.variants.iter() {
        let ident = &variant.ident;
        body = quote! {
            #body
            &#enum_name::#ident(..) => #enum_kind_name::#ident,
        }
    }

    quote! {
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

fn into_kind_method(input: &ItemEnum, fields: &Vec<Field>, enum_kind_name: &Ident) -> TokenStream {
    let component_kind_names = input.variants.iter().map(|v| &v.ident);
    let component_types = fields.iter().map(|field| &field.ty);

    let mut field_body = quote! {};
    for (component_type, component_kind_name) in component_types.zip(component_kind_names) {
        field_body = quote! {
            #field_body
            impl IntoKind<#enum_kind_name> for #component_type {
                fn into_kind() -> #enum_kind_name {
                    #enum_kind_name::#component_kind_name
                }
            }
        };
    }
    field_body
}

fn remove_method(input: &ItemEnum, fields: &Vec<Field>, enum_kind_name: &Ident) -> TokenStream {
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
