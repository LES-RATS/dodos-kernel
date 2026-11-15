//! Interrupts: the entries of the interrupt table of the node, their claim,
//! binding and acknowledgement, and the arrival of an interrupt.

use crate::caps::Placing;
use crate::{Irq, IrqState, Model, ObjId, Outcome, Output, live};
use dodos_abi::x86_64::{
    IRQ_MAX_NODES_REMAPPED, IRQ_TABLE_ENTRIES, IRQ_VECTOR_FIRST, IRQ_VECTOR_LAST,
};
use dodos_abi::{Error, IrqSource, ObjectKind, Rights, SlotAddr};

impl Model {
    /// `irq_claim`: one vector of the node, bound to one source. Returns the
    /// address and the data a device must be given for an MSI (0 and 0 for
    /// a line).
    pub(crate) fn irq_claim(
        &mut self,
        thread: ObjId,
        control: SlotAddr,
        (source, vector): (u64, u64),
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
        let vector = u8::try_from(vector)
            .ok()
            .filter(|vector| (IRQ_VECTOR_FIRST..=IRQ_VECTOR_LAST).contains(vector));
        let vector = vector.ok_or(Error::Invalid)?;
        // `unsupported` if this machine cannot deliver the source to the
        // core: without remapping, an APIC ID above 255; with remapping, a
        // node of index 341 or more.
        let apic = self.machine.core_list()[self.node as usize].hw_id;
        let remapped_node = u32::from(self.node) >= IRQ_MAX_NODES_REMAPPED;
        if !self.machine.remapping && apic > 255 || self.machine.remapping && remapped_node {
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
        if claimed(&|irq| irq.vector == vector) {
            return Err(Error::Busy);
        }
        if source.is_line()
            && claimed(&|irq| irq.source.is_line() && irq.source.id() == source.id())
        {
            return Err(Error::Busy);
        }
        self.vacant(place)?;
        // The new `irq` is idle, with pending 0; its root capability is a
        // child of the `irq_control` capability, with every right, and it
        // takes no memory from an untyped.
        let root = self.create(ObjectKind::Irq, 0, Some(cap), Rights::ALL, place);
        let id = self.caps[&root].object;
        live(&mut self.objects, id).from = None;
        let irq = Irq {
            vector,
            source,
            state: IrqState::Idle,
            pending: false,
            notify: None,
        };
        self.irqs.insert(id, irq);
        // The fixed remapping entry of the vector on this node.
        let entry = u64::from(vector - IRQ_VECTOR_FIRST);
        let handle = u64::from(self.node) * IRQ_TABLE_ENTRIES as u64 + entry;
        let (address, data) = match (source.is_line(), self.machine.remapping) {
            (true, _) => (0, 0),
            (false, true) => (dodos_abi::x86_64::msi_remapped(handle), 0),
            (false, false) => {
                dodos_abi::x86_64::msi_direct(apic as u32, vector).expect("checked above")
            }
        };
        assert!(entry < IRQ_TABLE_ENTRIES as u64);
        Ok(Output::Msi { address, data })
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
        let bits = if badge == 0 { 1 } else { badge };
        live(&mut self.irqs, irq).notify = Some((notify, bits));
        live(&mut self.notifies, notify).irqs.push(irq);
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
        *self.last_event = None;
        let entry = self.begin();
        let found = self.irqs.iter().find(|(_, irq)| irq.vector == vector);
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
    }

    /// `signal`, with the bits of a capability's badge.
    pub(crate) fn signal_bits_pub(&mut self, notify: ObjId, bits: u64) {
        self.signal_bits(notify, bits)
    }

    fn signal_bits(&mut self, notify: ObjId, bits: u64) {
        live(&mut self.notifies, notify).word |= bits;
        self.hand_over(notify);
    }

    /// Unbinding: the irq is idle, bound to no notify; pending is kept.
    pub(crate) fn unbind_irq(&mut self, irq: ObjId) {
        let entry = live(&mut self.irqs, irq);
        entry.state = IrqState::Idle;
        if let Some((notify, _)) = entry.notify.take() {
            live(&mut self.notifies, notify)
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
        self.irqs.remove(&irq);
    }
}
