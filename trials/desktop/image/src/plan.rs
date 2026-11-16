//! The steps of the run, in order (README, "The plan").

use crate::run::{Kind, Step};
use crate::{cet, gate, identity, ipi, platform, setup, shoot, stall};
use alloc::vec;
use alloc::vec::Vec;

pub fn steps() -> Vec<Step> {
    vec![
        Step { name: "setup", kind: Kind::Setup, limit_s: 60, run: setup::run },
        Step { name: "identity", kind: Kind::Measure, limit_s: 120, run: identity::run },
        Step { name: "platform", kind: Kind::Measure, limit_s: 120, run: platform::run },
        Step { name: "stall", kind: Kind::Measure, limit_s: 120, run: stall::run },
        Step { name: "gate", kind: Kind::Measure, limit_s: 300, run: gate::run },
        Step { name: "cet", kind: Kind::Measure, limit_s: 300, run: cet::run },
        Step { name: "ipi", kind: Kind::Measure, limit_s: 900, run: ipi::run },
        Step { name: "shoot", kind: Kind::Measure, limit_s: 900, run: shoot::run },
    ]
}
