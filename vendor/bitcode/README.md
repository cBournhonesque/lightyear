# Bitcode
[![Documentation](https://docs.rs/bitcode/badge.svg)](https://docs.rs/bitcode)
[![crates.io](https://img.shields.io/crates/v/bitcode.svg)](https://crates.io/crates/bitcode)
[![Build](https://github.com/SoftbearStudios/bitcode/actions/workflows/build.yml/badge.svg)](https://github.com/SoftbearStudios/bitcode/actions/workflows/build.yml)
[![unsafe forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/rust-secure-code/safety-dance/)

A bitwise encoder/decoder similar to [bincode](https://github.com/bincode-org/bincode), which attempts to shrink the serialized size without sacrificing speed (as would be the case with compression).

The format may change between major versions, so we are free to optimize it.

## Comparison with [bincode](https://github.com/bincode-org/bincode)

### Features

- Bitwise serialization
- [Gamma](https://en.wikipedia.org/wiki/Elias_gamma_coding) encoded lengths and enum variant indices

### Additional features with `#[derive(bitcode::Encode, bitcode::Decode)]`

- Enums use the fewest possible bits, e.g. an enum with 4 variants uses 2 bits
- Apply attributes to fields/enum variants:

| Attribute                                     | Type          | Result                                                                                                     |
|-----------------------------------------------|---------------|------------------------------------------------------------------------------------------------------------|
| `#[bitcode_hint(ascii)]`                      | String        | Uses 7 bits per character                                                                                  |
| `#[bitcode_hint(ascii_lowercase)]`            | String        | Uses 5 bits per character                                                                                  |
| `#[bitcode_hint(expected_range = "50..100"]`  | u8-u64        | Uses log2(range.end - range.start) bits                                                                    |
| `#[bitcode_hint(expected_range = "0.0..1.0"]` | f32/f64       | Uses ~25 bits for `f32` and ~54 bits for `f64`                                                             |
| `#[bitcode_hint(frequency = 123)`             | enum variant  | Frequent variants use fewer bits (see [Huffman coding](https://en.wikipedia.org/wiki/Huffman_coding))      |
| `#[bitcode_hint(gamma)]`                      | i8-i64/u8-u64 | Small integers use fewer bits (see [Elias gamma coding](https://en.wikipedia.org/wiki/Elias_gamma_coding)) |
| `#[bitcode(with_serde)]`                      | T: Serialize  | Uses `serde::Serialize` instead of `bitcode::Encode`                                                       |

### Limitations

- Doesn't support streaming APIs
- Format may change between major versions
- With `feature = "derive"`, types containing themselves must use `#[bitcode(recursive)]` to compile

## Benchmarks vs. [bincode](https://github.com/bincode-org/bincode) and [postcard](https://github.com/jamesmunns/postcard)

### Primitives (size in bits)

| Type                | Bitcode (derive) | Bitcode (serde) | Bincode | Bincode (varint) | Postcard |
|---------------------|------------------|-----------------|---------|------------------|----------|
| bool                | 1                | 1               | 8       | 8                | 8        |
| u8/i8               | 8                | 8               | 8       | 8                | 8        |
| u16/i16             | 16               | 16              | 16      | 8-24             | 8-24     |
| u32/i32             | 32               | 32              | 32      | 8-40             | 8-40     |
| u64/i64             | 64               | 64              | 64      | 8-72             | 8-80     |
| u128/i128           | 128              | 128             | 128     | 8-136            | 8-152    |
| usize/isize         | 64               | 64              | 64      | 8-72             | 8-80     |
| f32                 | 32               | 32              | 32      | 32               | 32       |
| f64                 | 64               | 64              | 64      | 64               | 64       |
| char                | 21               | 21              | 8-32    | 8-32             | 16-40    |
| Option<()>          | 1                | 1               | 8       | 8                | 8        |
| Result<(), ()>      | 1                | 1-3             | 32      | 8                | 8        |
| enum { A, B, C, D } | 2                | 1-5             | 32      | 8                | 8        |
| Duration            | 94               | 96              | 96      | 16-112           | 16-120   |

<sup>Note: These are defaults, and can be optimized with hints in the case of Bitcode (derive) or custom `impl Serialize` in the case of `serde` serializers.</sup>

### Values (size in bits)

| Value               | Bitcode (derive) | Bitcode (serde) | Bincode | Bincode (varint) | Postcard |
|---------------------|------------------|-----------------|---------|------------------|----------|
| [true; 4]           | 4                | 4               | 32      | 32               | 32       |
| vec![(); 0]         | 1                | 1               | 64      | 8                | 8        |
| vec![(); 1]         | 3                | 3               | 64      | 8                | 8        |
| vec![(); 256]       | 17               | 17              | 64      | 24               | 16       |
| vec![(); 65536]     | 33               | 33              | 64      | 40               | 24       |
| ""                  | 1                | 1               | 64      | 8                | 8        |
| "abcd"              | 37               | 37              | 96      | 40               | 40       |
| "abcd1234"          | 71               | 71              | 128     | 72               | 72       |


### Random [Structs and Enums](https://github.com/SoftbearStudios/bitcode/blob/2a47235eee64f4a7c49ad1841a5b509abd2d0e99/src/benches.rs#L16-L88) (average size and speed)

| Format                 | Size (bytes) | Serialize (ns) | Deserialize (ns) |
|------------------------|--------------|----------------|------------------|
| Bitcode (derive)       | 6.2          | 14             | 50               |
| Bitcode (serde)        | 6.7          | 18             | 59               |
| Bincode                | 20.3         | 17             | 61               |
| Bincode (varint)       | 10.9         | 26             | 68               |
| Bincode (LZ4)          | 9.9          | 58             | 73               |
| Bincode (Deflate Fast) | 8.4          | 336            | 279              |
| Bincode (Deflate Best) | 7.8          | 1990           | 275              |
| Postcard               | 10.7         | 21             | 57               |

### More benchmarks

[rust_serialization_benchmark](https://david.kolo.ski/rust_serialization_benchmark/)

## Acknowledgement

Some test cases were derived from [bincode](https://github.com/bincode-org/bincode) (see comment in `tests.rs`).

## License

Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.