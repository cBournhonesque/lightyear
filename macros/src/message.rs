use crate::shared::generate_unique_ident;
use darling::ast::NestedMeta;
use darling::util::PathList;
use darling::{Error, FromDeriveInput, FromMeta};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use std::ops::Deref;
use syn::{
    parse_macro_input, parse_quote, parse_quote_spanned, DeriveInput, Field, Fields, GenericParam,
    Generics, ItemEnum, LifetimeParam, LitStr,
};

#[derive(Debug, FromDeriveInput)]
#[darling(attributes(message))]
struct MessageAttrs {
    #[darling(default)]
    custom_map: bool,
}
pub fn message_impl(
    input: proc_macro::TokenStream,
    shared_crate_name: TokenStream,
) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let attrs = <MessageAttrs as FromDeriveInput>::from_derive_input(&input).unwrap();

    // Helper Properties
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    let map_entities = !attrs.custom_map;

    // Names
    let struct_name = &input.ident;
    // let struct_name_str = LitStr::new(&struct_name.to_string(), struct_name.span());
    let struct_name_str = &struct_name.to_string();
    let lowercase_struct_name =
        format_ident!("{}", struct_name.to_string().to_lowercase().as_str());
    let module_name = generate_unique_ident(&format!("mod_{}", lowercase_struct_name));
    let map_entities_trait = map_entities_trait(&input, map_entities);

    // Methods
    let gen = quote! {
        pub mod #module_name {
            use super::*;
            use bevy::prelude::*;
            use bevy::utils::{EntityHashMap, EntityHashSet};
            use #shared_crate_name::prelude::*;

            impl #impl_generics Message for #struct_name #type_generics #where_clause {}

            #map_entities_trait

            // TODO: maybe we should just be able to convert a message into a MessageKind, and impl Display/Debug on MessageKind?
            impl #impl_generics Named for #struct_name #type_generics #where_clause {
                fn name(&self) -> &'static str {
                    #struct_name_str
                }
            }
        }
    };

    proc_macro::TokenStream::from(gen)
}

// Add the MapEntities trait for the message.
// Need to combine the generics from the message with the generics from the trait
fn map_entities_trait(input: &DeriveInput, ident_map: bool) -> TokenStream {
    // combined generics
    let mut gen_clone = input.generics.clone();
    let lt: LifetimeParam = parse_quote_spanned! {Span::mixed_site() => 'a};
    gen_clone.params.push(GenericParam::from(lt));
    let (impl_generics, _, _) = gen_clone.split_for_impl();

    // type generics
    let (_, type_generics, where_clause) = input.generics.split_for_impl();

    // trait generics (MapEntities)
    let mut map_entities_generics = Generics::default();
    let lt: LifetimeParam = parse_quote_spanned! {Span::mixed_site() => 'a};
    map_entities_generics.params.push(GenericParam::from(lt));
    let (_, type_generics_map, _) = map_entities_generics.split_for_impl();

    let struct_name = &input.ident;
    if ident_map {
        quote! {
            impl #impl_generics MapEntities #type_generics_map for #struct_name #type_generics #where_clause {
                fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}
                fn entities(&self) -> EntityHashSet<Entity> {
                    EntityHashSet::default()
                }
            }
        }
    } else {
        quote! {}
    }
}

#[derive(Debug, FromMeta)]
struct MessageProtocolAttrs {
    protocol: Ident,
    #[darling(default)]
    derive: PathList,
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
            #variant(#shared_crate_name::inputs::leafwing::InputMessage<<#protocol as Protocol>::#ty>)
        });
    }

    // Helper Properties
    let fields = get_fields(&input);

    // Names
    let enum_name = &input.ident;
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    // Methods
    let input_message_kind_method = input_message_kind_method(&input);
    let add_events_method = add_events_method(&fields);
    let push_message_events_method = push_message_events_method(&fields, protocol);
    let delegate_method = delegate_method(&input);
    let encode_method = encode_method();
    let decode_method = decode_method();

    let from_into_methods = from_into_methods(&input, &fields, enum_name);

    let output = quote! {
        #[doc(hidden)]
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use bevy::prelude::{App, Entity, World};
            use bevy::utils::{EntityHashMap, EntityHashSet};
            use #shared_crate_name::_reexport::*;
            use #shared_crate_name::prelude::*;
            use #shared_crate_name::connection::events::{IterMessageEvent};
            use #shared_crate_name::shared::systems::events::push_message_events;
            use #shared_crate_name::shared::events::MessageEvent;


            #[derive(Serialize, Deserialize, Clone, PartialEq)]
            #extra_derives
            #[enum_delegate::implement(MessageBehaviour)]
            // #[derive(EnumAsInner)]
            #input

            impl MessageProtocol for #enum_name {
                type Protocol = #protocol;

                #input_message_kind_method
                #add_events_method
                #push_message_events_method
            }

            impl std::fmt::Debug for #enum_name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                    self.name().fmt(f)
                }
            }

            // #from_into_methods
            #delegate_method
            // impl BitSerializable for #enum_name {
            //     #encode_method
            //     #decode_method
            // }
        }
        pub use #module_name::#enum_name as #enum_name;

    };

    proc_macro::TokenStream::from(output)
}

fn push_message_events_method(fields: &Vec<&Field>, protocol_name: &Ident) -> TokenStream {
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

fn add_events_method(fields: &Vec<&Field>) -> TokenStream {
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

fn delegate_method(input: &ItemEnum) -> TokenStream {
    let enum_name = &input.ident;
    let variants = input.variants.iter().map(|v| v.ident.clone());
    let mut name_body = quote! {};
    let mut map_entities_body = quote! {};
    let mut entities_body = quote! {};
    for variant in input.variants.iter() {
        let ident = &variant.ident;
        name_body = quote! {
            #name_body
            &#enum_name::#ident(ref x) => x.name(),
        };
        map_entities_body = quote! {
            #map_entities_body
            #enum_name::#ident(ref mut x) => x.map_entities(entity_mapper),
        };
        entities_body = quote! {
            #entities_body
            #enum_name::#ident(ref x) => x.entities(),
        };
    }

    quote! {
        impl Named for #enum_name {
            fn name(&self) -> &'static str {
                match self {
                    #name_body
                }
            }
        }
        impl<'a> MapEntities<'a> for #enum_name {
            fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {
                match self {
                    #map_entities_body
                }
            }
            fn entities(&self) -> EntityHashSet<Entity> {
                match self {
                    #entities_body
                }
            }
        }
    }
}

fn from_into_methods(input: &ItemEnum, fields: &[&Field], enum_name: &Ident) -> TokenStream {
    let enum_name = &input.ident;
    let variants = input.variants.iter().map(|v| v.ident.clone());
    let mut body = quote! {};
    for (variant, field) in input.variants.iter().zip(fields.iter()) {
        let ident = &variant.ident;
        body = quote! {
            #body
            impl From<#field> for #enum_name {
                fn from(value: #field) -> Self {
                    #enum_name::#ident(value)
                }
            }
            impl TryInto<#field> for #enum_name {
                type Error = ();
                fn try_into(self) -> Result<#field, Self::Error> {
                    match self {
                        #enum_name::#ident(x) => Ok(x),
                        _ => Err(()),
                    }
                }
            }
        }
    }

    quote! {
        #body
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

fn get_fields(input: &ItemEnum) -> Vec<&Field> {
    let mut fields = Vec::new();
    for field in &input.variants {
        let Fields::Unnamed(unnamed) = &field.fields else {
            panic!("Field must be unnamed");
        };
        assert_eq!(unnamed.unnamed.len(), 1);
        let component = unnamed.unnamed.first().unwrap();
        fields.push(component);
    }
    fields
}
