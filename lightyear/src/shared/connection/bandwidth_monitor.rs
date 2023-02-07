use std::time::Duration;

pub struct BandwidthMonitor {
    time_queue: TimeQueue<usize>,
    total_bytes: u16,
    to_kbps_factor: f32,
}

impl BandwidthMonitor {
    pub fn new(bandwidth_measure_duration: Duration) -> Self {
        BandwidthMonitor {
            time_queue: TimeQueue::new(bandwidth_measure_duration),
            total_bytes: 0,
            to_kbps_factor: 0.008 / bandwidth_measure_duration.as_secs_f32(),
        }
    }

    pub fn record_packet(&mut self, bytes: usize) {
        self.clear_expired_packets();

        self.total_bytes += bytes as u16;
        self.time_queue.add_item(bytes);
    }

    pub fn bandwidth(&mut self) -> f32 {
        self.clear_expired_packets();

        self.total_bytes as f32 * self.to_kbps_factor
    }

    fn clear_expired_packets(&mut self) {
        while let Some(bytes) = self.time_queue.pop_item() {
            self.total_bytes -= bytes as u16;
        }
    }
}

////

use std::{cmp::Ordering, collections::BinaryHeap};

use naia_socket_shared::Instant;

#[derive(Clone)]
pub struct TimeQueue<T: Eq + PartialEq> {
    queue: BinaryHeap<ItemContainer<T>>,
    duration: Duration,
}

impl<T: Eq + PartialEq> TimeQueue<T> {
    pub fn new(duration: Duration) -> Self {
        TimeQueue {
            queue: BinaryHeap::new(),
            duration,
        }
    }

    pub fn add_item(&mut self, item: T) {
        self.queue.push(ItemContainer {
            instant: Instant::now(),
            item,
        });
    }

    pub fn has_item(&self) -> bool {
        if self.queue.is_empty() {
            return false;
        }
        if let Some(item) = self.queue.peek() {
            return item.instant.elapsed() > self.duration;
        }
        false
    }

    pub fn pop_item(&mut self) -> Option<T> {
        if self.has_item() {
            if let Some(container) = self.queue.pop() {
                return Some(container.item);
            }
        }
        None
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ItemContainer<T: Eq + PartialEq> {
    pub instant: Instant,
    pub item: T,
}

impl<T: Eq + PartialEq> Ord for ItemContainer<T> {
    fn cmp(&self, other: &ItemContainer<T>) -> Ordering {
        other.instant.cmp(&self.instant)
    }
}

impl<T: Eq + PartialEq> PartialOrd for ItemContainer<T> {
    fn partial_cmp(&self, other: &ItemContainer<T>) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
