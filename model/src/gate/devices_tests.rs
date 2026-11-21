//! The scenarios of pass 29's gaps 31 to 38, settled by D-153: each breaks
//! the checker under the reading before D-153 (an `OldRules` switch) and
//! holds under the rule; kept as regression tests.

use super::faulty::Op;
use super::*;

/// Steps one by one, the checker after each; a refused step panics.
fn steps(m: &mut Machine, ops: &[Op]) -> Result<(), (usize, String)> {
    for (i, op) in ops.iter().enumerate() {
        m.step(op)
            .unwrap_or_else(|e| panic!("step {i}: {op:?} refused: {e:?}"));
        m.check().map_err(|rule| (i, rule))?;
    }
    Ok(())
}

/// The holder's whole service of node `slot`'s record `entry`: accepted,
/// submitted, completed, answered; then the requester's release.
fn serve(m: &mut Machine, slot: u16, entry: usize) -> Result<(), (usize, String)> {
    let h = m.unit.holder;
    steps(m, &[Op::Accept { h, slot, entry }, Op::Submit { h }])?;
    m.complete_all();
    m.check().map_err(|rule| (2, rule))?;
    let serial = m.unit.records[usize::from(slot)][entry].serial;
    let service = m
        .unit
        .services
        .iter()
        .position(|s| s.slot == slot && s.entry == entry && s.serial == serial)
        .expect("the service");
    steps(
        m,
        &[Op::Answer { h, service }, Op::Release { n: slot, entry }],
    )
}

/// The holder's R1 walk of `x` to its last unit.
fn walk(m: &mut Machine, x: u16) {
    let h = m.unit.holder;
    m.step(&Op::R1Start { h, x }).expect("the walk starts");
    while m.step(&Op::R1Unit { h, x }).is_ok() {}
}

fn with(old: OldRules) -> Machine {
    let mut m = Machine::new(2, 2, 8).with_units();
    m.old = old;
    m
}

// D-153, gap 32: node 1 empties one of its handles itself; its
// invalidation of the interrupt entry cache is asked and never served
// before it dies. A walk that invalidates only the handles it found present
// leaves the dead generation's entry cached.
#[test]
fn r1_invalidates_every_handle_of_the_dead_nodes_cores() {
    let run = |old| {
        let mut m = with(old);
        steps(
            &mut m,
            &[
                Op::ClaimMsi {
                    n: 1,
                    core: 2,
                    v: 0,
                },
                Op::RemoveMsi {
                    n: 1,
                    core: 2,
                    v: 0,
                    entry: 0,
                },
                Op::Death { n: 1 },
            ],
        )
    };
    assert_eq!(
        run(OldRules {
            r1_present_handles_only: true,
            ..OldRules::default()
        }),
        Err((
            2,
            "IEC: the unit caches an interrupt entry of a generation that is over".to_string()
        ))
    );
    run(OldRules::default()).expect("the tables");
}

// D-153, gap 33: R1 clears node 1's entry while the unit is healthy; the
// unit fails before the walk's invalidation is confirmed; R6 on the failed
// unit finds no present entry to retire.
#[test]
fn r6_on_a_failed_unit_retires_every_handle_of_the_nodes_cores() {
    let run = |old| {
        let mut m = with(old);
        // No device of node 1 attached, so R6 does not stop on one.
        steps(
            &mut m,
            &[Op::IoDetach {
                n: 1,
                device: 1,
                entry: 0,
            }],
        )?;
        serve(&mut m, 1, 0)?;
        steps(
            &mut m,
            &[
                Op::ClaimMsi {
                    n: 1,
                    core: 2,
                    v: 0,
                },
                Op::Fence { n: 1 },
            ],
        )?;
        walk(&mut m, 1);
        steps(&mut m, &[Op::UnitFails, Op::R6 { n: 1 }])
    };
    assert_eq!(
        run(OldRules {
            r6_retires_present_only: true,
            ..OldRules::default()
        }),
        Err((
            1,
            "IEC: the unit caches an interrupt entry of a generation that is over".to_string()
        ))
    );
    run(OldRules::default()).expect("the tables");
}

/// Node 1's `its_table` at page 8, device 1 mapped by the ITS.
fn its_mapped(old: OldRules) -> Machine {
    let mut m = with(old);
    steps(
        &mut m,
        &[
            Op::ItsRetype {
                n: 1,
                first: 8,
                k: 0,
            },
            Op::ItsClaim {
                n: 1,
                device: 1,
                first: 8,
                event: 0,
                entry: 1,
            },
        ],
    )
    .expect("claimed");
    serve(&mut m, 1, 1).expect("mapped");
    assert!(m.dev.its_mapped.contains_key(&1));
    m
}

// D-153, gap 34: device 1 detached while the ITS maps it; the identifier
// word freed with the attach word alone; R1 and R6, which find the node's
// words through its identifier words, skip the ITS word; the ITS writes
// into a page R6 turned back into data.
#[test]
fn an_identifier_word_stays_taken_while_its_its_word_is_not_0() {
    let run = |old| {
        let mut m = its_mapped(old);
        steps(
            &mut m,
            &[Op::IoDetach {
                n: 1,
                device: 1,
                entry: 0,
            }],
        )?;
        serve(&mut m, 1, 0)?;
        steps(&mut m, &[Op::Death { n: 1 }])
    };
    assert_eq!(
        run(OldRules {
            identifier_freed_by_attach_word: true,
            ..OldRules::default()
        }),
        Err((
            0,
            "G: the ITS writes into a page that is no pinned ITS table".to_string()
        ))
    );
    run(OldRules::default()).expect("the tables");
}

// D-153, gap 35: the unmapping asked before the mapping is served (with
// gap 38's check off, so the request is made); the holder issues the
// unmapping first, the mapping after, and writes the word *detached*.
#[test]
fn the_holder_serves_an_its_unmapping_only_after_the_mapping() {
    let start = |old: OldRules| {
        let mut m = with(OldRules {
            concurrent_device_requests: true,
            ..old
        });
        steps(
            &mut m,
            &[
                Op::ItsRetype {
                    n: 1,
                    first: 8,
                    k: 0,
                },
                Op::ItsClaim {
                    n: 1,
                    device: 1,
                    first: 8,
                    event: 0,
                    entry: 1,
                },
                Op::ItsUnmap {
                    n: 1,
                    device: 1,
                    entry: 0,
                },
            ],
        )
        .expect("both asked");
        m
    };
    let mut m = start(OldRules {
        its_unmap_unordered: true,
        ..OldRules::default()
    });
    let broken = steps(
        &mut m,
        &[
            Op::Accept {
                h: 0,
                slot: 1,
                entry: 0,
            },
            Op::Accept {
                h: 0,
                slot: 1,
                entry: 1,
            },
            Op::Submit { h: 0 },
            Op::UnitCompletes { id: 2 },
            Op::Answer { h: 0, service: 0 },
        ],
    );
    assert_eq!(
        broken,
        Err((
            4,
            "G: the ITS maps a device whose ITS word is not attached".to_string()
        ))
    );
    let mut m = start(OldRules::default());
    let accepted = m.step(&Op::Accept {
        h: 0,
        slot: 1,
        entry: 0,
    });
    assert_eq!(accepted, Err(Refused::Condition), "served before the map");
    steps(
        &mut m,
        &[
            Op::Refuse {
                h: 0,
                slot: 1,
                entry: 0,
            },
            Op::Release { n: 1, entry: 0 },
        ],
    )
    .expect("refused");
    // The mapping, its word now 0, is refused in turn.
    steps(
        &mut m,
        &[Op::Refuse {
            h: 0,
            slot: 1,
            entry: 1,
        }],
    )
    .expect("the mapping refused");
}

// D-153, gap 36: node 1 asks an attach and is fenced; R1's walk passes
// its identifier word (still *pinned*); the holder then serves the attach
// and R1 reads done while the unit translates for the device.
#[test]
fn the_holder_serves_no_record_of_a_fenced_requester() {
    let run = |old| {
        let mut m = with(old);
        steps(
            &mut m,
            &[
                Op::IoAttach {
                    n: 1,
                    device: 2,
                    root: 9,
                    entry: 1,
                },
                Op::Fence { n: 1 },
            ],
        )?;
        walk(&mut m, 1);
        let accepted = m.step(&Op::Accept {
            h: 0,
            slot: 1,
            entry: 1,
        });
        if accepted.is_err() {
            return Ok(false);
        }
        steps(&mut m, &[Op::Submit { h: 0 }])?;
        m.complete_all();
        steps(&mut m, &[Op::R1Done { h: 0, x: 1 }])?;
        Ok(true)
    };
    assert_eq!(
        run(OldRules {
            serve_fenced_requesters: true,
            ..OldRules::default()
        }),
        Err((
            0,
            "R1: the unit translates for a device of a generation whose R1 is done".to_string()
        ))
    );
    assert_eq!(run(OldRules::default()), Ok(false), "accepted");
}

// D-153, gap 37: the two cells of the gate word's table agree on safety.
// On a failed unit R6 stops while a device of the node is attached; once
// none is, R6 writes the counts 0 and no device translates for the node.
#[test]
fn r6_on_a_failed_unit_waits_for_no_attached_device() {
    let mut m = with(OldRules::default());
    steps(
        &mut m,
        &[
            Op::IoMap {
                n: 1,
                root: 9,
                e: 0,
                page: 12,
            },
            Op::UnitFails,
            Op::Fence { n: 1 },
        ],
    )
    .expect("the steps");
    assert_eq!(m.step(&Op::R6 { n: 1 }), Err(Refused::Condition));
    assert_eq!(m.gate.members[1].state, NodeState::Fenced);
    let mut m = with(OldRules::default());
    steps(
        &mut m,
        &[
            Op::IoMap {
                n: 1,
                root: 9,
                e: 0,
                page: 12,
            },
            Op::IoDetach {
                n: 1,
                device: 1,
                entry: 0,
            },
        ],
    )
    .expect("the steps");
    serve(&mut m, 1, 0).expect("detached");
    steps(
        &mut m,
        &[
            Op::IoUnmap {
                n: 1,
                root: 9,
                entry: 1,
                e: 0,
            },
            Op::UnitFails,
            Op::Release { n: 1, entry: 1 },
            Op::Death { n: 1 },
            Op::Fresh { n: 1 },
        ],
    )
    .expect("the kept count of the dead generation holds nothing");
    assert_eq!(m.gate.members[1].state, NodeState::Live);
    // R6 cleared the dead generation's kept counts (the cell Death).
    assert!(m.unit.kept.iter().all(|&(n, _, _)| n != 1));
    assert!(m.dev.context_cache.keys().all(|&d| d != 1));
}

// D-153, gap 38: a detach asked while the attach is in flight (the word
// already *attached*); after a take-over the new holder serves the detach,
// then the kept attach again, which writes the context entry anew.
#[test]
fn no_detach_while_the_attach_of_the_device_is_open() {
    let start = |old| {
        let mut m = with(old);
        steps(
            &mut m,
            &[
                Op::IoAttach {
                    n: 1,
                    device: 2,
                    root: 9,
                    entry: 1,
                },
                Op::Accept {
                    h: 0,
                    slot: 1,
                    entry: 1,
                },
                Op::Fence { n: 0 },
                Op::TakeOver { s: 1 },
            ],
        )
        .expect("the attach in flight");
        m
    };
    let mut m = start(OldRules {
        concurrent_device_requests: true,
        ..OldRules::default()
    });
    let broken = steps(
        &mut m,
        &[
            Op::IoDetach {
                n: 1,
                device: 2,
                entry: 0,
            },
            Op::Accept {
                h: 1,
                slot: 1,
                entry: 0,
            },
            Op::Accept {
                h: 1,
                slot: 1,
                entry: 1,
            },
            Op::Submit { h: 1 },
            Op::UnitCompletes { id: 2 },
            Op::Answer { h: 1, service: 0 },
        ],
    );
    assert_eq!(
        broken,
        Err((
            5,
            "G: the unit translates for a device whose attach word is not attached".to_string()
        ))
    );
    let mut m = start(OldRules::default());
    assert_eq!(
        m.step(&Op::IoDetach {
            n: 1,
            device: 2,
            entry: 0,
        }),
        Err(Refused::Condition)
    );
}

// D-153, gap 31: a free-list link may name a table freed or hooked since;
// the hook reads the hook state, never the link, so nothing follows it.
#[test]
fn a_stale_free_list_link_is_never_followed() {
    let mut m = Machine::new(2, 2, 8).with_spaces();
    steps(
        &mut m,
        &[
            Op::TakeTable { n: 0, page: 0 },
            Op::TakeTable { n: 0, page: 1 },
            Op::TableLink {
                n: 0,
                table: 0,
                next: Some(1),
            },
            Op::TableFree { n: 0, page: 1 },
        ],
    )
    .expect("setup");
    // Page 1 lies in node 0's table floor (mapped read-only, D-154), so a
    // kernel write to it must fault even after TableFree returns it to Data.
    assert_eq!(
        m.step(&Op::Write {
            n: 0,
            page: 1,
            value: 0xdead,
        }),
        Err(Refused::Condition)
    );
    steps(
        &mut m,
        &[Op::Hook {
            n: 0,
            root: 2,
            e0: 1,
            child: 0,
        }],
    )
    .expect("the stale link harms nothing");
    assert_eq!(m.gate.links.get(&0), None, "the hook clears its own link");
    assert_eq!(
        m.step(&Op::Hook {
            n: 0,
            root: 2,
            e0: 2,
            child: 1,
        }),
        Err(Refused::Cas),
        "a data page hooked through the link"
    );
}

// D-152 item 7, the ports: an `in` or `out` only on a port whose entry the
// caller's node holds (bit 7 set, the port in it), a port of no entry only
// for the holder of `platform`; a port entry never split, never matched by
// the mapping lookup. A dead generation's entry is taken again by rule 3.
#[test]
fn ports_are_executed_only_for_the_holder_of_their_entry() {
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &[Op::TakeEntry { n: 1, entry: 1 }]).expect("taken");
    assert_eq!(
        m.step(&Op::PortIo { n: 0, port: 0x60 }),
        Err(Refused::Condition)
    );
    assert_eq!(
        m.step(&Op::PortIo { n: 1, port: 0x64 }),
        Err(Refused::Condition)
    );
    assert_eq!(
        m.step(&Op::PortIo { n: 1, port: 0x80 }),
        Err(Refused::Condition)
    );
    assert_eq!(
        m.step(&Op::SplitEntry { n: 1, entry: 1 }),
        Err(Refused::Condition)
    );
    assert_eq!(
        m.step(&Op::DeviceLookup { n: 1, entry: 1 }),
        Err(Refused::Condition)
    );
    assert_eq!(
        m.step(&Op::TakeEntry { n: 0, entry: 1 }),
        Err(Refused::Condition)
    );
    steps(
        &mut m,
        &[
            Op::PortIo { n: 1, port: 0x63 },
            Op::PortIo { n: 0, port: 0x80 },
            Op::TakeEntry { n: 0, entry: 0 },
            Op::SplitEntry { n: 0, entry: 0 },
            Op::DeviceLookup { n: 0, entry: 4 },
            Op::Death { n: 1 },
            Op::TakeEntry { n: 0, entry: 1 },
            Op::PortIo { n: 0, port: 0x61 },
            Op::Spurious { intid: 1023 },
        ],
    )
    .expect("the tables");
    assert_eq!(
        m.step(&Op::Spurious { intid: 1019 }),
        Err(Refused::Condition)
    );
}
