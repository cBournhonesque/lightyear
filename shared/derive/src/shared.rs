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
