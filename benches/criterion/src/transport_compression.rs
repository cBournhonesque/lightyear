use lightyear::metrics::metrics::Key;
use lightyear::metrics::metrics_util::{CompositeKey, MetricKind};
use lightyear::prelude::{
    CompressionConfig, GLOBAL_RECORDER, MessageSender, MetricsRegistry, Transport,
};
use lightyear_tests::protocol::{Channel1, StringMessage};
use lightyear_tests::stepper::{ClientServerStepper, StepperConfig};

#[derive(Clone, Copy, Debug)]
pub enum PayloadPattern {
    Repeated,
    Structured,
    RandomAscii,
}

#[derive(Clone, Copy, Debug)]
pub struct TransportCompressionCase {
    pub name: &'static str,
    pub pattern: PayloadPattern,
    pub message_len: usize,
    pub messages: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressionMode {
    Disabled,
    Lz4,
}

impl CompressionMode {
    pub const ALL: [Self; 2] = [Self::Disabled, Self::Lz4];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Lz4 => "lz4",
        }
    }

    pub const fn config(self) -> CompressionConfig {
        match self {
            Self::Disabled => CompressionConfig::DISABLED,
            Self::Lz4 => CompressionConfig::LZ4,
        }
    }
}

pub const TRANSPORT_COMPRESSION_CASES: &[TransportCompressionCase] = &[
    TransportCompressionCase {
        name: "repeated_64b_128msg",
        pattern: PayloadPattern::Repeated,
        message_len: 64,
        messages: 128,
    },
    TransportCompressionCase {
        name: "structured_128b_128msg",
        pattern: PayloadPattern::Structured,
        message_len: 128,
        messages: 128,
    },
    TransportCompressionCase {
        name: "random_ascii_128b_128msg",
        pattern: PayloadPattern::RandomAscii,
        message_len: 128,
        messages: 128,
    },
    TransportCompressionCase {
        name: "repeated_1024b_16msg",
        pattern: PayloadPattern::Repeated,
        message_len: 1024,
        messages: 16,
    },
];

#[derive(Clone, Copy, Debug, Default)]
pub struct TransportCompressionRun {
    pub send_bytes: f64,
    pub compression_abandoned: f64,
    pub compression_saved_bytes: f64,
    pub compression_skipped: f64,
    pub compression_expanded_packets: f64,
    pub compression_above_mtu_packets: f64,
}

pub struct PreparedTransportCompressionRun {
    stepper: ClientServerStepper,
}

pub fn run_transport_compression_case(
    case: TransportCompressionCase,
    mode: CompressionMode,
) -> TransportCompressionRun {
    let mut prepared = prepare_transport_compression_case(case, mode);
    run_prepared_transport_compression_case(&mut prepared)
}

pub fn prepare_transport_compression_case(
    case: TransportCompressionCase,
    mode: CompressionMode,
) -> PreparedTransportCompressionRun {
    let mut config = StepperConfig::single();
    config.server_registry = Some(GLOBAL_RECORDER.clone());
    config.client_registry = Some(GLOBAL_RECORDER.clone());
    let mut stepper = ClientServerStepper::from_config(config);
    set_compression(&mut stepper, mode.config());

    for message_index in 0..case.messages {
        let payload = payload(case.pattern, case.message_len, message_index);
        stepper
            .client_of_mut(0)
            .get_mut::<MessageSender<StringMessage>>()
            .unwrap()
            .send::<Channel1>(StringMessage(payload));
    }

    PreparedTransportCompressionRun { stepper }
}

pub fn run_prepared_transport_compression_case(
    prepared: &mut PreparedTransportCompressionRun,
) -> TransportCompressionRun {
    let start = TransportCompressionRun::from_metrics(&GLOBAL_RECORDER);
    prepared.stepper.frame_step_server_first(1);
    let end = TransportCompressionRun::from_metrics(&GLOBAL_RECORDER);
    end - start
}

pub fn print_transport_compression_stats_once(
    case: TransportCompressionCase,
    mode: CompressionMode,
) {
    let stats = run_transport_compression_case(case, mode);
    eprintln!(
        "transport_compression_stats case={} mode={} send_bytes={} abandoned={} saved_bytes={} skipped={} expanded={} above_mtu={}",
        case.name,
        mode.name(),
        stats.send_bytes,
        stats.compression_abandoned,
        stats.compression_saved_bytes,
        stats.compression_skipped,
        stats.compression_expanded_packets,
        stats.compression_above_mtu_packets,
    );
}

fn set_compression(stepper: &mut ClientServerStepper, compression: CompressionConfig) {
    for client_id in 0..stepper.client_entities.len() {
        stepper
            .client_mut(client_id)
            .get_mut::<Transport>()
            .unwrap()
            .set_compression(compression);
        stepper
            .client_of_mut(client_id)
            .get_mut::<Transport>()
            .unwrap()
            .set_compression(compression);
    }
}

impl TransportCompressionRun {
    fn from_metrics(registry: &MetricsRegistry) -> Self {
        Self {
            send_bytes: metric_value(registry, MetricKind::Gauge, "transport/send_bytes"),
            compression_abandoned: metric_value(
                registry,
                MetricKind::Counter,
                "transport/compression_abandoned",
            ),
            compression_saved_bytes: metric_value(
                registry,
                MetricKind::Counter,
                "transport/compression_saved_bytes",
            ),
            compression_skipped: metric_value(
                registry,
                MetricKind::Counter,
                "transport/compression_skipped",
            ),
            compression_expanded_packets: metric_value(
                registry,
                MetricKind::Counter,
                "transport/compression_expanded_packets",
            ),
            compression_above_mtu_packets: metric_value(
                registry,
                MetricKind::Counter,
                "transport/compression_above_mtu_packets",
            ),
        }
    }
}

impl core::ops::Sub for TransportCompressionRun {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self {
            send_bytes: self.send_bytes - rhs.send_bytes,
            compression_abandoned: self.compression_abandoned - rhs.compression_abandoned,
            compression_saved_bytes: self.compression_saved_bytes - rhs.compression_saved_bytes,
            compression_skipped: self.compression_skipped - rhs.compression_skipped,
            compression_expanded_packets: self.compression_expanded_packets
                - rhs.compression_expanded_packets,
            compression_above_mtu_packets: self.compression_above_mtu_packets
                - rhs.compression_above_mtu_packets,
        }
    }
}

fn metric_value(registry: &MetricsRegistry, kind: MetricKind, name: &'static str) -> f64 {
    registry
        .fetch_metric_value(&CompositeKey::new(kind, Key::from_name(name)))
        .unwrap_or_default()
}

fn payload(pattern: PayloadPattern, len: usize, message_index: usize) -> String {
    match pattern {
        PayloadPattern::Repeated => "A".repeat(len),
        PayloadPattern::Structured => structured_payload(len, message_index),
        PayloadPattern::RandomAscii => random_ascii_payload(len, message_index),
    }
}

fn structured_payload(len: usize, message_index: usize) -> String {
    let seed = format!(
        "{{entity:{:04},x:{:04},y:{:04},state:idle,tag:replicated}};",
        message_index,
        message_index % 128,
        (message_index * 3) % 128
    );
    repeat_to_len(&seed, len)
}

fn random_ascii_payload(len: usize, message_index: usize) -> String {
    let mut state = 0x9e37_79b9_7f4a_7c15u64 ^ message_index as u64;
    let mut payload = String::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let byte = b'!' + (state % 94) as u8;
        payload.push(byte as char);
    }
    payload
}

fn repeat_to_len(seed: &str, len: usize) -> String {
    let mut payload = String::with_capacity(len);
    while payload.len() + seed.len() <= len {
        payload.push_str(seed);
    }
    if payload.len() < len {
        payload.push_str(&seed[..len - payload.len()]);
    }
    payload
}
