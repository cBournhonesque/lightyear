use lightyear_serde::reader::Reader;
use lightyear_serde::writer::Writer;
use no_std_io2::io::{Read, Write};

#[test]
fn reader_and_writer_share_the_same_api_across_feature_modes() {
    let mut writer = Writer::with_capacity(1);
    writer.write_all(&[1, 2, 3, 4]).unwrap();

    assert_eq!(writer.len(), 4);
    assert_eq!(writer.position(), 4);
    assert_eq!(writer.split_to(1).as_ref(), &[1]);
    assert_eq!(writer.len(), 3);

    let mut reader = Reader::from(writer.split());
    let mut first = [0; 1];
    reader.read_exact(&mut first).unwrap();

    assert_eq!(first, [2]);
    assert_eq!(reader.position(), 1);
    assert_eq!(reader.remaining(), 2);
    assert_eq!(reader.split_len(1).as_ref(), &[3]);
    assert_eq!(reader.split().as_ref(), &[4]);
    assert!(!reader.has_remaining());
}
