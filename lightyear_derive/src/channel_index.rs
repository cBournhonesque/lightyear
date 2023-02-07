use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput};

pub fn channel_index_impl(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let enum_name = if let Data::Enum(_) = &input.data {
        input.ident.clone()
    } else {
        unimplemented!("Only enums are supported");
    };

    let mod_channel_name = format_ident!("define_{}", enum_name);

    let gen = quote! {
        mod #mod_channel_name {
            use lightyear_serde::{Serde, ChannelIndex};
            #[derive(Clone, Hash, PartialEq, Eq, Serde)]
            #input

            impl ChannelIndex for #enum_name {}
        }
        pub use #mod_channel_name::#enum_name;
    };

    proc_macro::TokenStream::from(gen)
}
