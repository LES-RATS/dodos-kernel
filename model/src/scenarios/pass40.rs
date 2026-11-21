//! Pass 40: D-168 (the sweep's cyclic cursor over sealed nodes) and the final
//! review's fixes on the model.

use super::*;
use dodos_abi::{Evidence, NODE_STOP_SEAL, SEAL_NS, SWEEP_NS};

fn seal(target: u16) -> Syscall {
    Syscall::NodeStop {
        node: node_cap(target),
        mode: NODE_STOP_SEAL,
        core: 0,
    }
}

/// D-168 (1): with one live node and three sealed nodes, each sweep reads
/// the seal instant of two sealed nodes from its cyclic cursor, so every
/// sealed node — including the third — is accused with evidence seal within
/// `ceil(s / 2) * SWEEP_NS + SEAL_NS`.
#[test]
fn sweep_cursor_accuses_third_sealed_node_with_evidence_seal() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    // Advance briefly after boot's initial sweep at t = 0 (next_sweep = SWEEP_NS),
    // then seal nodes 1, 2 and 3 so only node 0 stays live.
    m.run(10 * MS);
    for target in [1, 2, 3] {
        m.ok(0, seal(target));
    }
    m.run(10 * MS);
    for target in [1, 2, 3] {
        assert_eq!(m.state(target), MemberState::Sealed);
    }

    // Sweep 1 at SWEEP_NS happens before SEAL_NS has elapsed for the three
    // sealed nodes (sealed around t = 10 ms): it reads [1, 2] and advances the
    // cursor to 2, accusing none. Even past sealed_at + SEAL_NS (at t = 1.5 s),
    // no accusation happens until the next sweep.
    m.run(SWEEP_NS + 500 * MS - 20 * MS);
    for target in [1, 2, 3] {
        assert_eq!(
            m.state(target),
            MemberState::Sealed,
            "node {target} must stay sealed until a sweep reads it"
        );
    }

    // Sweep 2 at 2 * SWEEP_NS reads [3, 1] from cursor 2 (advancing cursor to
    // 1): node 3 (the third sealed node) and node 1 are accused with evidence
    // seal, while node 2 stays sealed because at most two are read per sweep.
    m.run(500 * MS + 20 * MS);
    assert_eq!(m.sys.member(3).evidence(), Evidence::Seal);
    assert_ne!(m.state(3), MemberState::Sealed);
    assert_eq!(m.sys.member(1).evidence(), Evidence::Seal);
    assert_ne!(m.state(1), MemberState::Sealed);
    assert_eq!(
        m.state(2),
        MemberState::Sealed,
        "sweep 2 reads only two sealed nodes ([3, 1]), leaving node 2 for sweep 3"
    );

    // Sweep 3 at 3 * SWEEP_NS reads the remaining sealed node (node 2) and
    // accuses it with evidence seal too.
    m.run(SWEEP_NS);
    assert_eq!(m.sys.member(2).evidence(), Evidence::Seal);
    assert_ne!(m.state(2), MemberState::Sealed);
    // All three were accused within ceil(3 / 2) * SWEEP_NS + SEAL_NS.
    assert!(m.sys.now() <= 2 * SWEEP_NS + SEAL_NS + 50 * MS);
}
