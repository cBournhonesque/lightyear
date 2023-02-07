cfg_if! {
    if #[cfg(feature = "zstd_support")]
    {
        use std::fs;

        use log::info;

        use zstd::{bulk::Compressor, dict::from_continuous};

        use super::compression_config::CompressionMode;

        pub struct Encoder {
            result: Vec<u8>,
            encoder: EncoderType,
        }

        impl Encoder {
            pub fn new(compression_mode: CompressionMode) -> Self {
                let encoder = match compression_mode {
                    CompressionMode::Training(sample_size) => {
                        EncoderType::DictionaryTrainer(DictionaryTrainer::new(sample_size))
                    }
                    CompressionMode::Default(compression_level) => EncoderType::Compressor(
                        Compressor::new(compression_level).expect("error creating Compressor"),
                    ),
                    CompressionMode::Dictionary(compression_level, dictionary) => EncoderType::Compressor(
                        Compressor::with_dictionary(compression_level, &dictionary)
                            .expect("error creating Compressor with dictionary"),
                    ),
                };

                Self {
                    result: Vec::new(),
                    encoder,
                }
            }

            pub fn encode(&mut self, payload: &[u8]) -> &[u8] {
                // TODO: only use compressed packet if the resulting size would be less!
                match &mut self.encoder {
                    EncoderType::DictionaryTrainer(trainer) => {
                        trainer.record_bytes(payload);
                        self.result = payload.to_vec();
                        return &self.result;
                    }
                    EncoderType::Compressor(encoder) => {
                        self.result = encoder.compress(payload).expect("encode error");
                        return &self.result;
                    }
                }
            }
        }

        pub enum EncoderType {
            Compressor(Compressor<'static>),
            DictionaryTrainer(DictionaryTrainer),
        }

        pub struct DictionaryTrainer {
            sample_data: Vec<u8>,
            sample_sizes: Vec<usize>,
            next_alert_size: usize,
            target_sample_size: usize,
            training_complete: bool,
        }

        impl DictionaryTrainer {
            /// `target_sample_size` here describes the number of samples (packets) to
            /// train on. Obviously, the more samples trained on, the better
            /// theoretical compression.
            pub fn new(target_sample_size: usize) -> Self {
                Self {
                    target_sample_size,
                    sample_data: Vec::new(),
                    sample_sizes: Vec::new(),
                    next_alert_size: 0,
                    training_complete: false,
                }
            }

            pub fn record_bytes(&mut self, bytes: &[u8]) {
                if self.training_complete {
                    return;
                }

                self.sample_data.extend_from_slice(bytes);
                self.sample_sizes.push(bytes.len());

                let current_sample_size = self.sample_sizes.len();

                if current_sample_size >= self.next_alert_size {
                    let percent =
                        ((self.next_alert_size as f32) / (self.target_sample_size as f32)) * 100.0;
                    info!("Dictionary training: {}% complete", percent);

                    self.next_alert_size += self.target_sample_size / 20;
                }

                if current_sample_size >= self.target_sample_size {
                    info!("Dictionary training complete!");
                    info!(
                        "Samples: {} ({} KB)",
                        self.sample_sizes.len(),
                        self.sample_data.len()
                    );
                    info!("Dictionary processing sample data...");

                    // We have enough sample data to train the dictionary!
                    let target_dict_size = self.sample_data.len() / 100;
                    let dictionary =
                        from_continuous(&self.sample_data, &self.sample_sizes, target_dict_size)
                            .expect("Error while training dictionary");

                    // Now need to ... write it to a file I guess
                    fs::write("dictionary.txt", dictionary)
                        .expect("Error while writing dictionary to file");

                    info!("Dictionary written to `dictionary.txt`!");

                    self.training_complete = true;
                }
            }
        }
    }
    else
    {
        use super::compression_config::CompressionMode;

        pub struct Encoder {
            result: Vec<u8>
        }

        impl Encoder {
            pub fn new(_: CompressionMode) -> Self {
                Self {
                    result: Vec::new(),
                }
            }

            pub fn encode(&mut self, payload: &[u8]) -> &[u8] {
                self.result = payload.to_vec();
                &self.result
            }
        }
    }
}
