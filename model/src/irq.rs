//! Interrupts: the entries of the interrupt table of each physical core of
//! the node (D-129), their claim, binding and acknowledgement, and the
//! arrival of an interrupt on its core, an event under the node lock
//! (D-138).

use crate::caps::Placing;
use crate::{Irq, IrqState, Model, ObjId, Outcome, Output, live};
use dodos_abi::x86_64::{
    IRQ_MAX_CORES_REMAPPED, IRQ_TABLE_ENTRIES, IRQ_VECTOR_FIRST, IRQ_VECTOR_LAST,
};
use dodos_abi::{Error, IrqSource, ObjectKind, Rights, SlotAddr};

/// The APLIC registers a node's gate writes by number (G9, D-146).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AplicReg {
    /// `setipnum`, offset 0x1CDC.
    SetIp,
    /// `setienum`, offset 0x1EDC.
    SetIe,
    /// `clrienum`, offset 0x1FDC.
    ClrIe,
}

impl Model {
    /// `irq_claim`: one vector of one physical core of the node, bound to
    /// one source. Returns the address and the data a device must be given
    /// for an MSI (0 and 0 for a line). A line's redirection entry is
    /// programmed by the holder of the IOAPIC's role, and a remapping entry
    /// is followed by the invalidation of the interrupt entry cache by the
    /// holder of the unit's role: unit requests, which the call waits for.
    pub(crate) fn irq_claim(
        &mut self,
        thread: ObjId,
        control: SlotAddr,
        (source, core, vector): (u64, u64, u64),
        destination: SlotAddr,
    ) -> Outcome {
        let cap = self.cap(thread, control, Rights::READ)?;
        self.is(cap, ObjectKind::IrqControl, Rights::MANAGE)?;
        let place = self.destination(thread, destination)?;
        // The plain arguments, after the destination: the source as its
        // format allows, a GSI that an IOAPIC serves (judged with the
        // source), then the core number, then the vector (O12).
        let source = IrqSource::from_raw(source)?;
        let served = |gsi: u32| {
            self.machine
                .ioapics
                .iter()
                .any(|io| (io.first_gsi..io.first_gsi + io.inputs).contains(&gsi))
        };
        if source.is_line() && !served(source.id()) {
            return Err(Error::Invalid);
        }
        // Then a core number that is not a core of the irq_control's node.
        let position = self.position(core).ok_or(Error::Invalid)?;
        let vector = u8::try_from(vector)
            .ok()
            .filter(|vector| (IRQ_VECTOR_FIRST..=IRQ_VECTOR_LAST).contains(vector));
        let vector = vector.ok_or(Error::Invalid)?;
        // `unsupported` if this machine cannot deliver the source to the
        // core: without remapping, an APIC ID above 255; with remapping, a
        // core of number 341 or more.
        let apic = self.machine.core_list()[core as usize].hw_id;
        let remapped_core = core >= u64::from(IRQ_MAX_CORES_REMAPPED);
        // An APIC ID of 255 is the broadcast (Intel SDM §13.6.2.1, D-141).
        if !self.machine.remapping && apic >= 255 || self.machine.remapping && remapped_core {
            return Err(Error::Unsupported);
        }
        // A level line claimed on a node that does not hold its
        // controller's role is held by the controller until acknowledged:
        // on x86-64 only where the end of interrupt is directed, on ARM64 by
        // the SPI's active state; on RISC-V by the mask at its arrival
        // (Interrupts, Nodes, D-141, D-146).
        if source.level() {
            let holder = self
                .ioapic_role(source.id())
                .map(|r| self.table.roles[r].holder());
            let held_here = holder == Some(self.node);
            // RISC-V: on any node, its gate writing the APLIC's registers
            // by number (D-146).
            let held_away = match self.machine.arch {
                crate::Arch::X86_64 => self.machine.directed_eoi,
                crate::Arch::Aarch64 | crate::Arch::Riscv64 => true,
            };
            if !held_here && !held_away {
                return Err(Error::Unsupported);
            }
            // x86-64 without a directed end of interrupt: mask then unmask
            // by the holder, so a level line is claimable only on a machine
            // where one node runs, one slot or the others phantom (D-148).
            if self.masks_at_arrival() && !self.one_node_runs() {
                return Err(Error::Unsupported);
            }
        }
        // A line beyond the last line word (Interrupts, D-135).
        if source.is_line() && source.id() as usize >= dodos_abi::IRQ_LINES_MAX {
            return Err(Error::Unsupported);
        }
        // Unlike cores: without such an event, kind 3 is `unsupported`.
        if source.is_core_event() && !self.machine.core_events {
            return Err(Error::Unsupported);
        }
        // The depth checks come last.
        self.check_fits(place.space, cap, Placing::Retyped(ObjectKind::Irq))?;
        // The state: the vector claimed, a line whose input is claimed, then
        // the destination.
        let claimed =
            |vector_or_line: &dyn Fn(&Irq) -> bool| self.irqs.values().any(vector_or_line);
        if claimed(&|irq| irq.core == position && irq.vector == vector) {
            #[cfg(test)]
            if self
                .irqs
                .values()
                .filter(|irq| irq.core == position)
                .count()
                >= dodos_abi::x86_64::IRQ_TABLE_ENTRIES
            {
                self.bound_hits.0.irq_193rd_refused = true;
            }
            return Err(Error::Busy);
        }
        // A vector whose bit is set in the core's retired word, in the
        // living part: a failed unit may still hold its remapping entry
        // (Interrupts, the irq, D-139, D-140).
        let handle = remap_handle(u64::from(self.cores[position].number), vector);
        if self.table.retired.contains(&handle) {
            return Err(Error::Busy);
        }
        // A line whose word the rules of taking refuse to this node, or that
        // this node's current generation already holds: claimed on any core
        // and any vector of the machine (Interrupts, the owner word of a
        // line, D-135).
        // (The rules of taking refuse a word this node's generation holds.)
        let generation = self.table.generation(self.node);
        let line_taken = source.is_line()
            && !self
                .table
                .may_take(self.table.line_word(source.id()), self.node);
        if line_taken {
            return Err(Error::Busy);
        }
        self.vacant(place)?;
        // The unit requests the claim needs, and a free entry of the unit
        // request table for them (`again` with nothing changed).
        // An MSI's remapping entry is followed by the invalidation of the
        // interrupt entry cache of each unit that serves the source's
        // segment, one after another (Devices and shared hardware, several
        // remapping units, gap 12): the first unit is asked here, the next
        // ones at the done of the one before.
        // A line's remapping entry is invalidated in the units of its
        // IOAPIC's segment before the IOAPIC's redirection entry is asked
        // (Interrupts, the interrupt remapping cache, D-134).
        let segment = match source.is_line() {
            true => self
                .machine
                .ioapics
                .iter()
                .find(|io| (io.first_gsi..io.first_gsi + io.inputs).contains(&source.id()))
                .map_or(0, |io| io.segment),
            false => source.segment(),
        };
        // A core event is raised by no device: it writes no remapping entry
        // and asks no unit, and its claim returns 0 and 0 (D-141; the final
        // review's verification of lot C, H7).
        let units = match self.machine.remapping && !source.is_core_event() {
            true => self.segment_roles(segment),
            false => Vec::new(),
        };
        let ioapic = source
            .is_line()
            .then(|| self.ioapic_role(source.id()))
            .flatten();
        let role = units.first().copied().or(ioapic);
        if role.is_some() && self.unit_requests_full() {
            return Err(Error::Again);
        }
        // The new `irq` is idle, with pending 0; its root capability is a
        // child of the `irq_control` capability, with every right, and it
        // takes no memory from an untyped.
        let root = self.create(ObjectKind::Irq, 0, Some(cap), Rights::ALL, place);
        let id = self.caps[&root].object;
        live(&mut self.objects, id).from = None;
        let irq = Irq {
            core: position,
            vector,
            source,
            state: IrqState::Idle,
            pending: false,
            notify: None,
            removing: false,
        };
        self.irqs.insert(id, irq);
        let kept = crate::table::IrqTableEntry {
            node: self.node,
            generation,
            units: units.clone(),
        };
        self.table.irq_tables.insert(handle, kept);
        if source.is_line() {
            let taken = dodos_abi::Ownership::taken(dodos_abi::Owner::Node(self.node), generation);
            self.table.write_line(self.node, source.id(), Some(taken));
        }
        // The fixed remapping entry of the vector on that core: handle
        // core number × 192 + (vector − 48), written by this node's gate.
        let entry = u64::from(vector - IRQ_VECTOR_FIRST);
        let handle = u64::from(remap_handle(core, vector));
        let (address, data) = match (source.is_line(), self.machine.remapping) {
            _ if source.is_core_event() => (0, 0),
            (true, _) => (0, 0),
            (false, true) => (dodos_abi::x86_64::msi_remapped(handle), 0),
            (false, false) => {
                dodos_abi::x86_64::msi_direct(apic as u32, vector).expect("checked above")
            }
        };
        assert!(entry < IRQ_TABLE_ENTRIES as u64);
        let Some(role) = role else {
            return Ok(Output::Msi { address, data });
        };
        for &unit in &units {
            let remap = &mut self.table.units.entry(unit).or_default().remap;
            remap.insert(handle as u32, (self.node, generation));
        }
        let target = self.line_target(position, vector);
        let redirection = ioapic.map(|io| {
            let op = crate::rings::UnitOp::Redirection {
                gsi: source.id(),
                claimed: true,
                release: false,
                level: source.level(),
                low: source.active_low(),
                target,
            };
            (io, op)
        });
        let op = match units.is_empty() {
            false => crate::rings::UnitOp::InterruptCache {
                handle: handle as u16,
            },
            true => redirection.expect("a line without remapping").1,
        };
        let waiter = crate::units::Waiter::Thread(Some(thread));
        let chain = match units.is_empty() {
            false => crate::units::Chain {
                segment: Some(segment),
                then: redirection,
            },
            true => crate::units::Chain::default(),
        };
        let words = Some((address, data));
        self.unit_request_with(role, op, waiter, None, words, chain)?;
        // Served at once (a unit or an IOAPIC this node holds, or a failed
        // unit passed over): the call ends with its result.
        let claimed = live(&mut self.threads, thread);
        if claimed.wait == crate::Wait::None && claimed.answered {
            claimed.answered = false;
            return claimed.outcome.take().unwrap_or(Ok(Output::Waits));
        }
        Ok(Output::Waits)
    }

    /// `irq_bind`: notify 0 unbinds; otherwise `send` on the notify, and
    /// `busy` if the irq is bound. Binding records the bits; a pending
    /// arrival is signalled at once and the irq is fired, otherwise armed.
    /// Unbinding makes it idle and keeps pending.
    pub(crate) fn irq_bind(&mut self, thread: ObjId, irq: SlotAddr, notify: SlotAddr) -> Outcome {
        let irq = self.object(thread, irq, ObjectKind::Irq, Rights::MANAGE)?;
        let Some(notify) = (!notify.is_none()).then_some(notify) else {
            self.unbind_irq(irq);
            return Ok(Output::Done);
        };
        let held = self.held(thread, notify, ObjectKind::Notify, Rights::SEND)?;
        let (notify, badge) = (held.object, held.badge);
        if self.irqs[&irq].notify.is_some() {
            return Err(Error::Busy);
        }
        if self.remotes.contains_key(&notify) {
            return Err(Error::Invalid);
        }
        let bits = if badge == 0 { 1 } else { badge };
        live(&mut self.irqs, irq).notify = Some((notify, bits));
        live(&mut self.notifies, notify).irqs.push(irq);
        let entry = live(&mut self.irqs, irq);
        if std::mem::take(&mut entry.pending) {
            entry.state = IrqState::Fired;
            self.signal_bits(notify, bits);
        } else {
            entry.state = IrqState::Armed;
            self.unmask_line(irq);
            self.setipnum(irq);
        }
        Ok(Output::Done)
    }

    /// RISC-V, a level line on an APLIC in MSI mode: the APLIC does not
    /// signal again a source that stays asserted, so `irq_ack` and a bind
    /// that arm such an irq write its number to the domain's `setipnum`,
    /// which sets the pending bit only while the input is high, then to
    /// `setienum`, which enables it again: written in that order, a line
    /// that fell and rose while masked sends one MSI, not two (Interrupts,
    /// D-143, D-146).
    fn setipnum(&mut self, irq: ObjId) {
        let source = self.irqs[&irq].source;
        if self.machine.arch == crate::Arch::Riscv64 && source.is_line() && source.level() {
            self.aplic_write(source.id(), AplicReg::SetIp);
            self.aplic_write(source.id(), AplicReg::SetIe);
        }
    }

    /// G9 on RISC-V: the claiming node's gate writes `setipnum`, `setienum`
    /// or `clrienum` of an APLIC domain, whoever holds the domain's role,
    /// only with the number of a level source whose line word the node
    /// holds with its generation and that a core of the node claimed; it
    /// refuses any other (D-146). Says whether it wrote.
    pub(crate) fn aplic_write(&mut self, gsi: u32, register: AplicReg) -> bool {
        let word = self.table.line_word(gsi);
        let held = word.state() == dodos_abi::OwnState::Taken
            && word.owner() == dodos_abi::Owner::Node(self.node)
            && word.generation() == self.table.generation(self.node);
        let claimed = self
            .irqs
            .values()
            .any(|irq| irq.source.is_line() && irq.source.level() && irq.source.id() == gsi);
        if self.machine.arch != crate::Arch::Riscv64 || !held || !claimed {
            return false;
        }
        let line = self.table.aplic.entry(gsi).or_default();
        match register {
            AplicReg::SetIp => line.pending |= line.active && line.high,
            AplicReg::SetIe => line.enabled |= line.active,
            AplicReg::ClrIe => line.enabled = false,
        }
        line.forward();
        self.event.aplic.push(gsi);
        true
    }

    /// An irq armed: a line is unmasked at its controller, which takes
    /// back the mask a rebuild wrote from its `Report` (Interrupts, the
    /// states of an irq; D-139), by the holder of the controller's role
    /// only: elsewhere a claimer acknowledges by the EOI register or
    /// `ICC_DIR_EL1`, and no node masks a level line at its arrival
    /// (D-147).
    fn unmask_line(&mut self, irq: ObjId) {
        let source = self.irqs[&irq].source;
        if let Some(role) = source
            .is_line()
            .then(|| self.ioapic_role(source.id()))
            .flatten()
            && self.holds_role(role)
            && let Some(unit) = self.table.units.get_mut(&role)
        {
            unit.masked.remove(&source.id());
        }
    }

    /// Whether one node runs on the machine: one node slot, or every other
    /// a phantom node (D-148, D-149).
    pub(crate) fn one_node_runs(&self) -> bool {
        (0..self.table.nodes).all(|z| z == self.node || !self.real(z))
    }

    /// Whether this node holds role `role`.
    fn holds_role(&self, role: usize) -> bool {
        let word = self.table.roles[role];
        word.state() == dodos_abi::RoleState::Held && word.holder() == self.node
    }

    /// x86-64 without a directed end of interrupt: the one architecture
    /// where a level line is masked at its arrival, by the holder of its
    /// controller's role, the only node that claims one, and unmasked at
    /// its acknowledgement (D-147).
    pub(crate) fn masks_at_arrival(&self) -> bool {
        self.machine.arch == crate::Arch::X86_64 && !self.machine.directed_eoi
    }

    /// `irq_ack`: `busy` on an idle irq; nothing on an armed one; on a fired
    /// one, a pending arrival is signalled again, otherwise it is armed.
    pub(crate) fn irq_ack(&mut self, thread: ObjId, irq: SlotAddr) -> Outcome {
        let irq = self.object(thread, irq, ObjectKind::Irq, Rights::WRITE)?;
        // Nothing is delivered to a dying notify: `irq_ack` on an irq whose
        // notify is dying is `busy`, as on an idle irq.
        let dying = self.irqs[&irq]
            .notify
            .is_some_and(|(notify, _)| self.dying(notify));
        let entry = live(&mut self.irqs, irq);
        match entry.state {
            _ if dying => return Err(Error::Busy),
            IrqState::Idle => return Err(Error::Busy),
            IrqState::Armed => {}
            IrqState::Fired if entry.pending => {
                entry.pending = false;
                let (notify, bits) = entry.notify.expect("a fired irq is bound");
                self.signal_bits(notify, bits);
            }
            IrqState::Fired => {
                entry.state = IrqState::Armed;
                self.unmask_line(irq);
                self.setipnum(irq);
            }
        }
        Ok(Output::Done)
    }

    /// Where a line claimed on the core at `position` with `vector` points
    /// its redirection entry: its fixed remapping handle (core number times
    /// 192, plus vector less 48) and the vector with interrupt remapping,
    /// the vector and the core's APIC ID without (Interrupts, x86-64).
    pub(crate) fn line_target(&self, position: usize, vector: u8) -> crate::rings::Target {
        let core = u64::from(self.cores[position].number);
        let handle = remap_handle(core, vector);
        match self.machine.remapping {
            true => crate::rings::Target::Remapped { handle, vector },
            false => crate::rings::Target::Direct {
                vector,
                apic: self.machine.core_list()[core as usize].hw_id as u32,
            },
        }
    }

    /// An arrival: the vector of a claimed entry arrives on its core. An
    /// armed irq becomes fired and signals its notify, as `signal` with the
    /// recorded bits; otherwise an edge source sets pending. An arrival on
    /// an irq whose notify is dying is handled as on an idle irq, and the
    /// state of the irq does not change. A vector claimed by no entry is
    /// ignored. The masking of a level source and the end of interrupt are
    /// the hardware's; an arrival is an event of the kernel.
    pub fn interrupt(&mut self, vector: u8) {
        self.interrupt_on(0, vector);
    }

    /// An arrival on the core at `position` of the node: one event.
    pub fn interrupt_on(&mut self, position: usize, vector: u8) {
        *self.last_event = None;
        let entry = self.begin_on(position);
        let found = self
            .irqs
            .iter()
            .find(|(_, irq)| irq.core == position && irq.vector == vector)
            .filter(|(_, irq)| !self.line_masked(irq.source));
        if let Some((&id, irq)) = found {
            // RISC-V: a level source is masked at its APLIC at its arrival,
            // by the claiming node's gate (Interrupts, rule 1, D-146).
            let source = irq.source;
            if source.is_line() && source.level() {
                self.mask_level(source);
                self.event.arrival_enabled |= self.machine.arch == crate::Arch::Riscv64
                    && self
                        .table
                        .aplic
                        .get(&source.id())
                        .is_some_and(|l| l.enabled);
            }
            let dying = self.irqs[&id]
                .notify
                .is_some_and(|(notify, _)| self.dying(notify));
            let irq = live(&mut self.irqs, id);
            match (irq.state, irq.notify) {
                _ if dying && irq.source.level() => {}
                _ if dying => irq.pending = true,
                (IrqState::Armed, Some((notify, bits))) => {
                    irq.state = IrqState::Fired;
                    self.signal_bits(notify, bits);
                }
                _ if !irq.source.level() => irq.pending = true,
                _ => {}
            }
        }
        self.end(entry);
        self.auto_deliver();
    }

    /// Whether a line's redirection entry is masked at its IOAPIC as a
    /// rebuild wrote it from the line's `Report` (D-139): a masked entry
    /// raises nothing, so no arrival reaches the core.
    pub(crate) fn line_masked(&self, source: dodos_abi::IrqSource) -> bool {
        source.is_line()
            && self.ioapic_role(source.id()).is_some_and(|role| {
                self.table
                    .units
                    .get(&role)
                    .is_some_and(|u| u.masked.contains(&source.id()))
            })
    }

    fn signal_bits(&mut self, notify: ObjId, bits: u64) {
        live(&mut self.notifies, notify).word |= bits;
        self.hand_over(notify);
    }

    /// A level source masked where it is masked at its arrival: on RISC-V
    /// at its APLIC, by the claiming node's gate (Interrupts, rule 1,
    /// D-146); on x86-64 without a directed end of interrupt at its
    /// IOAPIC, by the holder of the controller's role (D-147).
    fn mask_level(&mut self, source: dodos_abi::IrqSource) {
        self.aplic_write(source.id(), AplicReg::ClrIe);
        if self.masks_at_arrival()
            && let Some(role) = self.ioapic_role(source.id())
            && self.holds_role(role)
        {
            let unit = self.table.units.entry(role).or_default();
            unit.masked.insert(source.id());
        }
    }

    /// Unbinding: the irq is idle, bound to no notify; pending is kept. A
    /// level source is masked where it is masked at its arrival, at once
    /// (Interrupts, the states of an irq: "a level source is masked at the
    /// controller while the irq is idle or fired").
    pub(crate) fn unbind_irq(&mut self, irq: ObjId) {
        let entry = live(&mut self.irqs, irq);
        entry.state = IrqState::Idle;
        let source = entry.source;
        if let Some((notify, _)) = entry.notify.take() {
            live(&mut self.notifies, notify)
                .irqs
                .retain(|&other| other != irq);
        }
        if source.is_line() && source.level() {
            self.mask_level(source);
        }
    }

    /// Destroying a notify: after its timers, each irq bound to it is
    /// unbound, most recently bound first. Says whether there was one.
    pub(crate) fn unbind_one_irq(&mut self, notify: ObjId) -> bool {
        let Some(&irq) = self.notifies[&notify].irqs.last() else {
            return false;
        };
        self.unbind_irq(irq);
        true
    }

    /// The removal of an irq on a line whose controller another node drives
    /// is a request, as an unmapping is (Interrupts, the irq, D-137): the
    /// step unbinds it, asks the holder to mask the line and returns
    /// `again`; a claim still asking on that line ends first; the line word
    /// is freed only once the request is answered. Says what the step does.
    pub(crate) fn irq_removal(&mut self, irq: ObjId) -> crate::caps::Undone {
        use crate::caps::Undone;
        let entry = self.irqs[&irq];
        if !entry.source.is_line() {
            return self.msi_removal(irq);
        }
        let gsi = entry.source.id();
        if self.line_asking(gsi) {
            return Undone::Wait;
        }
        // The redirection entry masked and answered: then, where a remapping
        // entry is cleared, the invalidation of the interrupt entry cache
        // of each unit that serves the IOAPIC's segment (Interrupts, the
        // irq, D-137; pass 20 had left it out of the model).
        if entry.removing {
            return self.line_remap_removal(irq);
        }
        let Some(role) = self.ioapic_role(gsi) else {
            return Undone::Nothing;
        };
        if self.unit_requests_full() {
            return Undone::Wait;
        }
        self.unbind_irq(irq);
        live(&mut self.irqs, irq).removing = true;
        let op = crate::rings::UnitOp::Redirection {
            gsi,
            claimed: false,
            release: true,
            level: entry.source.level(),
            low: entry.source.active_low(),
            target: crate::rings::Target::Masked,
        };
        if self
            .unit_request(role, op, crate::units::Waiter::Thread(None), None)
            .is_err()
        {
            return Undone::Wait;
        }
        // Served at once when this node holds the role itself.
        match self.line_asking(gsi) {
            true => Undone::Wait,
            false => Undone::Unit,
        }
    }

    /// The remapping handle of an irq: its core's number × 192 + (vector −
    /// 48) (Interrupts, interrupt remapping).
    pub(crate) fn handle_of(&self, irq: &Irq) -> u16 {
        remap_handle(u64::from(self.cores[irq.core].number), irq.vector)
    }

    /// The removal of an MSI's irq whose remapping entry the units of its
    /// segment cache is a request too (Interrupts, the irq, D-137; pass 19
    /// had left it out of the model): the step unbinds the irq, its gate
    /// clears the remapping entry, and the invalidation of the interrupt
    /// entry cache is asked of each unit that serves the source's segment,
    /// one after another as for a claim; the next step removes the
    /// capability once the last is answered. A unit this node holds is
    /// asked by the same entry, with no message. A handle whose
    /// invalidation a unit answered *failed* is retired: not reused while
    /// the unit is failed (D-139).
    fn msi_removal(&mut self, irq: ObjId) -> crate::caps::Undone {
        use crate::caps::Undone;
        let entry = self.irqs[&irq];
        let units = match self.machine.remapping && !entry.source.is_core_event() {
            true => self.segment_roles(entry.source.segment()),
            false => Vec::new(),
        };
        let handle = self.handle_of(&entry);
        if self.handle_asking(handle) {
            return Undone::Wait;
        }
        let Some(&first) = units.first() else {
            return Undone::Nothing;
        };
        if entry.removing {
            return Undone::Nothing;
        }
        // A request also when this node holds every role concerned: the
        // same entry of the unit request table, served with no message
        // (Interrupts, the irq; D-150: the shortcut of a node that held
        // every unit skipped the claim that must end first and the
        // retirement at a failed unit).
        if self.unit_requests_full() {
            return Undone::Wait;
        }
        self.unbind_irq(irq);
        live(&mut self.irqs, irq).removing = true;
        for &unit in &units {
            self.table
                .units
                .entry(unit)
                .or_default()
                .remap
                .remove(&u32::from(handle));
        }
        let op = crate::rings::UnitOp::InterruptCache { handle };
        let chain = crate::units::Chain {
            segment: Some(entry.source.segment()),
            then: None,
        };
        let waiter = crate::units::Waiter::Thread(None);
        if self
            .unit_request_with(first, op, waiter, None, None, chain)
            .is_err()
        {
            return Undone::Wait;
        }
        match self.handle_asking(handle) {
            true => Undone::Wait,
            false => Undone::Unit,
        }
    }

    /// The second half of the removal of a line's irq, once its redirection
    /// entry is masked: its gate clears the remapping entry of its handle in
    /// each unit of the IOAPIC's segment, and the invalidation of the
    /// interrupt entry cache is asked of each, one after another as for a
    /// claim; a unit held by this node is asked with no message. Nothing when no
    /// unit holds the entry any more and no invalidation is in flight. A
    /// handle whose invalidation a unit answered *failed* is retired.
    fn line_remap_removal(&mut self, irq: ObjId) -> crate::caps::Undone {
        use crate::caps::Undone;
        let entry = self.irqs[&irq];
        let segment = self
            .machine
            .ioapics
            .iter()
            .find(|io| (io.first_gsi..io.first_gsi + io.inputs).contains(&entry.source.id()))
            .map_or(0, |io| io.segment);
        let units = match self.machine.remapping {
            true => self.segment_roles(segment),
            false => Vec::new(),
        };
        let handle = self.handle_of(&entry);
        if self.handle_asking(handle) {
            return Undone::Wait;
        }
        let remapped = |m: &Self, role: usize| {
            m.table
                .units
                .get(&role)
                .is_some_and(|u| u.remap.contains_key(&u32::from(handle)))
        };
        let Some(&first) = units.iter().find(|&&role| remapped(self, role)) else {
            return Undone::Nothing;
        };
        // A request also when this node holds every role concerned, as
        // for an MSI (D-150).
        if self.unit_requests_full() {
            return Undone::Wait;
        }
        for &unit in &units {
            if let Some(state) = self.table.units.get_mut(&unit) {
                state.remap.remove(&u32::from(handle));
            }
        }
        let op = crate::rings::UnitOp::InterruptCache { handle };
        let chain = crate::units::Chain {
            segment: Some(segment),
            then: None,
        };
        let waiter = crate::units::Waiter::Thread(None);
        if self
            .unit_request_with(first, op, waiter, None, None, chain)
            .is_err()
        {
            return Undone::Wait;
        }
        match self.handle_asking(handle) {
            true => Undone::Wait,
            false => Undone::Unit,
        }
    }

    /// Whether an entry of the unit request table still asks the
    /// invalidation of the interrupt entry cache for a handle.
    fn handle_asking(&self, handle: u16) -> bool {
        self.unit_requests.iter().flatten().any(
            |e| matches!(e.op, crate::rings::UnitOp::InterruptCache { handle: h } if h == handle),
        )
    }

    /// Whether an entry of the unit request table still asks something of
    /// a line: its redirection entry, or a claim's units before it.
    pub(crate) fn line_asking(&self, gsi: u32) -> bool {
        let names = |op: &crate::rings::UnitOp| matches!(*op, crate::rings::UnitOp::Redirection { gsi: g, .. } if g == gsi);
        self.unit_requests
            .iter()
            .flatten()
            .any(|e| names(&e.op) || e.then.is_some_and(|(_, op)| names(&op)))
    }

    /// Destroying an irq: it is unbound, its source masked, its pending bit
    /// dropped, and its vector can be claimed again. The controller holder's
    /// gate already freed a line's word when it served the masking operation
    /// 5 with *release* (D-150).
    pub(crate) fn irq_destroyed(&mut self, irq: ObjId) {
        self.unbind_irq(irq);
        if let Some(entry) = self.irqs.remove(&irq) {
            let handle = self.handle_of(&entry);
            self.table.irq_tables.remove(&handle);
        }
    }
}

/// The fixed remapping handle of `vector` on the core of number `core`:
/// core number × 192 + (vector − 48), the one formula (Interrupts,
/// interrupt remapping; O11). A vector below the claimable range counts as
/// 48.
pub(crate) fn remap_handle(core: u64, vector: u8) -> u16 {
    (core * IRQ_TABLE_ENTRIES as u64 + u64::from(vector.saturating_sub(IRQ_VECTOR_FIRST))) as u16
}

/// Every handle of one core: one per claimable vector (Interrupts).
pub(crate) fn handles_of_core(core: u64) -> impl Iterator<Item = u16> {
    (0..IRQ_TABLE_ENTRIES as u64).map(move |entry| (core * IRQ_TABLE_ENTRIES as u64 + entry) as u16)
}
