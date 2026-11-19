//! Drop bits (ARM64) and the seal's park requests, from the tables of
//! "Gate words" (D-152): the line word of each SPI, the GIC's active state,
//! each node's drop bits keyed on the line, and the park request, the
//! cores' parks and `seal_parked` of a seal.

use super::*;

/// The SPIs of the small machine.
pub(crate) const SPIS: usize = 4;

/// A deactivation a gate made: the node, the SPI, the line word then.
pub(crate) type Deactivation = (u16, usize, Option<(u16, u32)>);

/// The words of the interrupt and park side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParkGate {
    /// The line word of each SPI: the node and generation that took it.
    pub(crate) lines: [Option<(u16, u32)>; SPIS],
    /// The GIC's active state of each SPI (hardware).
    pub(crate) active: [bool; SPIS],
    /// Per node: its drop bits (gate part), one per SPI.
    pub(crate) drop: Vec<[bool; SPIS]>,
    /// The deactivations the gates made: (node, SPI, the line word then).
    pub(crate) deactivated: Vec<Deactivation>,
    /// Per physical core: the park request, `seal_parked`, and whether the
    /// core is parked.
    pub(crate) park_requests: Vec<u64>,
    pub(crate) seal_parked: Vec<u64>,
    pub(crate) parked: Vec<bool>,
}

impl ParkGate {
    pub(crate) fn new(nodes: u16, cores: usize) -> Self {
        let all = usize::from(nodes) * cores;
        Self {
            lines: [None; SPIS],
            active: [false; SPIS],
            drop: vec![[false; SPIS]; usize::from(nodes)],
            deactivated: Vec::new(),
            park_requests: vec![0; all],
            seal_parked: vec![0; all],
            parked: vec![false; all],
        }
    }
}

impl Machine {
    /// The line word takeable → *taken* (*X*, *g*): `irq_claim`; the line's
    /// drop bit cleared in the same call.
    pub(crate) fn claim_spi(&mut self, n: u16, spi: usize) -> Gated {
        require(self.gate.members[usize::from(n)].state.writes() && spi < SPIS)?;
        let takeable = match self.parks.lines[spi] {
            None => true,
            Some((owner, g)) => {
                let m = self.gate.members[usize::from(owner)];
                m.generation != g || m.state == NodeState::Dead
            }
        };
        require(takeable)?;
        let g = self.gate.members[usize::from(n)].generation;
        self.parks.lines[spi] = Some((n, g));
        // The line bitmap's bit set in the same call; the bits of lines the
        // node no longer holds cleared lazily by its own gate, here.
        let lines = self.parks.lines;
        let bits = &mut self.dev.line_bits[usize::from(n)];
        bits.retain(|&l| lines[l] == Some((n, g)));
        bits.insert(spi);
        if !self.old.drop_bit_per_intid {
            self.parks.drop[usize::from(n)][spi] = false;
        }
        Ok(())
    }

    /// A level SPI asserted (hardware): it becomes active, and the node
    /// its line word names takes the arrival call: the drop bit set.
    pub(crate) fn spi_arrives(&mut self, spi: usize) -> Gated {
        require(spi < SPIS && !self.parks.active[spi])?;
        self.parks.active[spi] = true;
        if let Some((n, g)) = self.parks.lines[spi]
            && self.gate.members[usize::from(n)].generation == g
            && self.gate.members[usize::from(n)].state.writes()
        {
            self.parks.drop[usize::from(n)][spi] = true;
        }
        Ok(())
    }

    /// The drop bit 1 → 0: `irq_ack` or an arming bind, which writes
    /// `ICC_DIR_EL1` for that SPI; the line word names the node with its
    /// generation.
    pub(crate) fn ack_spi(&mut self, n: u16, spi: usize) -> Gated {
        require(self.gate.members[usize::from(n)].state.writes() && spi < SPIS)?;
        require(self.parks.drop[usize::from(n)][spi])?;
        let g = self.gate.members[usize::from(n)].generation;
        if !self.old.drop_bit_per_intid {
            require(self.parks.lines[spi] == Some((n, g)))?;
        }
        self.parks.drop[usize::from(n)][spi] = false;
        self.parks.deactivated.push((n, spi, self.parks.lines[spi]));
        self.parks.active[spi] = false;
        Ok(())
    }

    /// The line word *taken* → *free*: the controller holder's call that
    /// writes the masked entry, at a removal (the holder is node 0 here).
    pub(crate) fn remove_spi(&mut self, n: u16, spi: usize) -> Gated {
        require(spi < SPIS)?;
        let g = self.gate.members[usize::from(n)].generation;
        require(self.parks.lines[spi] == Some((n, g)))?;
        self.parks.lines[spi] = None;
        Ok(())
    }

    /// The call that writes sealing: live → sealing, and (D-152) the park
    /// request of every core whose word names the node raised in it.
    pub(crate) fn sealing(&mut self, n: u16) -> Gated {
        require(self.gate.members[usize::from(n)].state == NodeState::Live)?;
        self.gate.members[usize::from(n)].state = NodeState::Sealing;
        if !self.old.park_requests_by_later_call {
            self.raise_park_requests(n);
        }
        Ok(())
    }

    /// The old text's separate call that raises the park requests, which a
    /// faulty kernel may skip.
    pub(crate) fn raise_park_requests(&mut self, n: u16) {
        for p in 0..self.layout[usize::from(n)].cores.len() {
            let core = self.core(n, p);
            self.parks.park_requests[core] += 1;
        }
    }

    /// A core's own park call, which does not return: `seal_parked`
    /// written with the request read; sealing with its generation.
    pub(crate) fn park_call(&mut self, n: u16, p: usize) -> Gated {
        require(self.gate.members[usize::from(n)].state == NodeState::Sealing)?;
        let core = self.core(n, p);
        self.parks.seal_parked[core] = self.parks.park_requests[core];
        self.parks.parked[core] = true;
        Ok(())
    }

    /// The last call of a seal: sealed, once every other core of the node
    /// reads `seal_parked` equal to its park request.
    pub(crate) fn sealed_call(&mut self, n: u16, p: usize) -> Gated {
        require(self.gate.members[usize::from(n)].state == NodeState::Sealing)?;
        let confirmed = (0..self.layout[usize::from(n)].cores.len())
            .filter(|&q| q != p)
            .all(|q| {
                let core = self.core(n, q);
                self.parks.seal_parked[core] == self.parks.park_requests[core]
            });
        require(confirmed)?;
        let core = self.core(n, p);
        self.parks.seal_parked[core] = self.parks.park_requests[core];
        self.parks.parked[core] = true;
        self.gate.members[usize::from(n)].state = NodeState::Sealed;
        Ok(())
    }
}
