use crate::{
    error::SerdeErr,
    reader_writer::{BitReader, BitWrite},
    serde::Serde,
};
macro_rules! impl_reflect_tuple {
    {$($index:tt : $name:tt),*} => {
        impl<$($name : Serde,)*> Serde for ($($name,)*) {
            fn ser(&self, writer: &mut dyn BitWrite) {
                $(self.$index.ser(writer);)*
            }
            fn de(reader: &mut BitReader) -> Result<($($name,)*), SerdeErr> {
                Ok(($($name::de(reader)?, )*))
            }
        }
    }
}

impl_reflect_tuple! {0: A}
impl_reflect_tuple! {0: A, 1: B}
impl_reflect_tuple! {0: A, 1: B, 2: C}
impl_reflect_tuple! {0: A, 1: B, 2: C, 3: D}
impl_reflect_tuple! {0: A, 1: B, 2: C, 3: D, 4: E}
impl_reflect_tuple! {0: A, 1: B, 2: C, 3: D, 4: E, 5: F}
impl_reflect_tuple! {0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G}
impl_reflect_tuple! {0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H}
impl_reflect_tuple! {0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I}
impl_reflect_tuple! {0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J}
impl_reflect_tuple! {0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K}
impl_reflect_tuple! {0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L}

// Tests

#[cfg(test)]
mod tests {
    use crate::{
        reader_writer::{BitReader, BitWriter},
        serde::Serde,
    };

    #[test]
    fn read_write() {
        // Write
        let mut writer = BitWriter::new();

        let in_1 = (true, -7532, "Hello tuple!".to_string(), Some(5));
        #[allow(unused_parens)]
        let in_2 = (5);
        let in_3 = (true, false, true, None, 4815, "Tuples tuples..".to_string());
        let in_4 = (332, "Goodbye tuple...".to_string());

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);
        in_3.ser(&mut writer);
        in_4.ser(&mut writer);

        let (buffer_length, buffer) = writer.flush();

        // Read

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        let out_1 = Serde::de(&mut reader).unwrap();
        let out_2 = Serde::de(&mut reader).unwrap();
        let out_3: (bool, bool, bool, Option<String>, u16, String) =
            Serde::de(&mut reader).unwrap();
        let out_4 = Serde::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
        assert_eq!(in_3, out_3);
        assert_eq!(in_4, out_4);
    }
}
