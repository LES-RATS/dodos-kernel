//! Interrupts: the entries of the interrupt table of each physical core of
//! the node (D-129), their claim, binding and acknowledgement, and the
//! arrival of an interrupt on its core. An entry is guarded by its core's
//! lock word under variant L2 (D-131).

use crate::caps::Placing;
use crate::{Irq, IrqState, Model, ObjId, Outcome, Output, live, locked};
use dodos_abi::x86_64::{
    IRQ_MAX_CORES_REMAPPED, IRQ_TABLE_ENTRIES, IRQ_VECTOR_FIRST, IRQ_VECTOR_LAST,
};
use dodos_abi::{Error, IrqSource, ObjectKind, Rights, SlotAddr};

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
        // source), then the vector.
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
        if !self.machine.remapping && apic > 255 || self.machine.remapping && remapped_core {
            return Err(Error::Unsupported);
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
        self.fits(place.space, cap, Placing::Retyped(ObjectKind::Irq))?;
        // The state: the vector claimed, a line whose input is claimed, then
        // the destination.
        let claimed =
            |vector_or_line: &dyn Fn(&Irq) -> bool| self.irqs.values().any(vector_or_line);
        if claimed(&|irq| irq.core == position && irq.vector == vector) {
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
        let units = match self.machine.remapping {
            true => self.segment_roles(segment),
            false => Vec::new(),
        };
        let ioapic = source
            .is_line()
            .then(|| self.ioapic_role(source.id()))
            .flatten();
        let role = units.first().copied().or(ioapic);
        if role.is_some() && self.unit_requests.iter().all(Option::is_some) {
            return Err(Error::Again);
        }
        // The new `irq` is idle, with pending 0; its root capability is a
        // child of the `irq_control` capability, with every right, and it
        // takes no memory from an untyped.
        let root = self.create(ObjectKind::Irq, 0, Some(cap), Rights::ALL, place);
        let id = self.caps[&root].object;
        live(&mut self.objects, id).from = None;
        self.touch_core(position);
        let irq = Irq {
            core: position,
            vector,
            source,
            state: IrqState::Idle,
            pending: false,
            notify: None,
        };
        self.irqs.insert(id, irq);
        if source.is_line() {
            let taken = dodos_abi::Ownership::taken(dodos_abi::Owner::Node(self.node), generation);
            self.table.write_line(self.node, source.id(), Some(taken));
        }
        // The fixed remapping entry of the vector on that core: handle
        // core number × 192 + (vector − 48), written by this node's gate.
        let entry = u64::from(vector - IRQ_VECTOR_FIRST);
        let handle = core * IRQ_TABLE_ENTRIES as u64 + entry;
        let (address, data) = match (source.is_line(), self.machine.remapping) {
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
        let target = match self.machine.remapping {
            true => crate::rings::Target::Remapped {
                handle: handle as u16,
                vector,
            },
            false => crate::rings::Target::Direct {
                vector,
                apic: apic as u32,
            },
        };
        let redirection = ioapic.map(|io| {
            let op = crate::rings::UnitOp::Redirection {
                gsi: source.id(),
                claimed: true,
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
        let claimed = locked(&mut self.threads, &self.locks, thread);
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
        self.touch_core(self.irqs[&irq].core);
        live(&mut self.irqs, irq).notify = Some((notify, bits));
        locked(&mut self.notifies, &self.locks, notify)
            .irqs
            .push(irq);
        let entry = live(&mut self.irqs, irq);
        if std::mem::take(&mut entry.pending) {
            entry.state = IrqState::Fired;
            self.signal_bits(notify, bits);
        } else {
            entry.state = IrqState::Armed;
        }
        Ok(Output::Done)
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
        self.touch_core(self.irqs[&irq].core);
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
            IrqState::Fired => entry.state = IrqState::Armed,
        }
        Ok(Output::Done)
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

    /// An arrival on the core at `position` of the node.
    pub fn interrupt_on(&mut self, position: usize, vector: u8) {
        *self.last_event = None;
        let entry = self.begin_on(position);
        self.touch_core(position);
        let found = self
            .irqs
            .iter()
            .find(|(_, irq)| irq.core == position && irq.vector == vector);
        if let Some((&id, _)) = found {
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

    fn signal_bits(&mut self, notify: ObjId, bits: u64) {
        locked(&mut self.notifies, &self.locks, notify).word |= bits;
        self.hand_over(notify);
    }

    /// Unbinding: the irq is idle, bound to no notify; pending is kept.
    pub(crate) fn unbind_irq(&mut self, irq: ObjId) {
        let entry = live(&mut self.irqs, irq);
        entry.state = IrqState::Idle;
        if let Some((notify, _)) = entry.notify.take() {
            locked(&mut self.notifies, &self.locks, notify)
                .irqs
                .retain(|&other| other != irq);
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

    /// Destroying an irq: it is unbound, its source masked, its pending bit
    /// dropped, and its vector can be claimed again.
    pub(crate) fn irq_destroyed(&mut self, irq: ObjId) {
        self.unbind_irq(irq);
        if let Some(entry) = self.irqs.remove(&irq) {
            self.touch_core(entry.core);
            // A line's word is written free, unclaimed (D-135).
            if entry.source.is_line() {
                self.table.write_line(self.node, entry.source.id(), None);
            }
        }
    }
}
