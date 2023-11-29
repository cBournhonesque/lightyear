use proc_macro2::{Ident, Span};
use syn::{Data, DeriveInput, Fields};

pub enum StructType {
    Struct,
    UnitStruct,
    TupleStruct,
}

/// Get the type of the struct
pub(crate) fn get_struct_type(input: &DeriveInput) -> StructType {
    if let Data::Struct(data_struct) = &input.data {
        return match &data_struct.fields {
            Fields::Named(_) => StructType::Struct,
            Fields::Unnamed(_) => StructType::TupleStruct,
            Fields::Unit => StructType::UnitStruct,
        };
    }
    panic!("Can only derive on a struct")
}

pub(crate) fn generate_unique_ident(prefix: &str) -> Ident {
    let uuid = uuid::Uuid::new_v4();
    let ident = format!("{}_{}", prefix, uuid).replace('-', "_");

    Ident::new(&ident, Span::call_site())
}
