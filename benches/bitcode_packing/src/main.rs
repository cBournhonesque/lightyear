//! Benchmark for how to use bitcode to pack messages into packets of MTU bytes
#![allow(unused_variables)]
use divan::counter::ItemsCount;
use divan::Bencher;
use rand::distributions::Standard;
use rand::prelude::*;

trait Packer {
    /// packet the messages into packets (preferably of size <=MAX_SIZE)
    fn pack(messages: &[Message]) -> Vec<Packet>;
}

#[derive(bitcode::Encode, bitcode::Decode)]
enum Message {
    A(bool),
    B(Vec<u8>),
    C { name: String, x: i16, y: i16 },
}

impl Distribution<Message> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Message {
        if rng.gen_bool(0.5) {
            Message::A(rng.gen_bool(0.1))
        } else if rng.gen_bool(0.4) {
            let len = rng.gen_range(1..=100);
            Message::B((0..len).map(|_| rng.gen_range(9..15)).collect())
        } else {
            Message::C {
                name: if rng.gen_bool(0.0001) {
                    // Throw a curveball of an incompressible string larger than a single packet.
                    let n = rng.gen_range(1300..2000);
                    (0..n).map(|_| rng.gen_range(b'0'..=b'9') as char).collect()
                } else {
                    [
                        "cow", "sheep", "zombie", "skeleton", "spider", "creeper", "parrot", "bee",
                    ]
                    .choose(rng)
                    .unwrap()
                    .to_string()
                },
                x: rng.gen_range(-100..100),
                y: rng.gen_range(0..15),
            }
        }
    }
}

struct Packet(Vec<u8>);
impl Packet {
    const MAX_SIZE: usize = 1200;
}

fn main() {
    divan::main();
}

trait GenMessages: Default {
    fn gen(&mut self) -> Vec<Message>;
}

struct RandomGen {
    rng: rand_chacha::ChaCha20Rng,
}

impl Default for RandomGen {
    fn default() -> Self {
        Self {
            rng: rand_chacha::ChaCha20Rng::from_seed(Default::default()),
        }
    }
}

impl GenMessages for RandomGen {
    fn gen(&mut self) -> Vec<Message> {
        let n = self.rng.gen_range(200..20000);
        (0..n).map(|_| self.rng.gen()).collect()
    }
}

#[divan::bench(
    types = [NaivePacker, AppendedPacker, ExponentialPacker, InterpolationPacker],
    sample_count = 10,
)]
fn run_packer<P: Packer>(bencher: Bencher) {
    let mut gen = RandomGen::default();
    let mut total_bytes = 0;
    let mut packet_count = 0;
    let mut packet_extensions = 0;
    bencher
        .with_inputs(|| gen.gen())
        .input_counter(|messages| ItemsCount::of_iter(messages))
        .bench_local_refs(|messages| {
            // println!("{}", messages.len());
            let packets = P::pack(messages);
            let packet_lens: Vec<_> = packets.iter().map(|p| p.0.len()).collect();
            // TODO: add as output_counters once it's supported https://github.com/nvzqz/divan/issues/34
            total_bytes += packet_lens.iter().sum::<usize>();
            packet_count += packets.len();
            // number of extra fragments needed
            packet_extensions += packet_lens
                .iter()
                .map(|&len| len.saturating_sub(1) / Packet::MAX_SIZE)
                .sum::<usize>();
            // println!("Packet lengths: {packet_lens:?}");
        });
    println!(
        "\n{total_bytes} bytes total, {packet_count} packets, {packet_extensions} extension packets"
    );
}

fn encode_compressed(t: &(impl bitcode::Encode + ?Sized)) -> Vec<u8> {
    let encoded = bitcode::encode(t);
    // Makes pack_interpolation_search take 33% fewer packets without reducing speed at all.
    const COMPRESS: bool = true;
    if COMPRESS {
        lz4_flex::compress_prepend_size(&encoded)
    } else {
        encoded
    }
}

struct NaivePacker;

impl Packer for NaivePacker {
    /// Just call encode_compressed once
    fn pack(messages: &[Message]) -> Vec<Packet> {
        vec![Packet(encode_compressed(messages))]
    }
}

struct AppendedPacker;

impl Packer for AppendedPacker {
    /// Encode each packet individually, check the size, and concatenate them up to MAX_SIZE
    fn pack(messages: &[Message]) -> Vec<Packet> {
        let mut bytes = vec![];
        let mut packets = vec![];
        for m in messages {
            // Don't use encode_compressed since compression doesn't improve tiny messages.
            let encoded = bitcode::encode(m);
            if bytes.len() + encoded.len() > Packet::MAX_SIZE {
                packets.push(Packet(core::mem::take(&mut bytes)));
            }
            bytes.extend_from_slice(&encoded);
        }
        if !bytes.is_empty() {
            packets.push(Packet(bytes));
        }
        packets
    }
}

struct ExponentialPacker;

impl Packer for ExponentialPacker {
    // pack messages[0..k] where k is a 2^i up to MAX_SIZE
    fn pack(mut messages: &[Message]) -> Vec<Packet> {
        let mut packets = vec![];
        let mut n = 1;
        let mut last = None;

        loop {
            n = n.min(messages.len());
            let chunk = &messages[..n];
            let encoded = encode_compressed(chunk);
            let current = (encoded, n);

            if current.0.len() < Packet::MAX_SIZE && n < messages.len() {
                last = Some(current);
                n *= 2;
                continue;
            }

            n = 1;
            // If the current chunk is too big, use the last chunk.
            let (encoded, n) = last
                .take()
                .filter(|_| current.0.len() > Packet::MAX_SIZE)
                .unwrap_or(current);

            messages = &messages[n..];
            packets.push(Packet(encoded));
            if messages.is_empty() {
                break;
            }
        }
        packets
    }
}

// https://en.wikipedia.org/wiki/Interpolation_search
// Check the average message size to guess how many more messages we can add.
struct InterpolationPacker;

impl Packer for InterpolationPacker {
    fn pack(mut messages: &[Message]) -> Vec<Packet> {
        const SAMPLE: usize = 32; // Tune based on expected message size and variance.
        const PRECISION: usize = 30; // More precision will take longer, but get closer to max packet size.
        const MAX_ATTEMPTS: usize = 4; // Maximum number of attempts before giving up.
        const TARGET_SIZE: usize = Packet::MAX_SIZE * PRECISION / (PRECISION + 1);
        const MIN_SIZE: usize = TARGET_SIZE * PRECISION / (PRECISION + 1);
        const DEBUG: bool = false;

        let mut packets = vec![];
        let mut message_size = None;
        // If we run out of attempts, send the largest attempt so far to avoid infinite loop.
        let mut attempts = 0;
        let mut largest_so_far = None;

        while !messages.is_empty() {
            let n = message_size
                .map(|message_size: f32| (TARGET_SIZE as f32 / message_size).floor() as usize)
                .unwrap_or(SAMPLE);
            let n = n.clamp(1, messages.len());

            let chunk = &messages[..n];
            let encoded = encode_compressed(chunk);

            message_size = Some(encoded.len() as f32 / n as f32);
            let too_large = encoded.len() > Packet::MAX_SIZE;
            let too_small = encoded.len() < MIN_SIZE && n != messages.len();
            let current = (encoded, n);

            let (encoded, n) = if too_large || too_small {
                if attempts < MAX_ATTEMPTS {
                    if DEBUG {
                        println!("skipping {n} messages with {} bytes", current.0.len());
                    }
                    if too_small && n > largest_so_far.as_ref().map_or(0, |(_, n)| *n) {
                        largest_so_far = Some(current);
                    }
                    attempts += 1;
                    continue;
                }
                // We ran out of attempts, if the current chunk is too big, use the largest chunk so far.
                largest_so_far
                    .take()
                    .filter(|_| current.0.len() > Packet::MAX_SIZE)
                    .unwrap_or(current)
            } else {
                current
            };

            attempts = 0;
            largest_so_far = None;

            if DEBUG {
                println!("packed {n} messages with {} bytes", encoded.len());
            }
            messages = &messages[n..];
            packets.push(Packet(encoded));
        }

        // TODO merge tiny packets (caused by single messages > Packet::MAX_SIZE)
        packets
    }
}
