use darling::ast::NestedMeta;
use darling::{Error, FromMeta};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, parse_quote, Attribute, Data, DeriveInput, Field, Fields, ItemEnum, Path,
};

#[derive(Debug, FromMeta)]
struct MacroAttrs {
    protocol: Ident,
}

pub fn component_impl(
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
    let add_systems_method = add_systems_method(fields, &attr.protocol);
    let encode_method = encode_method();
    let decode_method = decode_method();

    let gen = quote! {
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use #shared_crate_name::{EnumKind, enum_delegate};
            use bevy::prelude::{App, IntoSystemConfigs};
            use bevy::ecs::world::EntityMut;
            use #shared_crate_name::{ReadBuffer, WriteBuffer, BitSerializable,
                ComponentProtocol, ComponentBehaviour, ComponentProtocolKind, PostUpdate, Protocol,
                ReplicationSet, ReplicationSend};
            use #shared_crate_name::plugin::systems::send_component_update;

            #[derive(EnumKind, Serialize, Deserialize, Clone)]
            #[enum_kind(#enum_kind_name, derive(Serialize, Deserialize, ComponentProtocolKind))]
            #[enum_delegate::implement(ComponentBehaviour)]
            #input

            impl ComponentProtocol for #enum_name {
                #add_systems_method
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

fn add_systems_method(fields: Vec<&Field>, protocol_name: &Ident) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let component_type = &field.ty;
        body = quote! {
            #body
            app.add_systems(PostUpdate, (
                send_component_update::<#component_type, #protocol_name, R>
            ).in_set(ReplicationSet::Send));
        };
    }
    quote! {
        fn add_systems<R: ReplicationSend<#protocol_name>>(app: &mut App)
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

pub fn component_kind_impl(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    // Names
    let enum_name = input.ident;

    // Methods
    let gen = quote! {
        impl ComponentProtocolKind for #enum_name {}
    };
    proc_macro::TokenStream::from(gen)
}
