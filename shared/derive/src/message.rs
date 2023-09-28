use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Fields, Type};

use super::shared::{get_struct_type, StructType};

pub fn message_impl(
    input: proc_macro::TokenStream,
    shared_crate_name: TokenStream,
) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    // Helper Properties
    let struct_type = get_struct_type(&input);
    let fields = get_fields(&input);

    // Names
    let struct_name = input.ident;
    let lowercase_struct_name = Ident::new(
        struct_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);
    let builder_name = format_ident!("{}Builder", struct_name);

    // Methods
    let decode_method = decode_method(&struct_name);
    let get_builder_method = get_builder_method(&builder_name);

    let gen = quote! {
        mod #module_name {
            pub use bitcode::Buffer;
            pub use #shared_crate_name::{Message, MessageBuilder, MessageContainer};
            use super::*;

            struct #builder_name;
            impl MessageBuilder for #builder_name {
                #decode_method
            }

            impl Message for #struct_name {
                #get_builder_method
            }
        }
    };

    proc_macro::TokenStream::from(gen)
}

fn decode_method(struct_name: &Ident) -> TokenStream {
    quote! {
        fn decode(&self, registry: &MessageRegistry, reader: &mut impl Read) -> anyhow::Result<MessageContainer> {
            let id = <Option<MessageId> as Decode>::decode(Fixed, reader)?;
            let message = <#struct_name as Decode>::decode(Fixed, reader)?;
            MessageContainer{
                id,
                message: Box::New(message),
            }
        }
    }
}

fn encode_method(struct_name: &Ident) -> TokenStream {
    quote! {
        fn encode(&self, buffer: bitcode::Buffer) -> anyhow::Result<&[u8]> {
            buffer.encode::<#struct_name>(&self)?;
        }
    }
}

fn get_builder_method(builder_name: &Ident) -> TokenStream {
    quote! {
        fn get_builder() -> Box<dyn MessageBuilder> where Self: Sized {
            Box::new(#builder_name)
        }
    }
}

const UNNAMED_FIELD_PREFIX: &'static str = "unnamed_field_";
fn get_variable_name_for_unnamed_field(index: usize, span: Span) -> Ident {
    Ident::new(&format!("{}{}", UNNAMED_FIELD_PREFIX, index), span)
}

pub struct EntityProperty {
    pub variable_name: Ident,
    pub uppercase_variable_name: Ident,
}

pub struct Normal {
    pub variable_name: Ident,
    pub field_type: Type,
}
pub enum Field {
    EntityProperty(EntityProperty),
    Normal(Normal),
}

impl Field {
    pub fn entity_property(variable_name: Ident) -> Self {
        Self::EntityProperty(EntityProperty {
            variable_name: variable_name.clone(),
            uppercase_variable_name: Ident::new(
                variable_name.to_string().to_uppercase().as_str(),
                Span::call_site(),
            ),
        })
    }

    pub fn normal(variable_name: Ident, field_type: Type) -> Self {
        Self::Normal(Normal {
            variable_name: variable_name.clone(),
            field_type,
        })
    }

    pub fn variable_name(&self) -> &Ident {
        match self {
            Self::EntityProperty(property) => &property.variable_name,
            Self::Normal(field) => &field.variable_name,
        }
    }
}

fn get_fields(input: &DeriveInput) -> Vec<Field> {
    let mut fields = Vec::new();

    if let Data::Struct(data_struct) = &input.data {
        match &data_struct.fields {
            Fields::Named(fields_named) => {
                for field in fields_named.named.iter() {
                    if let Some(variable_name) = &field.ident {
                        match &field.ty {
                            Type::Path(type_path) => {
                                if let Some(property_seg) = type_path.path.segments.first() {
                                    let property_type = property_seg.ident.clone();
                                    // EntityProperty
                                    if property_type == "EntityProperty" {
                                        fields.push(Field::entity_property(variable_name.clone()));
                                        continue;
                                        // Property
                                    } else {
                                        fields.push(Field::normal(
                                            variable_name.clone(),
                                            field.ty.clone(),
                                        ));
                                    }
                                }
                            }
                            _ => {
                                fields.push(Field::normal(variable_name.clone(), field.ty.clone()));
                            }
                        }
                    }
                }
            }
            Fields::Unnamed(fields_unnamed) => {
                for (index, field) in fields_unnamed.unnamed.iter().enumerate() {
                    if let Type::Path(type_path) = &field.ty {
                        if let Some(property_seg) = type_path.path.segments.first() {
                            let property_type = property_seg.ident.clone();
                            let variable_name =
                                get_variable_name_for_unnamed_field(index, property_type.span());
                            if property_type == "EntityProperty" {
                                fields.push(Field::entity_property(variable_name));
                                continue;
                            } else {
                                fields.push(Field::normal(variable_name, field.ty.clone()))
                            }
                        }
                    }
                }
            }
            Fields::Unit => {}
        }
    } else {
        panic!("Can only derive Replicate on a struct");
    }

    fields
}
