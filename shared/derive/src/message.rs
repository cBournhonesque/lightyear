use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields, Type};

pub fn message_impl(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
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
        impl SerializableProtocol for #struct_name {
            #encode_method
            #decode_method
        }
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
