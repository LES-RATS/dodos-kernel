//! What crosses between nodes (D-128, D-129, D-131): the eleven kinds of
//! message (numbered 1 to 12, 9 reserved), the two rings of each ordered pair of node slots, the header
//! at the start of a message's first entry, the credits and the bounds.
//!
//! The words that follow the header of each kind are the contract's table
//! "The words after the header, by kind" (What crosses between nodes,
//! gap 3), as the documentation of [`NodeMessage`] lists them, so that the
//! room a message takes in its ring is a function of the contract's values.

/// Bytes of one entry of a ring.
pub const RING_ENTRY_BYTES: usize = 64;
/// Entries of the kernel ring and of the user ring of an ordered pair.
pub const RING_KERNEL_ENTRIES: u64 = 64;
pub const RING_USER_ENTRIES: u64 = 64;
/// Entries one message takes at most (a `Report` of 28 items takes the
/// most; a `Send` of 64 words with 4 grants takes 10).
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
/// A `Report` key holds its kind in bits 61-63 and its identifier in bits
/// 0-60, so keys compare as numbers in the fixed order of a report.
pub const REPORT_KIND_SHIFT: u32 = 61;
/// The kinds of a `Report` key, as the contract numbers them (D-141, kept
/// by D-142).
pub const REPORT_KIND_DEVICE: u64 = 0;
pub const REPORT_KIND_HANDLE: u64 = 1;
pub const REPORT_KIND_LINE: u64 = 2;
pub const REPORT_KIND_ITS_DEVICE: u64 = 3;
pub const REPORT_KIND_ITS_EVENT: u64 = 4;

/// The ring a kind travels on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring {
    /// Kinds 1 to 10, the kernels' own messages.
    Kernel,
    /// Kinds 11 and 12, the messages user threads ask for.
    User,
}

/// The eleven kinds of message between nodes (numbered 1 to 12, 9 reserved
/// since D-135), and nothing else. The words after the header, by kind
/// (What crosses between nodes, gap 3):
///
/// - `Revoke`: the grant index (bits 0-11), its serial (12-41) and the use
///   index (42-53), one word.
/// - `RevokeStep`: 1 if the root remote capability was removed, else 0.
/// - `Release`, `Signal`: the grant index (0-11) and its serial (12-41).
/// - `UnitRequest`: the role (0-15), the epoch (16-44) and the operation
///   (48-55); then two operands, as the table of words by kind gives it.
///   CONTRACT-GAP 53 (What crosses between nodes, the table of words by
///   kind; Devices and shared hardware, the request record, D-151): since
///   D-151 the holder reads the operation and its operands from the
///   request record, and "The `UnitRequest` names the entry and its serial
///   (its operand words are 0 and unread)"; the table still gives the
///   words above and no place for the entry or the serial. Left as the
///   table gives it, three words.
/// - `UnitAnswer`: the result (`UnitResult`), then its value.
/// - `Handed`, `ReportAsk`: the role (0-15) and the epoch (16-44); a
///   `ReportAsk` adds the key it asks from.
/// - `Report`: the number of items (bits 0-4, at most 28); then the key of
///   the next item, or `REPORT_END`; then three words per item, its key
///   first (`REPORT_KIND_SHIFT`).
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
    // 9 is reserved: it was the sweep's `Ping`, removed by D-135, and is
    // never used again.
    SendAnswer = 10,
    Send = 11,
    Signal = 12,
}

impl NodeMessage {
    /// The eleven kinds, in the order of their numbers (9 reserved).
    pub const ALL: [Self; 11] = [
        Self::Revoke,
        Self::RevokeStep,
        Self::Release,
        Self::UnitRequest,
        Self::UnitAnswer,
        Self::Handed,
        Self::ReportAsk,
        Self::Report,
        Self::SendAnswer,
        Self::Send,
        Self::Signal,
    ];

    /// The kind of number `raw`, `None` for a kind the receiver does not
    /// know (0, the reserved 9, and every number above 12).
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            1..=8 => Some(Self::ALL[raw as usize - 1]),
            10..=12 => Some(Self::ALL[raw as usize - 2]),
            _ => None,
        }
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

/// The consumer index the receiver starts from (What crosses between
/// nodes): a producer index behind the consumer index moves the consumer to
/// the producer (D-141); a producer index more than the ring's entries ahead
/// of the consumer moves the consumer to the producer minus the ring's
/// entries (D-131); otherwise it stays. Every difference is taken modulo
/// 2^64.
///
/// CONTRACT-GAP 52 (What crosses between nodes, "every difference of two
/// indices is taken modulo 2^64"): modulo 2^64 a producer behind the
/// consumer is also more than the ring's entries ahead of it, and taken in
/// the order written ("first moves") the behind rule never applies. Read as
/// the model read it since D-141: behind when the consumer leads by 1 to
/// the ring's entries; a consumer further ahead reads as a producer more
/// than the ring's entries ahead.
pub const fn ring_resync(produced: u64, consumed: u64, entries: u64) -> u64 {
    let behind = consumed.wrapping_sub(produced);
    if behind != 0 && behind <= entries {
        produced
    } else if produced.wrapping_sub(consumed) > entries {
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
            Some((index, serial)) => {
                // A credit is an index below 24 (D-141).
                assert!((index as usize) < REQUESTS_PER_PEER + SENDS_PER_PEER);
                index as u32 | (serial & CREDIT_SERIAL_MASK) << 5
            }
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

    /// The credit and its serial, `None` for a message without one or for
    /// an index that is no credit: 0 to 15 are the request credits, 16 to
    /// 23 the send credits, 31 none, and no other value is a credit (D-141).
    pub const fn credit(&self) -> Option<(u8, u32)> {
        let index = (self.credit & 0x1f) as u8;
        if index as usize >= REQUESTS_PER_PEER + SENDS_PER_PEER {
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
/// idempotent, numbered as the contract numbers them (Devices and shared
/// hardware, gap 3). The two operands are words of the request record,
/// which the holder's gate reads in place of the ring (D-151).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitOperation {
    /// Attach a device to an iospace: operand 1 the device identifier
    /// (bits 0-15) and the domain number (16-31), from the requester's
    /// node slot's range; operand 2 the physical address of the iospace's
    /// root table (D-134).
    Attach = 1,
    /// Detach a device: operand 1 the device identifier (0-15) and the
    /// domain number (16-31); operand 2 the physical address of the
    /// iospace's root table the requester's header entry names (D-140; 0
    /// before).
    Detach = 2,
    /// Invalidate the IOMMU translations of a domain, after an unmap or the
    /// unhooking of a table: operand 1 the domain number (0-15).
    Invalidate = 3,
    /// Invalidate the interrupt entry cache after a remapping entry:
    /// operand 1 the handle (bits 0-15), index-selective (D-134).
    InterruptCache = 4,
    /// Program a line's redirection entry in an IOAPIC: operand 1 the GSI
    /// (0-31), 1 claimed or 0 masked (32), the trigger (33), the polarity
    /// (34) and *release* (35, masked only: the holder's gate writes the
    /// line word free, unclaimed, in the call that masks; D-150); operand 2 remapped the handle (0-15) and the vector
    /// (16-23), not remapped the vector (16-23) and the APIC ID (32-63),
    /// masked 0 (D-134).
    Redirection = 5,
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// A header's credit is a request or a send credit, or none (D-141).
    #[kani::proof]
    fn a_credit_is_one_of_the_credits() {
        let header = RingHeader {
            kind: kani::any(),
            length: kani::any(),
            sender: kani::any(),
            sender_generation: kani::any(),
            receiver_generation: kani::any(),
            credit: kani::any(),
        };
        if let Some((index, serial)) = header.credit() {
            assert!((index as usize) < REQUESTS_PER_PEER + SENDS_PER_PEER);
            assert!(serial == header.credit >> 5);
        }
    }

    /// The ring indices: a message that fits leaves the producer at most
    /// the ring's entries ahead of the consumer, and its reserve free; a
    /// resynchronised consumer is never more than the ring's entries behind
    /// the producer and never moves back while the ring is sane.
    ///
    /// What the assumption `sane` leaves unproven: the consequences of
    /// `ring_fits` (the producer stays within the ring's entries of the
    /// consumer, the reserve stays free) for a ring whose producer index is
    /// already more than its entries ahead of its consumer. There they are
    /// false (a message of no entry fits a ring already beyond its
    /// entries), and they need not hold: `ring_fits` is called only by the
    /// sender on its own ring, whose producer only it writes and which it
    /// keeps sane. A ring written by a faulty node is the receiver's
    /// concern, and the receiver never calls `ring_fits` on it: it calls
    /// `ring_resync`, whose two assertions below hold for every pair of
    /// indices, sane or not (the producer is never more than the ring's
    /// entries ahead of the resynchronised consumer), and reads each header
    /// by the rule `a_readable_header_fits_in_what_was_produced` proves.
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
        // A sender's own ring is sane: never more than its entries ahead.
        let sane = produced.wrapping_sub(consumed) <= entries;
        if sane && ring_fits(produced, consumed, entries, length, reserve) {
            let after = produced.wrapping_add(length);
            assert!(after.wrapping_sub(consumed) <= entries);
            assert!(ring_free(after, consumed, entries) >= reserve);
        }
        let start = ring_resync(produced, consumed, entries);
        assert!(produced.wrapping_sub(start) <= entries);
        if produced.wrapping_sub(consumed) <= entries {
            assert!(start == consumed);
        }
        // A producer behind the consumer moves the consumer to it (D-141,
        // read by CONTRACT-GAP 52): nothing is read that was not produced.
        let behind = consumed.wrapping_sub(produced);
        if behind != 0 && behind <= entries {
            assert!(start == produced);
        }
    }

    /// A credit (an index below 24, D-141) and its serial read back, and a
    /// header with no credit reads none.
    #[kani::proof]
    fn a_header_reads_back_its_credit() {
        let (index, serial): (u8, u32) = (kani::any(), kani::any());
        kani::assume((index as usize) < REQUESTS_PER_PEER + SENDS_PER_PEER);
        kani::assume(serial <= CREDIT_SERIAL_MASK);
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

    /// The largest messages fit: a `Send` of 64 words with 4 grants takes
    /// ten entries and a `Report` of the most items eleven.
    #[kani::proof]
    fn the_largest_messages_fit_eleven_entries() {
        let words: usize = kani::any();
        kani::assume(words <= 64);
        assert!(message_entries(3 + SEND_GRANTS_MAX + words) <= 10);
        // A `Report`: the count, the key of the next item, three words per
        // item, 2 + 3 × 28 = 86 words (F-10: the proof counted 85).
        assert!(message_entries(2 + 3 * 28) <= RING_MESSAGE_ENTRIES_MAX);
        assert!(message_entries(0) == 1);
    }
}
