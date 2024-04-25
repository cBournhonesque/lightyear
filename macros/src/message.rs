use crate::shared::{generate_unique_ident, get_fields, strip_attributes};
use darling::ast::NestedMeta;
use darling::util::PathList;
use darling::{Error, FromDeriveInput, FromField, FromMeta};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use std::ops::Deref;
use syn::{
    parse_macro_input, parse_quote, parse_quote_spanned, DeriveInput, Field, Fields, GenericParam,
    Generics, ItemEnum, LifetimeParam, LitStr, Type,
};

const ATTRIBUTES: &[&str] = &["protocol"];

#[derive(Debug, FromMeta)]
struct MessageProtocolAttrs {
    protocol: Ident,
    #[darling(default)]
    derive: PathList,
}

#[derive(Debug, FromField)]
#[darling(attributes(protocol))]
struct AttrField {
    ident: Option<Ident>,
    ty: Type,
    #[darling(default)]
    map_entities: MapField,
}

#[derive(Debug, Default, FromMeta, PartialEq, Eq)]
enum MapField {
    #[default]
    NoMapEntities,
    Custom,
    #[darling(word)]
    MapWithMapEntity,
}

pub fn message_protocol_impl(
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
    let attr = match MessageProtocolAttrs::from_list(&attr_args) {
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
        InputMessage(#shared_crate_name::inputs::native::InputMessage<<#protocol as Protocol>::Input>)
    });

    #[cfg(feature = "leafwing")]
    for i in 1..3 {
        let variant = Ident::new(&format!("LeafwingInput{}Message", i), Span::call_site());
        let ty = Ident::new(&format!("LeafwingInput{}", i), Span::call_site());
        input.variants.push(parse_quote! {
            #[protocol(map_entities)]
            #variant(#shared_crate_name::inputs::leafwing::InputMessage<<#protocol as Protocol>::#ty>)
        });
    }

    // Helper Properties
    let fields = get_fields(&input);
    let input_without_attributes = strip_attributes(&input, ATTRIBUTES);
    let fields: Vec<AttrField> = fields
        .iter()
        .map(|field| FromField::from_field(field).unwrap())
        .collect();

    // Names
    let enum_name = &input.ident;
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    // Methods
    let from_into_impl = from_into_impl(&input, &fields);
    let message_kind_method = message_kind_method(&input, &fields);
    let input_message_kind_method = input_message_kind_method(&input);
    let add_events_method = add_events_method(&fields);
    let push_message_events_method = push_message_events_method(&fields, protocol);
    let name_method = name_method(&input, &fields);
    let map_entities_impl = map_entities_impl(&input, &fields);
    let encode_method = encode_method();
    let decode_method = decode_method();

    let output = quote! {
        #[doc(hidden)]
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use bevy::prelude::{App, Entity, World};
            use bevy::ecs::entity::{MapEntities, EntityMapper};
            use #shared_crate_name::_reexport::*;
            use #shared_crate_name::prelude::*;
            use #shared_crate_name::shared::events::systems::push_message_events;

            #[derive(Serialize, Deserialize, Clone, PartialEq)]
            #extra_derives
            #input_without_attributes

            impl MessageProtocol for #enum_name {
                type Protocol = #protocol;

                #name_method
                #message_kind_method
                #input_message_kind_method
                #add_events_method
                #push_message_events_method
            }

            #from_into_impl

            impl std::fmt::Debug for #enum_name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                    self.name().fmt(f)
                }
            }

            // #from_into_methods
            #map_entities_impl
            // impl BitSerializable for #enum_name {
            //     #encode_method
            //     #decode_method
            // }
        }
        pub use #module_name::#enum_name as #enum_name;

    };

    proc_macro::TokenStream::from(output)
}

fn push_message_events_method(fields: &Vec<AttrField>, protocol_name: &Ident) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let message_type = &field.ty;
        body = quote! {
            #body
            push_message_events::<#message_type, #protocol_name, E, Ctx>(world, events);
        };
    }
    quote! {
        fn push_message_events<E: IterMessageEvent<#protocol_name, Ctx>, Ctx: EventContext>(
            world: &mut World,
            events: &mut E
        )
        {
            #body
        }
    }
}

fn from_into_impl(input: &ItemEnum, fields: &Vec<AttrField>) -> TokenStream {
    let enum_name = &input.ident;
    let mut body = quote! {};
    for field in fields.iter() {
        let ident = &field.ident;
        let ty = &field.ty;
        body = quote! {
            #body
            impl From<#ty> for #enum_name {
                fn from(value: #ty) -> Self {
                    #enum_name::#ident(value)
                }
            }
            impl TryInto<#ty> for #enum_name {
                type Error = ();
                fn try_into(self) -> Result<#ty, Self::Error> {
                    match self {
                        #enum_name::#ident(inner) => Ok(inner),
                        _ => Err(()),
                    }
                }
            }
            impl<'a> TryInto<&'a #ty> for &'a #enum_name {
                type Error = ();
                fn try_into(self) -> Result<&'a #ty, Self::Error> {
                    match self {
                        #enum_name::#ident(inner) => Ok(inner),
                        _ => Err(()),
                    }
                }
            }
            impl<'a> TryInto<&'a mut #ty> for &'a mut #enum_name {
                type Error = ();
                fn try_into(self) -> Result<&'a mut #ty, Self::Error> {
                    match self {
                        #enum_name::#ident(inner) => Ok(inner),
                        _ => Err(()),
                    }
                }
            }
        };
    }

    body
}

fn message_kind_method(input: &ItemEnum, fields: &Vec<AttrField>) -> TokenStream {
    let enum_name = &input.ident;
    let mut body = quote! {};
    for field in fields.iter() {
        let ident = &field.ident;
        let ty = &field.ty;
        body = quote! {
            #body
            &#enum_name::#ident(_) => MessageKind::of::<#ty>(),
        };
    }

    quote! {
        fn kind(&self) -> MessageKind {
            match self {
                #body
            }
        }
    }
}

fn input_message_kind_method(input: &ItemEnum) -> TokenStream {
    let enum_name = &input.ident;
    let variants = input.variants.iter().map(|v| v.ident.clone());
    let mut body = quote! {};
    for variant in input.variants.iter() {
        let ident = &variant.ident;
        let variant_name = ident.to_string();
        if variant_name.starts_with("Input") && variant_name.ends_with("Message") {
            body = quote! {
                #body
                &#enum_name::#ident(_) => InputMessageKind::Native,
            };
        } else if variant_name.starts_with("Leafwing") && variant_name.ends_with("Message") {
            body = quote! {
                #body
                &#enum_name::#ident(_) => InputMessageKind::Leafwing,
            };
        } else {
            body = quote! {
                #body
                &#enum_name::#ident(_) => InputMessageKind::None,
            };
        }
    }

    quote! {
        fn input_message_kind(&self) -> InputMessageKind {
            match self {
                #body
            }
        }
    }
}

fn add_events_method(fields: &Vec<AttrField>) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let component_type = &field.ty;
        body = quote! {
            #body
            app.add_event::<MessageEvent<#component_type, Ctx>>();
        };
    }
    quote! {
        fn add_events<Ctx: EventContext>(app: &mut App)
        {
            #body
        }
    }
}

fn name_method(input: &ItemEnum, fields: &Vec<AttrField>) -> TokenStream {
    let enum_name = &input.ident;
    let mut body = quote! {};
    for field in fields.iter() {
        let ident = field.ident.as_ref().unwrap();
        let name = LitStr::new(&ident.to_string(), Span::call_site());
        body = quote! {
            #body
            &#enum_name::#ident(ref x) => #name,
        };
    }
    quote! {
        fn name(&self) -> &'static str {
            match self {
                #body
            }
        }
    }
}

fn map_entities_impl(input: &ItemEnum, fields: &Vec<AttrField>) -> TokenStream {
    let enum_name = &input.ident;
    let mut map_entities_body = quote! {};
    let mut external_mapper_body = quote! {};
    for field in fields.iter() {
        let message_type = &field.ty;
        let ident = &field.ident;
        match &field.map_entities {
            MapField::NoMapEntities => {
                // if there is no mapping, still implement the ExternalMapper trait which is used to perform the mapping on the component directly
                // if there's no map entities, no need to do any mapping.
                external_mapper_body = quote! {
                    #external_mapper_body
                    impl ExternalMapper<#message_type> for #enum_name {
                        fn map_entities_for<M: EntityMapper>(x: &mut #message_type, entity_mapper: &mut M) {}
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
                    Self::#ident(ref mut x) => <Self as ExternalMapper<#message_type>>::map_entities_for(x, entity_mapper),
                };
            }
            MapField::MapWithMapEntity => {
                // if there is an MapEntities defined on the component, we use it for ExternalMapper
                external_mapper_body = quote! {
                    #external_mapper_body
                    impl ExternalMapper<#message_type> for #enum_name {
                        fn map_entities_for<M: EntityMapper>(x: &mut #message_type, entity_mapper: &mut M) {
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
