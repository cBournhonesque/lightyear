/// Conservative default maximum payload size for a link.
///
/// Concrete links can advertise a different limit through [`LinkMtu`]. The default remains 1200
/// bytes because it is safe for the connection and datagram transports supported by Lightyear.
pub const DEFAULT_MTU: usize = 1200;

/// The minimum, currently available, and configured maximum payload size of a link.
///
/// The minimum is stable once the link is linked and lets higher layers choose a fragment size
/// which remains valid if path-MTU discovery later changes the current value. The current MTU may
/// grow or shrink, but never below [`min_mtu`](Self::min_mtu) or above the configured maximum.
///
/// Both peers must agree on the minimum MTU. The transport derives its fixed fragment payload size
/// from this value instead of repeating that size in every fragment packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkMtu {
    min_mtu: usize,
    mtu: usize,
    max_mtu: usize,
}

impl LinkMtu {
    /// Creates link MTU characteristics whose minimum, current, and maximum values are all `mtu`.
    pub const fn new(mtu: usize) -> Self {
        Self {
            min_mtu: mtu,
            mtu,
            max_mtu: mtu,
        }
    }

    /// Returns the smallest MTU this link will report.
    pub const fn min_mtu(self) -> usize {
        self.min_mtu
    }

    /// Returns the link's current maximum payload size.
    pub const fn mtu(self) -> usize {
        self.mtu
    }

    /// Lowers the minimum MTU to `min_mtu` if it is higher.
    pub const fn lower_min_mtu(&mut self, min_mtu: usize) {
        if min_mtu < self.min_mtu {
            self.min_mtu = min_mtu;
        }
    }

    /// Updates the current MTU, capped at the configured maximum, without allowing it to fall
    /// below the minimum.
    pub const fn set_mtu(&mut self, mtu: usize) -> Result<(), MtuTooSmall> {
        if mtu < self.min_mtu {
            return Err(MtuTooSmall {
                mtu,
                min: self.min_mtu,
            });
        }
        self.mtu = if mtu < self.max_mtu {
            mtu
        } else {
            self.max_mtu
        };
        Ok(())
    }
}

impl Default for LinkMtu {
    fn default() -> Self {
        Self::new(DEFAULT_MTU)
    }
}

/// Error returned when a current MTU is smaller than a link's stable minimum MTU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MtuTooSmall {
    /// Rejected current MTU.
    pub mtu: usize,
    /// Smallest MTU accepted by the link.
    pub min: usize,
}

impl core::fmt::Display for MtuTooSmall {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "link MTU {} is smaller than its minimum {}",
            self.mtu, self.min
        )
    }
}

impl core::error::Error for MtuTooSmall {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_mtu_tracks_stable_minimum_and_current_value() {
        let mut mtu = LinkMtu::new(900);
        assert_eq!(mtu.min_mtu(), 900);
        assert_eq!(mtu.mtu(), 900);

        mtu.set_mtu(1400).unwrap();
        assert_eq!(mtu.min_mtu(), 900);
        assert_eq!(mtu.mtu(), 900);

        assert_eq!(mtu.set_mtu(899), Err(MtuTooSmall { mtu: 899, min: 900 }));
        assert_eq!(mtu.mtu(), 900);

        mtu.lower_min_mtu(1000);
        assert_eq!(mtu.min_mtu(), 900);
        mtu.lower_min_mtu(600);
        mtu.set_mtu(700).unwrap();
        assert_eq!(mtu.min_mtu(), 600);
        assert_eq!(mtu.mtu(), 700);
        assert_eq!(mtu.set_mtu(599), Err(MtuTooSmall { mtu: 599, min: 600 }));
    }
}
