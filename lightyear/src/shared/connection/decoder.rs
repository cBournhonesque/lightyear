cfg_if! {
    if #[cfg(feature = "zstd_support")]
    {
        use zstd::bulk::Decompressor;

        use super::compression_config::CompressionMode;

        pub struct Decoder {
            result: Vec<u8>,
            decoder: Option<Decompressor<'static>>,
        }

        impl Decoder {
            pub fn new(compression_mode: CompressionMode) -> Self {
                let decoder = match compression_mode {
                    CompressionMode::Training(_) => None,
                    CompressionMode::Default(_) => {
                        Some(Decompressor::new().expect("error creating Decompressor"))
                    }
                    CompressionMode::Dictionary(_, dictionary) => Some(
                        Decompressor::with_dictionary(&dictionary).expect("error creating Decompressor"),
                    ),
                };

                Self {
                    decoder,
                    result: Vec::new(),
                }
            }

            pub fn decode(&mut self, payload: &[u8]) -> &[u8] {
                if let Some(decoder) = &mut self.decoder {
                    self.result = decoder
                        .decompress(
                            payload,
                            Decompressor::<'static>::upper_bound(payload)
                                .expect("upper bound decode error"),
                        )
                        .expect("decode error");
                    return &self.result;
                } else {
                    self.result = payload.to_vec();
                    return &self.result;
                }
            }
        }
    }
    else
    {
        use super::compression_config::CompressionMode;

        pub struct Decoder {
            result: Vec<u8>,
        }

        impl Decoder {
            pub fn new(_: CompressionMode) -> Self {
                Self {
                    result: Vec::new(),
                }
            }

            pub fn decode(&mut self, payload: &[u8]) -> &[u8] {
                self.result = payload.to_vec();
                &self.result
            }
        }
    }
}
