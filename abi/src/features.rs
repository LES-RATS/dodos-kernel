//! Unlike cores: the feature mask of a thread, the size of a thread, and
//! whether one kind of core covers another (Unlike cores).

use crate::{Error, THREAD_FIXED_BYTES};

/// State feature bit 31: processor-delivered user interrupts, reserved on
/// every architecture (User interrupts: a placed hook).
pub const FEATURE_USER_INTERRUPTS: u32 = 31;
/// The first instruction feature; bits 0-31 are state features.
pub const FEATURE_INSTRUCTION_FIRST: u32 = 32;

/// The features an architecture lists (in `abi/`, per architecture): the
/// listed state features, those that have a vector length, and those that
/// are large (always optional).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureList {
    pub state: u32,
    pub with_length: u32,
    pub large: u32,
}

/// What a kind of core has, as the entries `Kinds` and `Features` publish
/// it: its features (state in bits 0-31, instruction in 32-63), its vector
/// length in bytes (0 if none), and the save-area bytes of each state
/// feature. Its baseline is derived from the list ([`Kind::baseline`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    pub features: u64,
    pub vector_bytes: u32,
    /// The bytes of the save area of the baseline (x86-64: x87, SSE and the
    /// state of every baseline feature).
    pub baseline_bytes: u32,
    pub feature_bytes: [u32; 32],
}

impl Kind {
    /// Baseline and optional state features: a listed state feature that
    /// the kind has and that is not large is baseline; bit 31 never is.
    pub const fn baseline(&self, list: FeatureList) -> u32 {
        (self.features as u32) & list.state & !list.large & !(1 << FEATURE_USER_INTERRUPTS)
    }
}

/// A thread's feature mask: bits 0-30 the optional state features it may
/// use, bit 31 user interrupts (reserved), bits 32-47 the vector length in
/// bytes, bits 48-63 zero.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureMask(u64);

impl FeatureMask {
    pub const NONE: Self = Self(0);

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn new(features: u32, vector_bytes: u16) -> Self {
        Self(features as u64 | (vector_bytes as u64) << 32)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Bits 0-31.
    pub const fn features(self) -> u32 {
        self.0 as u32
    }

    pub const fn vector_bytes(self) -> u32 {
        (self.0 >> 32 & 0xFFFF) as u32
    }

    /// The feature mask of a thread, judged as at creation, in order:
    /// `Invalid` for a bit 48-63 set, a bit that names no listed state
    /// feature, a length without a feature that has one, or no length with
    /// such a feature; then `Unsupported` for bit 31, a feature the kind of
    /// the node does not have, or a length other than the kind's. A bit for
    /// a baseline feature is accepted.
    pub const fn check(self, list: FeatureList, kind: &Kind) -> Result<(), Error> {
        let features = self.features();
        let user = 1 << FEATURE_USER_INTERRUPTS;
        let named = list.state | user;
        let lengthy = features & list.with_length != 0;
        let length = self.vector_bytes();
        if self.0 >> 48 != 0 || features & !named != 0 || lengthy != (length != 0) {
            return Err(Error::Invalid);
        }
        let had = kind.features as u32;
        if features & user != 0 || features & !had != 0 || lengthy && length != kind.vector_bytes {
            return Err(Error::Unsupported);
        }
        Ok(())
    }

    /// Whether a kind has every feature of the mask and its length: the
    /// condition of `thread_move` on the destination node.
    pub const fn fits(self, kind: &Kind) -> bool {
        let features = self.features() & !(1 << FEATURE_USER_INTERRUPTS);
        features & !(kind.features as u32) == 0
            && (self.vector_bytes() == 0 || self.vector_bytes() == kind.vector_bytes)
    }
}

/// The size of a thread: the smallest power of two at least
/// `THREAD_FIXED_BYTES` + the baseline bytes of the kind + the bytes of each
/// optional feature of the mask on that kind. `None` on overflow.
pub const fn thread_bytes(mask: FeatureMask, list: FeatureList, kind: &Kind) -> Option<u64> {
    let optional = mask.features() & !kind.baseline(list) & !(1 << FEATURE_USER_INTERRUPTS);
    let mut sum = THREAD_FIXED_BYTES + kind.baseline_bytes as u64;
    let mut bit = 0;
    while bit < 31 {
        if optional & 1 << bit != 0 {
            sum += kind.feature_bytes[bit] as u64;
        }
        bit += 1;
    }
    sum.checked_next_power_of_two()
}

/// Whether a core of kind `host` may run a node whose own kind is `node`:
/// it has every feature, state and instruction, of the node's kind, with
/// the same vector length (Where a thread may run).
pub const fn kind_covers(host: &Kind, node: &Kind) -> bool {
    node.features & !host.features == 0
        && (node.vector_bytes == 0 || node.vector_bytes == host.vector_bytes)
}

#[cfg(kani)]
mod proofs {
    use super::*;

    fn any_kind() -> Kind {
        let mut feature_bytes = [0u32; 32];
        for bytes in feature_bytes.iter_mut().take(4) {
            *bytes = kani::any::<u32>() >> 12;
        }
        Kind {
            features: kani::any(),
            vector_bytes: kani::any::<u16>() as u32,
            baseline_bytes: kani::any::<u32>() >> 12,
            feature_bytes,
        }
    }

    const LIST: FeatureList = FeatureList {
        state: 0b1111,
        with_length: 0b0010,
        large: 0b0100,
    };

    #[kani::proof]
    fn the_mask_is_invalid_then_unsupported_in_order() {
        let mask = FeatureMask::from_raw(kani::any());
        let kind = any_kind();
        let features = mask.features();
        let invalid = mask.raw() >> 48 != 0
            || features & !(LIST.state | 1 << 31) != 0
            || (features & LIST.with_length != 0) != (mask.vector_bytes() != 0);
        let unsupported = features & 1 << 31 != 0
            || features & !(kind.features as u32) != 0
            || features & LIST.with_length != 0 && mask.vector_bytes() != kind.vector_bytes;
        match mask.check(LIST, &kind) {
            Err(Error::Invalid) => assert!(invalid),
            Err(Error::Unsupported) => assert!(!invalid && unsupported),
            Ok(()) => assert!(!invalid && !unsupported && mask.fits(&kind)),
            Err(_) => panic!(),
        }
    }

    #[kani::proof]
    #[kani::unwind(33)]
    fn the_size_of_a_thread_is_a_power_of_two_at_least_the_sum() {
        let mut mask = FeatureMask::from_raw(kani::any::<u64>() & 0b1111);
        if kani::any() {
            mask = FeatureMask::from_raw(mask.raw() | 1 << 31);
        }
        let kind = any_kind();
        if let Some(bytes) = thread_bytes(mask, LIST, &kind) {
            assert!(bytes.is_power_of_two() && bytes >= THREAD_FIXED_BYTES);
            let optional = mask.features() & !kind.baseline(LIST) & 0x7FFF_FFFF;
            let mut sum = THREAD_FIXED_BYTES + kind.baseline_bytes as u64;
            for bit in 0..4 {
                if optional & 1 << bit != 0 {
                    sum += kind.feature_bytes[bit] as u64;
                }
            }
            assert!(bytes >= sum && bytes / 2 < sum);
        }
    }

    #[kani::proof]
    fn a_kind_covers_another_with_its_features_and_length() {
        let (host, node) = (any_kind(), any_kind());
        let expected = (node.features & !host.features == 0)
            && (node.vector_bytes == 0 || node.vector_bytes == host.vector_bytes);
        assert_eq!(kind_covers(&host, &node), expected);
        assert!(kind_covers(&node, &node));
    }
}
