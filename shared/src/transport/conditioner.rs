#[cfg(test)]
use mock_instant::Instant;
#[cfg(not(test))]
use std::time::Instant;

use anyhow::Result;
use crate::transport::{PacketReceiver, Transport};
use implementation::TimeMinHeap;


/// Contains configuration required to initialize a LinkConditioner
#[derive(Clone)]
pub struct LinkConditionerConfig {
    /// Delay to receive incoming messages in milliseconds
    pub incoming_latency: u32,
    /// The maximum additional random latency to delay received incoming
    /// messages in milliseconds. This may be added OR subtracted from the
    /// latency determined in the `incoming_latency` property above
    pub incoming_jitter: u32,
    /// The % chance that an incoming packet will be dropped.
    /// Represented as a value between 0 and 1
    pub incoming_loss: f32,
}


pub struct LinkConditioner<T: PacketReceiver> {
    packet_receiver: T,
    config: LinkConditionerConfig,
    time_queue: TimeMinHeap<Box<[u8]>>
}

impl<T> LinkConditioner<T> {
    fn condition_packet<P: Eq>(&mut self, packet: P) {
        if Random::gen_range_f32(0.0, 1.0) <= self.config.incoming_loss {
            // drop the packet
            return;
        }
        let mut latency: u32 = self.config.incoming_latency;
        if config.incoming_jitter > 0 {
            if Random::gen_bool() {
                latency += Random::gen_range_u32(0, self.config.incoming_jitter);
            } else {
                latency -= Random::gen_range_u32(0, self.config.incoming_jitter);
            }
        }
        let mut packet_timestamp = Instant::now();
        packet_timestamp.add_millis(latency);
        self.time_queue.add_item(packet_timestamp, packet);
    }
}

impl<T: PacketReceiver> PacketReceiver for LinkConditioner<T> {
    fn recv(&mut self) -> Result<Option<&[u8]>> {
        loop {
            // keep trying to receive packets from the inner packet receiver
            match self.packet_receiver.recv() {
                Ok(option) => match option {
                    None => break,
                    // add conditioning (put the packets in the time queue)
                    Some(packet) => self.condition_packet(packet),
                },
                Err(err) => {
                    return Err(err);
                }
            }
        }
        // only return a packet if it is ready to be returned
        match self.time_queue.pop_item() {
            Some(packet) => Ok(Some(packet.as_ref())),
            None => Ok(None),
        }
    }
}

impl LinkConditionerConfig {
    /// Creates a new LinkConditionerConfig
    pub fn new(incoming_latency: u32, incoming_jitter: u32, incoming_loss: f32) -> Self {
        LinkConditionerConfig {
            incoming_latency,
            incoming_jitter,
            incoming_loss,
        }
    }

    /// Creates a new LinkConditioner that simulates a connection which is in a
    /// good condition
    pub fn good_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: 40,
            incoming_jitter: 6,
            incoming_loss: 0.002,
        }
    }

    /// Creates a new LinkConditioner that simulates a connection which is in an
    /// average condition
    pub fn average_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: 170,
            incoming_jitter: 45,
            incoming_loss: 0.02,
        }
    }

    /// Creates a new LinkConditioner that simulates a connection which is in an
    /// poor condition
    pub fn poor_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: 300,
            incoming_jitter: 84,
            incoming_loss: 0.04,
        }
    }
}


pub(crate) mod implementation {
    #[cfg(test)]
    use mock_instant::Instant;
    #[cfg(not(test))]
    use std::time::Instant;

    use std::{cmp::Ordering, collections::BinaryHeap};

    /// A heap that contains items associated with an instant.
    ///
    /// The instant represents the time at which the item becomes "visible"
    /// Before that time, it's as if the item does not exist
    #[derive(Clone)]
    pub(crate) struct TimeMinHeap<T: Eq + PartialEq> {
        heap: BinaryHeap<ItemWithTime<T>>,
    }

    impl<T: Eq + PartialEq> TimeMinHeap<T> {
        pub fn new() -> Self {
            Self {
                heap: BinaryHeap::default(),
            }
        }
    }

    impl<T: Eq + PartialEq> TimeMinHeap<T> {
        /// Adds an item to the heap marked by time
        pub fn add_item(&mut self, instant: Instant, item: T) {
            self.heap.push(ItemWithTime { instant, item });
        }

        /// Returns whether or not there is an item that is ready to be returned
        /// (i.e. we are beyond the instant associated with the item)
        pub fn has_item(&self) -> bool {
            if self.heap.is_empty() {
                return false;
            }
            if let Some(item) = self.heap.peek() {
                return item.instant <= Instant::now();
            }
            false
        }

        /// Pops the most recent item from the queue if sufficient time has elapsed
        /// (i.e. we are beyond the instant associated with the item)
        pub fn pop_item(&mut self) -> Option<T> {
            if self.has_item() {
                if let Some(container) = self.heap.pop() {
                    return Some(container.item);
                }
            }
            None
        }

        /// Returns the length of the underlying queue
        pub fn len(&self) -> usize {
            self.heap.len()
        }

        /// Checks if the underlying queue is empty
        pub fn is_empty(&self) -> bool {
            self.heap.is_empty()
        }
    }

    #[derive(Clone, Eq, PartialEq)]
    struct ItemWithTime<T> {
        pub instant: Instant,
        pub item: T,
    }


    /// BinaryHeap is a max-heap, so we must reverse the ordering of the Instants
    /// to get a min-heap
    impl<T: Eq + PartialEq> Ord for ItemWithTime<T> {
        fn cmp(&self, other: &ItemWithTime<T>) -> Ordering {
            other.instant.cmp(&self.instant)
        }
    }

    impl<T: Eq + PartialEq> PartialOrd for ItemWithTime<T> {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    #[cfg(test)]
    mod tests {
        use std::time::Duration;
        use mock_instant::{MockClock, Instant};
        use super::TimeMinHeap;

        #[test]
        fn test_time_heap() {
            let mut heap = TimeMinHeap::<u64>::new();
            let now = Instant::now();

            // can insert items in any order of time
            heap.add_item(now + Duration::from_secs(2), 2);
            heap.add_item(now + Duration::from_secs(1), 1);
            heap.add_item(now + Duration::from_secs(3), 3);

            // no items are visible
            assert_eq!(heap.has_item(), false);

            // we move the clock to 2, 2 items should be visible, in order of insertion
            MockClock::advance(Duration::from_secs(2));
            assert_eq!(heap.pop_item(), Some(1));
            assert_eq!(heap.pop_item(), Some(2));
            assert_eq!(heap.pop_item(), None);
            assert_eq!(heap.len(), 1);

        }
    }
}


