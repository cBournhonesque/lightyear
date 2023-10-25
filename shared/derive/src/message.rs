use darling::ast::NestedMeta;
use darling::{Error, FromMeta};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{parse_macro_input, DeriveInput, Field, Fields, ItemEnum};

#[derive(Debug, FromMeta)]
struct MacroAttrs {
    protocol: Ident,
}

pub fn message_impl(
    input: proc_macro::TokenStream,
    shared_crate_name: TokenStream,
) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    // Helper Properties
    // Names
    let struct_name = input.ident;

    // Methods
    let gen = quote! {
        impl Message for #struct_name {}
    };

    proc_macro::TokenStream::from(gen)
}

pub fn message_protocol_impl(
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
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    // Methods
    let push_message_events_method = push_message_events_method(fields, protocol);
    let encode_method = encode_method();
    let decode_method = decode_method();

    let output = quote! {
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use bevy::prelude::{World};
            use #shared_crate_name::{enum_delegate, EnumAsInner};
            use #shared_crate_name::{ReadBuffer, WriteBuffer, BitSerializable, MessageBehaviour,
                MessageProtocol, MessageKind};
            use #shared_crate_name::connection::events::{EventContext, IterMessageEvent};
            use #shared_crate_name::plugin::systems::events::push_message_events;

            #[derive(Serialize, Deserialize, Clone)]
            #[enum_delegate::implement(MessageBehaviour)]
            // #[derive(EnumAsInner)]
            #input

            impl MessageProtocol for #enum_name {
                type Protocol = #protocol;

                #push_message_events_method

            }
            // impl BitSerializable for #enum_name {
            //     #encode_method
            //     #decode_method
            // }
        }
        pub use #module_name::#enum_name as #enum_name;

    };

    proc_macro::TokenStream::from(output)
}

fn push_message_events_method(fields: Vec<&Field>, protocol_name: &Ident) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let component_type = &field.ty;
        body = quote! {
            #body
            push_message_events::<#component_type, #protocol_name, E, Ctx>(world, events);
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
