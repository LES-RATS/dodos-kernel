//! Port entries (x86-64), from the table of "Gate words" (D-152), the
//! ownership words: a device entry whose first word has bit 7 set is a
//! port entry (its first port and the log2 of its number of ports); the
//! port table, written once by the machine start-up, names each port's
//! entry. The mapping lookup matches only entries with bit 7 clear; an `in`
//! or `out` is executed only if the entry the port table names has bit 7
//! set, holds the port and is owned by the caller's node, or, for a port of
//! no entry, for the holder of `platform`. A port entry is never split.

use super::check::{Check, ensure};
use super::*;

/// A device entry: memory (bit 7 clear), its pages; or ports (bit 7 set),
/// its first port and the log2 of its number of ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeviceEntry {
    Memory { pages: u64 },
    Ports { first: u16, log2: u8 },
}

impl DeviceEntry {
    fn holds_port(self, port: u16) -> bool {
        match self {
            DeviceEntry::Ports { first, log2 } => {
                port >= first && u32::from(port) < u32::from(first) + (1u32 << log2)
            }
            DeviceEntry::Memory { .. } => false,
        }
    }
}

/// An access the gate executed (for the checker): the caller, its
/// generation, the port, the entry index and ownership word the port
/// table named then (or none), and the `platform` holder then.
pub(crate) type Access = (u16, u32, u16, Option<(usize, Option<(u16, u32)>)>, u16);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PortGate {
    /// The device entries, and the ownership word of each (*taken* by
    /// (node, generation), or *free*).
    pub(crate) entries: Vec<DeviceEntry>,
    pub(crate) owners: Vec<Option<(u16, u32)>>,
    /// The port table (global read-only set): port → entry.
    pub(crate) table: BTreeMap<u16, usize>,
    /// The holder of the `platform` role.
    pub(crate) platform: u16,
    pub(crate) executed: Vec<Access>,
}

impl PortGate {
    /// Two memory entries and two port entries (ports 0x60-0x63 and
    /// 0x70-0x71); port 0x80 lies in no entry.
    pub(crate) fn new() -> Self {
        let entries = vec![
            DeviceEntry::Memory { pages: 4 },
            DeviceEntry::Ports {
                first: 0x60,
                log2: 2,
            },
            DeviceEntry::Ports {
                first: 0x70,
                log2: 1,
            },
            DeviceEntry::Memory { pages: 1 },
        ];
        let mut table = BTreeMap::new();
        for (i, e) in entries.iter().enumerate() {
            if let DeviceEntry::Ports { first, log2 } = *e {
                for port in first..first + (1 << log2) {
                    table.insert(port, i);
                }
            }
        }
        Self {
            owners: vec![None; entries.len()],
            entries,
            table,
            platform: 0,
            executed: Vec::new(),
        }
    }
}

/// The ports the small machine's calls name.
pub(crate) const PORTS: [u16; 5] = [0x60, 0x63, 0x64, 0x71, 0x80];

impl Machine {
    /// `memory_take` of a device entry: *free* or *taken* by a generation
    /// that is over → *taken* (*X*, *g*).
    pub(crate) fn take_entry(&mut self, n: u16, entry: usize) -> Gated {
        self.unit_writes(n)?;
        require(entry < self.ports.entries.len())?;
        let takeable = match self.ports.owners[entry] {
            None => true,
            Some((o, g)) => {
                let m = self.gate.members[usize::from(o)];
                m.generation != g || m.state == NodeState::Dead
            }
        };
        require(takeable)?;
        let g = self.gate.members[usize::from(n)].generation;
        self.ports.owners[entry] = Some((n, g));
        Ok(())
    }

    /// `memory_give` of a device entry: *taken* (*X*, *g*) → *free*.
    pub(crate) fn give_entry(&mut self, n: u16, entry: usize) -> Gated {
        self.unit_writes(n)?;
        let g = self.gate.members[usize::from(n)].generation;
        require(self.ports.owners.get(entry) == Some(&Some((n, g))))?;
        self.ports.owners[entry] = None;
        Ok(())
    }

    /// `memory_split`: a device entry → two entries; bit 7 clear (a port
    /// entry is never split), counts 0 (the small machine maps no device
    /// memory, so its leaf and grant counts read 0).
    pub(crate) fn split_entry(&mut self, n: u16, entry: usize) -> Gated {
        self.unit_writes(n)?;
        let g = self.gate.members[usize::from(n)].generation;
        require(self.ports.owners.get(entry) == Some(&Some((n, g))))?;
        let DeviceEntry::Memory { pages } = self.ports.entries[entry] else {
            return Err(Refused::Condition);
        };
        require(pages >= 2)?;
        self.ports.entries[entry] = DeviceEntry::Memory { pages: pages / 2 };
        self.ports.entries.push(DeviceEntry::Memory {
            pages: pages - pages / 2,
        });
        self.ports.owners.push(Some((n, g)));
        Ok(())
    }

    /// The mapping lookup of device memory: only an entry with bit 7 clear,
    /// taken by the node.
    pub(crate) fn device_lookup(&self, n: u16, entry: usize) -> Gated {
        let g = self.gate.members[usize::from(n)].generation;
        require(self.ports.owners.get(entry) == Some(&Some((n, g))))?;
        require(matches!(
            self.ports.entries[entry],
            DeviceEntry::Memory { .. }
        ))
    }

    /// An `in` or an `out`, executed by the gate only if the port table's
    /// entry has bit 7 set, holds the port and is owned by the caller's
    /// node; a port of no entry only for the holder of `platform`.
    pub(crate) fn port_io(&mut self, n: u16, port: u16) -> Gated {
        self.unit_writes(n)?;
        let g = self.gate.members[usize::from(n)].generation;
        let named = self.ports.table.get(&port).copied();
        match named {
            Some(entry) => {
                require(self.ports.entries[entry].holds_port(port))?;
                require(self.ports.owners[entry] == Some((n, g)))?;
            }
            None => require(self.ports.platform == n)?,
        }
        let owner = named.map(|e| (e, self.ports.owners[e]));
        self.ports
            .executed
            .push((n, g, port, owner, self.ports.platform));
        Ok(())
    }

    /// One rule per row: every access executed was on a port whose entry
    /// the caller's generation held, with bit 7 set and the port in it, or
    /// of no entry by the `platform` holder; a port entry is never split
    /// (each still holds the ports the port table gives it).
    pub(super) fn check_ports(&self) -> Check {
        for &(n, g, port, entry_owner, platform) in &self.ports.executed {
            match entry_owner {
                Some((entry, owner)) => {
                    ensure(
                        self.ports.table.get(&port) == Some(&entry)
                            && self
                                .ports
                                .entries
                                .get(entry)
                                .is_some_and(|e| e.holds_port(port)),
                        "PORT: an access on a port whose entry does not hold it",
                    )?;
                    ensure(
                        owner == Some((n, g)),
                        "PORT: an access on a port whose entry its node does not hold",
                    )?;
                }
                None => {
                    ensure(
                        !self.ports.table.contains_key(&port) && platform == n,
                        "PORT: an access on a port of no entry by a node without platform",
                    )?;
                }
            }
        }
        for (&port, &entry) in &self.ports.table {
            ensure(
                self.ports.entries[entry].holds_port(port),
                "PORT: a port entry that no longer holds its ports",
            )?;
        }
        Ok(())
    }
}
