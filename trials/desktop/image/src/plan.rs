//! The steps of the run, in order (README, "The plan").

use crate::run::{Kind, Step};
use crate::{gate, identity, setup};
use alloc::vec;
use alloc::vec::Vec;

pub fn steps() -> Vec<Step> {
    vec![
        Step { name: "setup", kind: Kind::Setup, limit_s: 60, run: setup::run },
        Step { name: "identity", kind: Kind::Measure, limit_s: 120, run: identity::run },
        Step { name: "gate", kind: Kind::Measure, limit_s: 300, run: gate::run },
    ]
}
