//! A machine of several nodes: one model per node, one machine table they
//! share, and the messages between kernels in flight, which arrive in the
//! order a test chooses (Memory and devices between nodes). Every event of
//! a node runs alone, so the states "waiting for another node" are entered
//! and left as on a machine whose cores interleave in that order.

use crate::boot::Machine;
use crate::remote::{Body, Packet};
use crate::table::GrantState;
use crate::{Model, ObjId, Outcome, Refusal, Syscall};
use std::collections::BTreeSet;

/// The machine: its nodes and the messages in flight.
#[derive(Clone, Debug, PartialEq)]
pub struct System {
    nodes: Vec<Model>,
    in_flight: Vec<Packet>,
}

/// `Err` names the invariant that fails.
type Check = Result<(), String>;

impl System {
    /// Every node started from its floor and the kept image, with the
    /// machine table of machine start-up; the first thread of each node.
    pub fn boot(machine: &Machine) -> Result<(Self, Vec<ObjId>), Refusal> {
        let layout = machine.layout()?;
        // The nodes start one after another, each on the table the one
        // before left; then all read the last.
        let mut table = machine.table(&layout);
        let mut nodes: Vec<Model> = Vec::new();
        let mut firsts = Vec::new();
        for node in 0..machine.nodes() as u16 {
            let (mut model, first) = Model::start(machine, &layout, table, node)?;
            model.shared = true;
            table = model.table.clone();
            nodes.push(model);
            firsts.push(first);
        }
        for model in &mut nodes {
            model.table = table.clone();
        }
        Ok((
            Self {
                nodes,
                in_flight: Vec::new(),
            },
            firsts,
        ))
    }

    /// The model of a node, to read it.
    pub fn node(&self, node: u16) -> &Model {
        &self.nodes[node as usize]
    }

    /// The messages in flight, in the order they were sent.
    pub fn pending(&self) -> &[Packet] {
        &self.in_flight
    }

    /// One call made by a thread of a node.
    pub fn apply(&mut self, node: u16, thread: ObjId, call: Syscall) -> Outcome {
        let outcome = self.nodes[node as usize].apply(thread, call);
        self.after(node);
        outcome
    }

    /// The message in flight at `index` arrives at its node.
    pub fn deliver(&mut self, index: usize) {
        let packet = self.in_flight.remove(index);
        let to = packet.to;
        self.nodes[to as usize].receive_packet(packet);
        self.after(to);
    }

    /// Delivers every message in flight, oldest first, as long as some are,
    /// at most `bound` of them; returns how many.
    pub fn settle(&mut self, bound: usize) -> usize {
        let mut delivered = 0;
        while !self.in_flight.is_empty() && delivered < bound {
            self.deliver(0);
            delivered += 1;
        }
        delivered
    }

    /// Time passes on every node, up to `to`.
    pub fn advance(&mut self, to: u64) {
        for node in 0..self.nodes.len() as u16 {
            self.nodes[node as usize].advance(to);
            self.after(node);
        }
    }

    /// After an event of `node`: its messages are in flight, and every node
    /// reads the machine table as that node left it.
    fn after(&mut self, node: u16) {
        let model = &mut self.nodes[node as usize];
        self.in_flight.append(&mut model.outbox);
        let table = model.table.clone();
        for other in &mut self.nodes {
            other.table = table.clone();
        }
    }

    /// Every invariant of every node, and those between nodes. That no
    /// chunk is covered by blocks of two nodes (8.2, invariant 2) follows
    /// from 53 and 54 on every node, which read the same words; that the
    /// memory under a revoked grant returns to an untyped only after the
    /// holder has no mapping under it (8.2, invariant 8) follows from 58 on
    /// the holder, which counts every leaf of granted memory in its use
    /// entry and requires the grant entry to be there.
    /// 58. every map count equals the leaves of every node that target its
    ///     chunk;
    /// 71. a grant entry is live or revoking only while its anchor is in the
    ///     granting node's kernel space, and only its grantee holds a
    ///     remote object of a live grant with its serial;
    /// 72. a use entry names a grant of another node, and is the use entry
    ///     of exactly one remote object of its node;
    /// 73. a remote object names a grant of another node, never a slot.
    pub fn check(&self) -> Check {
        for (index, node) in self.nodes.iter().enumerate() {
            node.check()
                .map_err(|broken| format!("node {index}: {broken}"))?;
        }
        let ensure = |holds: bool, invariant: &str| match holds {
            true => Ok(()),
            false => Err(invariant.to_string()),
        };
        let table = &self.nodes[0].table;
        let mut leaves = vec![0u64; table.maps.len()];
        for node in &self.nodes {
            for (sum, own) in leaves.iter_mut().zip(node.chunk_leaves()) {
                *sum += own;
            }
        }
        ensure(leaves == table.maps, "58: map count of the machine")?;
        for (granter, entries) in table.grants.iter().enumerate() {
            let node = &self.nodes[granter];
            for (&grant, entry) in entries {
                ensure(node.anchor(grant).is_some(), "71: grant without its anchor")?;
                // A remote object of this grant, with its serial, is held by
                // its grantee only.
                for (holder, uses) in table.uses.iter().enumerate() {
                    let held = uses.values().any(|u| {
                        u.granter == granter as u16 && u.grant == grant && u.serial == entry.serial
                    });
                    let live = entry.state == GrantState::Live;
                    ensure(
                        !held || !live || holder == entry.grantee as usize,
                        "71: remote object held by another node",
                    )?;
                }
            }
        }
        for (holder, uses) in table.uses.iter().enumerate() {
            let node = &self.nodes[holder];
            let named: BTreeSet<u16> = node.remotes.values().map(|r| r.use_entry).collect();
            let entries: BTreeSet<u16> = uses.keys().copied().collect();
            ensure(named == entries, "72: use entries and remote objects")?;
            for used in uses.values() {
                ensure(
                    used.granter as usize != holder,
                    "72: use of a grant of its own node",
                )?;
            }
            for remote in node.remotes.values() {
                ensure(
                    remote.node as usize != holder,
                    "73: remote object of its own node",
                )?;
            }
        }
        Ok(())
    }

    /// The bodies in flight, for a test to find the one it wants.
    pub fn find(&self, wanted: impl Fn(&Body) -> bool) -> Option<usize> {
        self.in_flight.iter().position(|p| wanted(&p.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{GrantEntry, UseEntry};
    use crate::{Message, Range, Transfer, live};
    use dodos_abi::{ObjectKind, PageSize, Rights, SlotAddr};

    const OBJ: u32 = 101;
    const GOT: u32 = 110;

    /// Two nodes; node 0 has granted node 1 a notify, whose remote
    /// capability is in slot `GOT` of node 1.
    fn granted() -> (System, Vec<ObjId>) {
        let (mut sys, t) = System::boot(&Machine::example()).unwrap();
        let retype = |kind, destination| Syscall::UntypedRetype {
            untyped: SlotAddr::own(25),
            kind,
            size: 0,
            page: PageSize::Page as u64,
            memory: 0,
            destination: SlotAddr::own(destination),
        };
        for node in 0..2 {
            assert_eq!(
                sys.apply(node, t[node as usize], retype(ObjectKind::Reply, 100)),
                Ok(crate::Output::Done)
            );
        }
        assert_eq!(
            sys.apply(0, t[0], retype(ObjectKind::Notify, OBJ)),
            Ok(crate::Output::Done)
        );
        let recv = Syscall::Recv {
            endpoint: SlotAddr::own(dodos_abi::START_SLOT_MAILBOX),
            reply: SlotAddr::own(100),
            range: Range {
                first: SlotAddr::own(GOT),
                count: 1,
            },
            wait: 0,
        };
        assert_eq!(sys.apply(1, t[1], recv), Ok(crate::Output::Waits));
        let all = Rights::ALL.without(Rights::EXEC).bits();
        let send = Syscall::Send {
            target: SlotAddr::own(17),
            message: Message {
                label: 1,
                flags: 0,
                words: vec![1],
                caps: vec![Transfer::Derive(SlotAddr::own(OBJ), all)],
                marks: 0,
            },
        };
        assert_eq!(sys.apply(0, t[0], send), Ok(crate::Output::Waits));
        assert_eq!(sys.settle(16), 4);
        assert_eq!(sys.check(), Ok(()));
        assert_eq!(sys.nodes[1].remotes.len(), 1);
        (sys, t)
    }

    /// Changes the machine table of every node the same way.
    fn tables(sys: &mut System, change: impl Fn(&mut crate::table::Table)) {
        for node in &mut sys.nodes {
            change(&mut node.table);
        }
    }

    /// Node 1 waits on its mailbox for one capability; node 0 sends it a
    /// derived capability for its notify. Returns the flags of the message
    /// node 1 gets.
    fn second_transfer(sys: &mut System, t: &[ObjId]) -> u32 {
        let recv = Syscall::Recv {
            endpoint: SlotAddr::own(dodos_abi::START_SLOT_MAILBOX),
            reply: SlotAddr::own(100),
            range: Range {
                first: SlotAddr::own(GOT + 1),
                count: 1,
            },
            wait: 0,
        };
        assert_eq!(sys.apply(1, t[1], recv), Ok(crate::Output::Waits));
        let send = Syscall::Send {
            target: SlotAddr::own(17),
            message: Message {
                label: 2,
                flags: 0,
                words: vec![2],
                caps: vec![Transfer::Derive(SlotAddr::own(OBJ), Rights::SEND.bits())],
                marks: 0,
            },
        };
        assert_eq!(sys.apply(0, t[0], send), Ok(crate::Output::Waits));
        sys.settle(16);
        match sys.nodes[1].woken(t[1]) {
            Some(Ok(crate::Output::Message(delivery))) => delivery.received.header.flags(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_full_grant_table_drops_the_capabilities_of_a_transfer() {
        let (mut sys, t) = granted();
        // Every other entry of node 0's grant table is taken.
        let entry = GrantEntry {
            grantee: 1,
            generation: 0,
            serial: 0,
            state: GrantState::Live,
        };
        tables(&mut sys, |t| {
            for grant in 1..dodos_abi::GRANTS as u16 {
                t.grants[0].insert(grant, entry);
            }
        });
        assert_eq!(
            second_transfer(&mut sys, &t),
            dodos_abi::Header::FLAG_CAPS_DROPPED
        );
    }

    #[test]
    fn a_full_use_table_on_the_receiver_drops_the_capabilities_and_frees_the_grant() {
        let (mut sys, t) = granted();
        let used = UseEntry {
            granter: 0,
            grant: 9,
            serial: 0,
            generation: 0,
            leaves: 0,
        };
        tables(&mut sys, |t| {
            for entry in 1..dodos_abi::GRANTS as u16 {
                t.uses[1].insert(entry, used);
            }
        });
        assert_eq!(
            second_transfer(&mut sys, &t),
            dodos_abi::Header::FLAG_CAPS_DROPPED
        );
        // The grant made for it was freed at the refusal: only grant 0 is
        // left on node 0.
        assert_eq!(sys.nodes[0].table.grants[0].len(), 1);
    }

    #[test]
    fn a_map_count_above_the_leaves_of_every_node_is_reported() {
        let (mut sys, _) = granted();
        let chunk = sys.nodes[0].table.maps.iter().position(|&m| m > 0).unwrap();
        tables(&mut sys, |t| t.maps[chunk] += 1);
        assert_eq!(sys.check(), Err("58: map count of the machine".into()));
    }

    #[test]
    fn a_map_count_below_the_leaves_of_one_node_is_reported() {
        let (mut sys, _) = granted();
        let chunk = sys.nodes[0].table.maps.iter().position(|&m| m > 0).unwrap();
        tables(&mut sys, |t| t.maps[chunk] = 0);
        assert_eq!(sys.check(), Err("node 0: 58: map count".into()));
    }

    #[test]
    fn a_grant_entry_without_its_anchor_is_reported() {
        let (mut sys, _) = granted();
        let entry = GrantEntry {
            grantee: 1,
            generation: 0,
            serial: 0,
            state: GrantState::Live,
        };
        tables(&mut sys, |t| {
            t.grants[0].insert(9, entry);
        });
        assert_eq!(sys.check(), Err("71: grant without its anchor".into()));
    }

    #[test]
    fn a_remote_object_of_a_live_grant_on_another_node_than_its_grantee_is_reported() {
        let (mut sys, _) = granted();
        tables(&mut sys, |t| live(&mut t.grants[0], 0).grantee = 0);
        assert_eq!(
            sys.check(),
            Err("71: remote object held by another node".into())
        );
    }

    #[test]
    fn a_use_entry_of_no_remote_object_is_reported() {
        let (mut sys, _) = granted();
        let used = UseEntry {
            granter: 0,
            grant: 0,
            serial: 0,
            generation: 0,
            leaves: 0,
        };
        tables(&mut sys, |t| {
            t.uses[1].insert(7, used);
        });
        assert_eq!(
            sys.check(),
            Err("72: use entries and remote objects".into())
        );
    }

    #[test]
    fn a_use_entry_of_a_grant_of_its_own_node_is_reported() {
        let (mut sys, _) = granted();
        tables(&mut sys, |t| {
            for used in t.uses[1].values_mut() {
                used.granter = 1;
            }
        });
        assert_eq!(
            sys.check(),
            Err("72: use of a grant of its own node".into())
        );
    }

    #[test]
    fn a_remote_object_of_its_own_node_is_reported() {
        let (mut sys, _) = granted();
        for remote in sys.nodes[1].remotes.values_mut() {
            remote.node = 1;
        }
        assert_eq!(sys.check(), Err("73: remote object of its own node".into()));
    }
}
