//! Rules of the machine table's words and of the start-up page that the
//! suite does not pin (found by mutation, see the review): a free word with
//! a generation, the width of a port access, a module that fits a large page
//! exactly, and the code a destroyed block of a reserve chunk keeps.

use dodos_abi::*;
use dodos_model::*;

const fn word(state: u64, owner: u16, generation: u32) -> u64 {
    let v = state | (owner as u64) << 2 | (generation as u64) << 18;
    let check = v.wrapping_mul(CHECK_MUL) >> 50;
    state | check << 2 | (owner as u64) << 16 | (generation as u64) << 32
}

#[test]
fn a_free_word_with_a_generation_is_refused() {
    // "No block covers the chunk; its generation is 0."
    let unclaimed = Owner::Unclaimed.code();
    let node = Owner::Node(3).code();
    assert!(Ownership::from_raw(word(1, unclaimed, 0)).is_some());
    assert_eq!(
        Ownership::from_raw(word(1, unclaimed, 0)),
        Some(Ownership::free(Owner::Unclaimed))
    );
    assert!(Ownership::from_raw(word(1, node, 0)).is_some());
    for generation in [1, 2, 0x8000_0000] {
        assert_eq!(
            Ownership::from_raw(word(1, unclaimed, generation)),
            None,
            "free word of generation {generation}"
        );
        assert_eq!(Ownership::from_raw(word(1, node, generation)), None);
    }
    // A word that is *taken* keeps its generation.
    assert_eq!(
        Ownership::from_raw(word(2, node, 5)),
        Some(Ownership::taken(Owner::Node(3), 5))
    );
}

#[test]
fn a_port_access_of_eight_bytes_is_refused() {
    // "width is 1, 2 or 4."
    let range = IoPortRange::new(0x3F8, 0x3FF).unwrap();
    assert_eq!(ioport_check(range, 0x3F8, 1, Some(0xFF)), Ok(()));
    assert_eq!(ioport_check(range, 0x3F8, 2, Some(0xFFFF)), Ok(()));
    assert_eq!(ioport_check(range, 0x3FC, 4, Some(0xFFFF_FFFF)), Ok(()));
    assert_eq!(ioport_check(range, 0x3F8, 8, Some(0)), Err(Error::Invalid));
    assert_eq!(ioport_check(range, 0x3FF, 2, Some(0)), Err(Error::Invalid));
}

#[test]
fn a_module_that_fits_a_large_page_exactly_keeps_one_frame() {
    let (m, _) = Model::boot(&Machine::example()).unwrap();
    let geometry = m.geometry();
    let large = geometry.bytes(PageSize::Large1).unwrap();
    // "the largest supported page size ... that is not longer than what
    // remains rounded up to a page": an exact fit takes the large size.
    let frames: Vec<_> = module_frames(0, None, large, geometry).collect();
    assert_eq!(
        frames,
        [ModuleFrame {
            size: PageSize::Large1,
            pages: 1,
            offset: 0,
        }]
    );
    // And a byte more is the large page followed by one page.
    let frames: Vec<_> = module_frames(0, None, large + 1, geometry).collect();
    assert_eq!(
        frames.iter().map(|f| (f.size, f.pages)).collect::<Vec<_>>(),
        [(PageSize::Large1, 1), (PageSize::Page, 1)]
    );
}

#[test]
fn a_destroyed_block_of_a_reserve_chunk_keeps_the_reserves_code() {
    // "Destroying a block writes *free* with the same owner into its words
    // (the reserve's code for a reserve chunk)."
    let (mut m, t) = Model::boot(&Machine::example()).unwrap();
    let shift = m.geometry().chunk_shift();
    let chunk = 1u64 << shift;
    let reserve = Ownership::free(Owner::Reserve(0)).raw();
    let bits = Machine::example().phys_bits;
    let at = (0..(1u64 << (bits - shift)))
        .map(|c| c << shift)
        .find(|&base| m.ownership(base >> shift) == reserve)
        .expect("a reserve chunk");
    // The role of the reserve is vacant at start-up: claim it first.
    let role = (21..=22)
        .map(SlotAddr::own)
        .find(|&role| matches!(m.apply(t, Syscall::RoleClaim(role)), Ok(Output::Epoch(_))))
        .expect("a reserve role");
    let _ = role;
    let take = Syscall::MemoryTake {
        control: SlotAddr::own(START_SLOT_MEMORY_CONTROL),
        kind: 0,
        place: at,
        size: u64::from(shift),
        destination: SlotAddr::own(120),
    };
    assert_eq!(m.apply(t, take), Ok(Output::Done));
    assert_eq!(m.check(), Ok(()));
    assert_eq!(
        m.ownership(at >> shift),
        Ownership::taken(Owner::Reserve(0), 1).raw(),
        "the block is taken with the reserve's code"
    );
    assert_eq!(
        m.apply(t, Syscall::CapDelete(SlotAddr::own(120))),
        Ok(Output::Done)
    );
    assert_eq!(m.check(), Ok(()));
    assert_eq!(
        m.ownership(at >> shift),
        reserve,
        "free with the reserve's code"
    );
    assert_eq!(chunk, 1 << shift);
}
