use darling::ast::NestedMeta;
use darling::{Error, FromMeta};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{parse_macro_input, Field, Fields, ItemEnum, Variant};

#[derive(Debug, FromMeta)]
struct MacroAttrs {
    protocol: Ident,
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
    let input = parse_macro_input!(input as ItemEnum);

    // Helper Properties
    let fields = get_fields(&input);

    // Names
    let enum_name = &input.ident;
    let enum_kind_name = format_ident!("{}Kind", enum_name);
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    // Methods
    let add_systems_method = add_per_component_replication_send_systems_method(&fields, protocol);
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
            use bevy::prelude::{App, IntoSystemConfigs, EntityWorldMut};
            use #shared_crate_name::{ReadBuffer, WriteBuffer, BitSerializable,
                ComponentProtocol, ComponentBehaviour, ComponentProtocolKind, IntoKind, PostUpdate, Protocol,
                ComponentKindBehaviour, ReplicationSet, ReplicationSend};
            use #shared_crate_name::plugin::systems::replication::add_per_component_replication_send_systems;

            #[derive(Serialize, Deserialize, Clone)]
            #[enum_delegate::implement(ComponentBehaviour)]
            #input

            impl ComponentProtocol for #enum_name {
                type Protocol = #protocol;

                #add_systems_method
            }

            #[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
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

fn add_per_component_replication_send_systems_method(
    fields: &Vec<&Field>,
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

fn into_kind_method(input: &ItemEnum, fields: &Vec<&Field>, enum_kind_name: &Ident) -> TokenStream {
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

fn remove_method(input: &ItemEnum, fields: &Vec<&Field>, enum_kind_name: &Ident) -> TokenStream {
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
