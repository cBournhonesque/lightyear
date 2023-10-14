use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

pub fn component_impl(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    // Helper Properties
    // let struct_type = get_struct_type(&input);
    // let fields = get_fields(&input);

    // Names
    let struct_name = input.ident;

    // Methods
    let encode_method = encode_method();
    let decode_method = decode_method();

    let gen = quote! {
        impl ComponentProtocol for #struct_name {}
        // impl BitSerializable for #struct_name {
        //     #encode_method
        //     #decode_method
        // }
    };

    proc_macro::TokenStream::from(gen)
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
    let struct_name = input.ident;

    // Methods
    let gen = quote! {
        impl ComponentProtocolKind for #struct_name {}
    };
    proc_macro::TokenStream::from(gen)
}
