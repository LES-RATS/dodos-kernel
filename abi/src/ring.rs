//! What crosses between nodes (D-128, D-129, D-131): the twelve kinds of
//! message, the two rings of each ordered pair of node slots, the header
//! at the start of a message's first entry, the credits and the bounds.
//!
//! The contract fixes the header and the counts; it gives no layout of the
//! words that follow the header. CONTRACT-GAP 3: each kind's words are
//! fixed here, as the documentation of [`NodeMessage`] lists them, so that
//! the room a message takes in its ring is a function of the contract's
//! values.

/// Bytes of one entry of a ring.
pub const RING_ENTRY_BYTES: usize = 64;
/// Entries of the kernel ring and of the user ring of an ordered pair.
pub const RING_KERNEL_ENTRIES: u64 = 64;
pub const RING_USER_ENTRIES: u64 = 64;
/// Entries one message takes at most (a `Send` of 64 words with 4 grants
/// takes the most).
pub const RING_MESSAGE_ENTRIES_MAX: usize = 11;
/// Request credits and send credits of a node toward each peer.
pub const REQUESTS_PER_PEER: usize = 16;
pub const SENDS_PER_PEER: usize = 8;
/// Of the send credits, those kept for a `node` capability with `manage`
/// (the recommended rule of the first question after the eighth review).
pub const SENDS_RESERVED_FOR_MANAGE: usize = 2;
/// Frames one `node_send` grants, at most.
pub const SEND_GRANTS_MAX: usize = 4;
/// Entries of a user ring served per interrupt.
pub const RING_USER_STEP: u64 = 16;
/// Pending releases or signals sent per event.
pub const PENDING_STEP: usize = 32;
/// Entries of a node's unit request table.
pub const UNIT_REQUESTS: usize = 64;
/// Entries of each kernel ring kept for answers: one per credit the peer
/// can have in use, and ten more because a `Report` takes up to eleven.
pub const RING_ANSWER_RESERVE: u64 = (REQUESTS_PER_PEER + SENDS_PER_PEER + 10) as u64;
/// Bytes of the header of a message.
pub const RING_HEADER_BYTES: usize = 16;
/// The credit field of a message that uses no credit.
pub const CREDIT_NONE: u8 = 31;
/// The serial of a credit: 27 bits, one more at each use.
pub const CREDIT_SERIAL_MASK: u32 = (1 << 27) - 1;
/// The key of a `Report` with no next item.
pub const REPORT_END: u64 = u64::MAX;

/// The ring a kind travels on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring {
    /// Kinds 1 to 10, the kernels' own messages.
    Kernel,
    /// Kinds 11 and 12, the messages user threads ask for.
    User,
}

/// The twelve kinds of message between nodes, and nothing else. The words
/// after the header, by kind (CONTRACT-GAP 3):
///
/// - `Revoke`: the grant index (bits 0-11), its serial (12-41) and the use
///   index (42-53), one word.
/// - `RevokeStep`: 1 if the root remote capability was removed, else 0.
/// - `Release`, `Signal`: the grant index (0-11) and its serial (12-41).
/// - `UnitRequest`: the role (0-15), the epoch (16-44) and the operation
///   (48-55); then two operands.
/// - `UnitAnswer`: the result (`UnitResult`), then its value.
/// - `Handed`, `ReportAsk`: the role (0-15) and the epoch (16-44); a
///   `ReportAsk` adds the key it asks from.
/// - `Report`: the key of the next item, or `REPORT_END`; then three words
///   per item.
/// - `Ping`: nothing.
/// - `SendAnswer`: 1 if delivered, with `caps_dropped` in bit 1; then one
///   word per frame, the use index or `u64::MAX` if not delivered.
/// - `Send`: the message header, the task identifier and the marks of the
///   stamp; then one word per grant, its index (0-11), serial (12-41) and
///   rights (42-49); then the message words.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeMessage {
    Revoke = 1,
    RevokeStep = 2,
    Release = 3,
    UnitRequest = 4,
    UnitAnswer = 5,
    Handed = 6,
    ReportAsk = 7,
    Report = 8,
    Ping = 9,
    SendAnswer = 10,
    Send = 11,
    Signal = 12,
}

impl NodeMessage {
    pub const ALL: [Self; 12] = [
        Self::Revoke,
        Self::RevokeStep,
        Self::Release,
        Self::UnitRequest,
        Self::UnitAnswer,
        Self::Handed,
        Self::ReportAsk,
        Self::Report,
        Self::Ping,
        Self::SendAnswer,
        Self::Send,
        Self::Signal,
    ];

    /// The kind of number `raw`, `None` for a kind the receiver does not
    /// know.
    pub const fn from_raw(raw: u8) -> Option<Self> {
        if raw == 0 || raw as usize > Self::ALL.len() {
            return None;
        }
        Some(Self::ALL[raw as usize - 1])
    }

    pub const fn ring(self) -> Ring {
        match self {
            Self::Send | Self::Signal => Ring::User,
            _ => Ring::Kernel,
        }
    }

    /// The requests of the kernel ring, which take a request credit.
    pub const fn is_request(self) -> bool {
        matches!(self, Self::Revoke | Self::UnitRequest | Self::ReportAsk)
    }

    /// The answers, which always have room and name the credit they answer.
    pub const fn is_answer(self) -> bool {
        matches!(
            self,
            Self::RevokeStep | Self::UnitAnswer | Self::Report | Self::SendAnswer
        )
    }

    /// The request an answer answers.
    pub const fn answers(self) -> Option<Self> {
        match self {
            Self::RevokeStep => Some(Self::Revoke),
            Self::UnitAnswer => Some(Self::UnitRequest),
            Self::Report => Some(Self::ReportAsk),
            Self::SendAnswer => Some(Self::Send),
            _ => None,
        }
    }
}

/// The entries a message of `words` words after its header takes.
pub const fn message_entries(words: usize) -> usize {
    (RING_HEADER_BYTES + 8 * words).div_ceil(RING_ENTRY_BYTES)
}

/// The entries a ring of `entries` entries has free, with its producer and
/// consumer indices: 64-bit counts that never go back, so the difference
/// is taken modulo 2^64. A producer more than `entries` ahead leaves none.
pub const fn ring_free(produced: u64, consumed: u64, entries: u64) -> u64 {
    entries.saturating_sub(produced.wrapping_sub(consumed))
}

/// Whether a message of `length` entries fits in the ring with `reserve`
/// entries kept beyond it (the reserve of answers, for every kernel message
/// that is not an answer).
pub const fn ring_fits(
    produced: u64,
    consumed: u64,
    entries: u64,
    length: u64,
    reserve: u64,
) -> bool {
    match length.checked_add(reserve) {
        Some(need) => ring_free(produced, consumed, entries) >= need,
        None => false,
    }
}

/// The consumer index the receiver starts from: a producer index more than
/// the ring's entries ahead of the consumer first moves the consumer to the
/// producer minus the ring's entries (D-131); otherwise it stays.
pub const fn ring_resync(produced: u64, consumed: u64, entries: u64) -> u64 {
    if produced.wrapping_sub(consumed) > entries {
        produced.wrapping_sub(entries)
    } else {
        consumed
    }
}

/// The header at the start of a message's first entry (16 bytes): its kind,
/// its length in entries, its sender's node slot and generation, the
/// generation of the receiver it was sent to, and its credit (bits 0-4,
/// `CREDIT_NONE` for none) with the credit's serial (bits 5-31).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RingHeader {
    pub kind: u8,
    pub length: u8,
    pub sender: u16,
    pub sender_generation: u32,
    pub receiver_generation: u32,
    pub credit: u32,
}

impl RingHeader {
    pub const fn new(
        kind: NodeMessage,
        length: u8,
        sender: (u16, u32),
        receiver_generation: u32,
        credit: Option<(u8, u32)>,
    ) -> Self {
        let credit = match credit {
            Some((index, serial)) => (index & 0x1f) as u32 | (serial & CREDIT_SERIAL_MASK) << 5,
            None => CREDIT_NONE as u32,
        };
        Self {
            kind: kind as u8,
            length,
            sender: sender.0,
            sender_generation: sender.1,
            receiver_generation,
            credit,
        }
    }

    /// The credit and its serial, `None` for a message without one.
    pub const fn credit(&self) -> Option<(u8, u32)> {
        let index = (self.credit & 0x1f) as u8;
        if index == CREDIT_NONE {
            None
        } else {
            Some((index, self.credit >> 5))
        }
    }

    /// Whether the receiver can read the header, with `available` entries
    /// between its consumer index and the producer index: a kind it knows
    /// and a length from 1 to `RING_MESSAGE_ENTRIES_MAX` that does not
    /// reach past the producer index. A header it cannot read is consumed
    /// as one entry (D-131).
    pub const fn readable(&self, available: u64) -> bool {
        NodeMessage::from_raw(self.kind).is_some()
            && self.length >= 1
            && self.length as usize <= RING_MESSAGE_ENTRIES_MAX
            && self.length as u64 <= available
    }
}

/// The result of a `UnitAnswer`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitResult {
    /// The operation is done, its confirmation included.
    Done = 0,
    /// The holder does not serve the request: the requester reads the role
    /// word again.
    Elsewhere = 1,
    /// The unit did not confirm within `IOMMU_WAIT_NS`.
    Failed = 2,
}

/// The operations of a unit request (Devices and shared hardware), each
/// idempotent. CONTRACT-GAP 3: the contract lists them without numbers.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitOperation {
    /// Attach a device to an iospace (its context entry, its domain).
    Attach = 1,
    /// Detach a device.
    Detach = 2,
    /// Invalidate the IOMMU translations of a domain, after an unmap or the
    /// unhooking of a table.
    Invalidate = 3,
    /// Invalidate the interrupt entry cache after a remapping entry.
    InterruptCache = 4,
    /// Program a line's redirection entry in an IOAPIC.
    Redirection = 5,
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// The ring indices: a message that fits leaves the producer at most
    /// the ring's entries ahead of the consumer, and its reserve free; a
    /// resynchronised consumer is never more than the ring's entries behind
    /// the producer and never moves back while the ring is sane.
    #[kani::proof]
    fn ring_indices_never_overflow_the_ring() {
        let (produced, consumed, length, reserve): (u64, u64, u64, u64) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        let entries = if kani::any() {
            RING_KERNEL_ENTRIES
        } else {
            RING_USER_ENTRIES
        };
        let free = ring_free(produced, consumed, entries);
        assert!(free <= entries);
        if ring_fits(produced, consumed, entries, length, reserve) {
            let after = produced.wrapping_add(length);
            assert!(after.wrapping_sub(consumed) <= entries);
            assert!(ring_free(after, consumed, entries) >= reserve);
        }
        let start = ring_resync(produced, consumed, entries);
        assert!(produced.wrapping_sub(start) <= entries);
        if produced.wrapping_sub(consumed) <= entries {
            assert!(start == consumed);
        }
    }

    /// The credit and its serial read back, and a header with no credit
    /// reads none.
    #[kani::proof]
    fn a_header_reads_back_its_credit() {
        let (index, serial): (u8, u32) = (kani::any(), kani::any());
        kani::assume(index < CREDIT_NONE && serial <= CREDIT_SERIAL_MASK);
        let kind = NodeMessage::Revoke;
        let header = RingHeader::new(
            kind,
            1,
            (kani::any(), kani::any()),
            kani::any(),
            Some((index, serial)),
        );
        assert!(header.credit() == Some((index, serial)));
        let none = RingHeader::new(kind, 1, (0, 0), 0, None);
        assert!(none.credit().is_none());
    }

    /// A readable header has a known kind and a length from 1 to 11 that
    /// does not pass the producer index; a header of length 0 is never
    /// readable, so the receiver always moves at least one entry.
    #[kani::proof]
    fn a_readable_header_fits_in_what_was_produced() {
        let header = RingHeader {
            kind: kani::any(),
            length: kani::any(),
            sender: 0,
            sender_generation: 0,
            receiver_generation: 0,
            credit: 0,
        };
        let available: u64 = kani::any();
        if header.readable(available) {
            assert!(NodeMessage::from_raw(header.kind).is_some());
            assert!(header.length >= 1 && header.length as usize <= RING_MESSAGE_ENTRIES_MAX);
            assert!(header.length as u64 <= available);
        }
        if header.length == 0 {
            assert!(!header.readable(available));
        }
    }

    /// The largest messages fit: a `Send` of 64 words with 4 grants and a
    /// `Report` of the most items take at most eleven entries.
    #[kani::proof]
    fn the_largest_messages_fit_eleven_entries() {
        let words: usize = kani::any();
        kani::assume(words <= 64);
        assert!(message_entries(3 + SEND_GRANTS_MAX + words) <= RING_MESSAGE_ENTRIES_MAX);
        assert!(message_entries(1 + 3 * 28) <= RING_MESSAGE_ENTRIES_MAX);
        assert!(message_entries(0) == 1);
    }
}
