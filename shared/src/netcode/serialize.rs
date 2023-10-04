use std::io;

#[inline]
pub fn read_u64(src: &mut impl io::Read) -> Result<u64, io::Error> {
    let mut buffer = [0u8; 8];
    src.read_exact(&mut buffer)?;
    Ok(u64::from_le_bytes(buffer))
}

#[inline]
pub fn read_u32(src: &mut impl io::Read) -> Result<u32, io::Error> {
    let mut buffer = [0u8; 4];
    src.read_exact(&mut buffer)?;
    Ok(u32::from_le_bytes(buffer))
}

#[inline]
pub fn read_u16(src: &mut impl io::Read) -> Result<u16, io::Error> {
    let mut buffer = [0u8; 2];
    src.read_exact(&mut buffer)?;
    Ok(u16::from_le_bytes(buffer))
}

#[inline]
pub fn read_u8(src: &mut impl io::Read) -> Result<u8, io::Error> {
    let mut buffer = [0u8; 1];
    src.read_exact(&mut buffer)?;
    Ok(u8::from_le_bytes(buffer))
}

#[inline]
pub fn read_bytes<const N: usize>(src: &mut impl io::Read) -> Result<[u8; N], io::Error> {
    let mut data = [0u8; N];
    src.read_exact(&mut data)?;
    Ok(data)
}

#[inline]
pub fn read_i32(src: &mut impl io::Read) -> Result<i32, io::Error> {
    let mut buffer = [0u8; 4];
    src.read_exact(&mut buffer)?;
    Ok(i32::from_le_bytes(buffer))
}
