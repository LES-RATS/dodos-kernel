use dodos_abi::{MemberState, SlotAddr};
use dodos_model::{Machine, Syscall, System};

#[test]
fn smoke() {
    let machine = Machine::example();
    let (mut sys, t) = System::boot(&machine).unwrap();
    sys.check().unwrap();
    sys.settle(1000);
    sys.check().unwrap();
    sys.advance(2_000_000_000);
    sys.check().unwrap();
    assert_eq!(sys.member(1).state(), MemberState::Live);
    // node 0 kills node 1
    let kill = Syscall::NodeStop {
        node: SlotAddr::own(17),
        mode: 0,
        core: 0,
    };
    assert_eq!(sys.apply(0, t[0], kill), Ok(dodos_model::Output::Done));
    sys.check().unwrap();
    for _ in 0..200 {
        sys.advance(sys.now() + 1_000_000);
    }
    sys.check().unwrap();
    assert_eq!(
        sys.member(1).state(),
        MemberState::Dead,
        "{:?}",
        sys.member(1)
    );
}
