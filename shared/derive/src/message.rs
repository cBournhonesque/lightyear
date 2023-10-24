use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{parse_macro_input, DeriveInput, ItemEnum};

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
    let item = parse_macro_input!(input as ItemEnum);

    // Helper Properties

    // Names
    let enum_name = &item.ident;
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    // Methods
    let encode_method = encode_method();
    let decode_method = decode_method();

    let output = quote! {
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use #shared_crate_name::{enum_delegate, EnumAsInner};
            use #shared_crate_name::{ReadBuffer, WriteBuffer, BitSerializable, MessageBehaviour,
                MessageProtocol, MessageKind};

            #[derive(Serialize, Deserialize, Clone)]
            #[enum_delegate::implement(MessageBehaviour)]
            // #[derive(EnumAsInner)]
            #item

            impl MessageProtocol for #enum_name {}
            // impl BitSerializable for #enum_name {
            //     #encode_method
            //     #decode_method
            // }
        }
        pub use #module_name::#enum_name as #enum_name;

    };

    proc_macro::TokenStream::from(output)
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
