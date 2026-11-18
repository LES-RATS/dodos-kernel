//! A machine of several nodes (D-128): one model per node slot, the one
//! machine table they share, and the physical cores as the hardware has
//! them. Every event of a node runs alone; what interleaves are the events
//! of the cores of one node and of different nodes: a call, an interrupt a
//! core takes, a timer, an action of the hardware (a stop, a start, a
//! park), and the park stub's halt. `System::step` chooses among the events
//! that can happen now with a seeded generator, so that a failure replays.
//!
//! The park stub's lost wake-up (16 November 2026): an NMI that reaches the
//! stub between its read of the start request and its halt starts nothing;
//! the starter sends the wake again every millisecond until the stub
//! confirms (`dodos_abi::START_RETRY_NS`).

use crate::boot::{Layout, Machine};
use crate::nodes::HwAction;
use crate::{Arch, Model, ObjId, Outcome, Refusal, Syscall};
use dodos_abi::{CoreState, CoreWord, Member, MemberState};

/// The state of a physical core, as the hardware has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hardware {
    /// It runs the kernel of the node its word names.
    Running,
    /// It executes nothing useful (a test's input); `nmi` says whether its
    /// kernel can still run the handler of the fence's signal.
    Hung { nmi: bool },
    /// In the park stub (x86-64), halted; `window` while it is between its
    /// read of the start request and its halt.
    Stub { window: bool },
    /// INIT received, waiting for a SIPI (x86-64).
    WaitSipi,
    /// Off: ARM64 `CPU_OFF`, RISC-V STOPPED.
    Off,
    /// ARM64: the fence's handler completed the event and its core calls
    /// `CPU_OFF`; firmware answers OFF once the call is done.
    Parking,
    /// Started for a node whose first core has not built its state yet.
    Entering,
    /// x86-64: in the kept image's entry stub, left by the park stub with a
    /// start request; `installed` once it has installed the node kernel's
    /// NMI handler, after which it reads its core word (Node slots and core
    /// words, the start request and the entry stub, D-134).
    EntryStub { installed: bool },
    /// ARM64: on its way to a state that loses its context, between the
    /// gate's `SDEI_PE_MASK` and read of the park request and the power-down
    /// of `CPU_SUSPEND`: SDEI masked, so a fence signal that lands here is
    /// lost (Trusted Firmware-A); `roused` if something woke it meanwhile
    /// (`node_idle`, D-143).
    Suspending { latency: u64, roused: bool },
    /// In an idle state that loses its context (ARM64 `CPU_SUSPEND`, RISC-V
    /// a non-retentive `sbi_hart_suspend`); `wake` is the instant its exit
    /// latency ends, once its timer, an interrupt or a signal woke it.
    Asleep { latency: u64, wake: Option<u64> },
}

/// One event that can happen now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A core of a node takes its pending interrupts.
    Ipi { node: u16, core: usize },
    /// A core of a node takes its timer interrupt, due now.
    Timer { node: u16, core: usize },
    /// The hardware carries out an action a node asked for.
    Action(usize),
    /// The park stub halts after a read that found no start request.
    StubHalt { core: usize },
    /// ARM64: `CPU_OFF` of a core whose handler completed the fence's
    /// event; from then `AFFINITY_INFO` answers OFF.
    CpuOff { core: usize },
    /// x86-64: the entry stub's next step: it installs the node kernel's
    /// NMI handler, then reads its core word and enters or parks again.
    EntryStep { core: usize },
    /// ARM64 `CPU_SUSPEND` or RISC-V `sbi_hart_suspend` returns without
    /// losing the context (refused, or a shallower state): the gate reads
    /// the core word before it returns to the node (D-146).
    ShallowReturn { core: usize },
    /// An idle core enters the state its idle table chooses that loses its
    /// context (`node_idle`, Idle): on ARM64 the gate masks the PE and
    /// reads the park request first.
    Sleep { core: usize },
    /// ARM64: the power-down of `CPU_SUSPEND` after the gate's read.
    PowerDown { core: usize },
    /// A core leaves its sleep, its exit latency over: the gate reads the
    /// park request and parks the core if it is raised (D-143).
    Wake { core: usize },
}

/// A deterministic generator (xorshift64*), seeded by the tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0.max(1);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// The machine: its node slots, the actions in flight, its physical cores.
#[derive(Clone, Debug)]
pub struct System {
    nodes: Vec<Option<Model>>,
    hardware: Vec<Hardware>,
    actions: Vec<(u16, HwAction)>,
    pub(crate) machine: Machine,
    layout: Layout,
    /// The machine stopped with a stop record (containment violated).
    pub stopped: bool,
    /// Every action the hardware carried out, in order, with its instant
    /// and the node that asked for it.
    pub sent: Vec<(u64, u16, HwAction)>,
    /// Whether a park request was ever written lower than it was.
    pub(crate) park_went_back: bool,
    /// The common clock.
    now: u64,
}

impl System {
    /// Every node of the initial layout started from its floor and the kept
    /// image, with the machine table of machine start-up; the first thread
    /// of each.
    pub fn boot(machine: &Machine) -> Result<(Self, Vec<ObjId>), Refusal> {
        let layout = machine.layout()?;
        let mut table = machine.table(&layout);
        let mut nodes: Vec<Option<Model>> = vec![None; machine.nodes()];
        let mut firsts = Vec::new();
        for (slot, cores) in layout.nodes.clone().iter().enumerate() {
            let (mut model, first) = Model::start(machine, &layout, table, slot as u16, cores)?;
            model.shared = true;
            model.manual = true;
            model.ready = true;
            table = model.table.clone();
            nodes[slot] = Some(model);
            firsts.push(first);
        }
        let hardware = (0..machine.cores.len())
            .map(|core| {
                match table
                    .core_word(core)
                    .is_some_and(|w| w.state() == CoreState::Owned)
                {
                    true => Hardware::Running,
                    // A spare core waits in the park stub on x86-64; on
                    // ARM64 and RISC-V it is off until `CPU_ON` or
                    // `sbi_hart_start` (H15).
                    false => match machine.arch {
                        Arch::X86_64 => Hardware::Stub { window: false },
                        Arch::Aarch64 | Arch::Riscv64 => Hardware::Off,
                    },
                }
            })
            .collect();
        let mut system = Self {
            nodes,
            hardware,
            actions: Vec::new(),
            machine: machine.clone(),
            layout,
            stopped: false,
            sent: Vec::new(),
            park_went_back: false,
            now: 0,
        };
        system.share(table);
        Ok((system, firsts))
    }

    /// The kernels that run, by node slot.
    pub(crate) fn models(&self) -> impl Iterator<Item = (u16, &Model)> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(slot, m)| Some((slot as u16, m.as_ref()?)))
    }

    /// The machine table, as every kernel reads it.
    pub(crate) fn canonical(&self) -> &crate::table::Table {
        self.table()
    }

    /// The model of a node, to read it.
    pub fn node(&self, node: u16) -> &Model {
        self.nodes[node as usize]
            .as_ref()
            .expect("a node with a kernel")
    }

    /// Whether a node slot has a kernel.
    pub fn has(&self, node: u16) -> bool {
        self.nodes[node as usize].is_some()
    }

    /// The canonical machine table: the one of any model.
    fn table(&self) -> &crate::table::Table {
        &self.nodes.iter().flatten().next().expect("a node").table
    }

    /// The member word of a node.
    pub fn member(&self, node: u16) -> Member {
        self.table().member(node)
    }

    /// The role word of role `index`.
    pub fn role(&self, index: usize) -> dodos_abi::RoleWord {
        self.table().roles[index]
    }

    /// The core word of a physical core.
    pub fn core_word(&self, core: usize) -> u64 {
        self.table().core_words[core]
    }

    /// The park request of a physical core.
    pub fn park_request(&self, core: usize) -> u64 {
        self.table().park_requests[core]
    }

    /// The state of a physical core.
    pub fn hardware(&self, core: usize) -> Hardware {
        self.hardware[core]
    }

    /// The common clock.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// The actions in flight.
    pub fn actions(&self) -> &[(u16, HwAction)] {
        &self.actions
    }

    fn model(&mut self, node: u16) -> &mut Model {
        self.nodes[node as usize]
            .as_mut()
            .expect("a node with a kernel")
    }

    /// Every model reads the table one of them wrote.
    fn share(&mut self, table: crate::table::Table) {
        if let Some(before) = self.nodes.iter().flatten().next() {
            let back = before
                .table
                .park_requests
                .iter()
                .zip(&table.park_requests)
                .any(|(old, new)| new < old);
            self.park_went_back |= back;
        }
        for model in self.nodes.iter_mut().flatten() {
            model.table = table.clone();
        }
    }

    /// After an event of a node: the table it wrote is the machine's, and
    /// the actions it asked for are in flight; a node whose generation is
    /// dead loses its kernel.
    fn after(&mut self, node: u16) {
        let model = self.model(node);
        let actions = model.take_hw();
        let table = model.table.clone();
        self.actions.extend(actions.into_iter().map(|a| (node, a)));
        self.share(table);
        for slot in 0..self.nodes.len() as u16 {
            let dead = self.nodes[slot as usize].as_ref().is_some_and(|m| {
                let member = m.table.member(slot);
                member.state() == MemberState::Dead || member.generation() != m.generation_built()
            });
            if dead {
                self.nodes[slot as usize] = None;
            }
        }
    }

    /// One call made by a thread of a node, on the core that runs it.
    pub fn apply(&mut self, node: u16, thread: ObjId, call: Syscall) -> Outcome {
        let outcome = self.model(node).apply(thread, call);
        self.after(node);
        outcome
    }

    /// The events that can happen now, in a fixed order.
    pub fn events(&self) -> Vec<Event> {
        let mut events = Vec::new();
        for (slot, model) in self.nodes.iter().enumerate() {
            let Some(model) = model else { continue };
            for (core, _) in model.pending_ipis() {
                let event = Event::Ipi {
                    node: slot as u16,
                    core,
                };
                if !events.contains(&event) && !model.is_silent(core) && self.awake(model, core) {
                    events.push(event);
                }
            }
            for core in 0..model.core_count() {
                if !model.is_silent(core) && model.due_now(core) && self.awake(model, core) {
                    events.push(Event::Timer {
                        node: slot as u16,
                        core,
                    });
                }
                let number = model.core_number(core);
                if self.hardware[number] == Hardware::Running
                    && model.sleep_state(core).is_some()
                    && matches!(self.machine.arch, Arch::Aarch64 | Arch::Riscv64)
                {
                    events.push(Event::Sleep { core: number });
                }
            }
        }
        // A park waiting for its roused core to wake is not a step.
        let waits = |action: &HwAction| matches!(*action, HwAction::Park { core } if self.roused(core) == Some(true));
        events.extend(
            (0..self.actions.len())
                .filter(|&i| !waits(&self.actions[i].1))
                .map(Event::Action),
        );
        for (core, &hw) in self.hardware.iter().enumerate() {
            if hw == (Hardware::Stub { window: true }) {
                events.push(Event::StubHalt { core });
            }
            if hw == Hardware::Parking {
                events.push(Event::CpuOff { core });
            }
            if matches!(hw, Hardware::EntryStub { .. }) {
                events.push(Event::EntryStep { core });
            }
            if matches!(hw, Hardware::Suspending { .. }) {
                events.push(Event::PowerDown { core });
                events.push(Event::ShallowReturn { core });
            }
            if matches!(hw, Hardware::Asleep { wake: Some(at), .. } if at <= self.now) {
                events.push(Event::Wake { core });
            }
        }
        events
    }

    /// Whether the core at `position` of `model` takes its interrupts: not
    /// on its way to sleep or asleep, where an interrupt only wakes it.
    fn awake(&self, model: &Model, position: usize) -> bool {
        !matches!(
            self.hardware[model.core_number(position)],
            Hardware::Suspending { .. } | Hardware::Asleep { .. }
        )
    }

    /// The node model and position of a physical core its word names.
    fn position_of(&self, core: usize) -> Option<(u16, usize)> {
        let word = CoreWord::from_raw(self.table().core_words[core])?;
        let model = self.nodes.get(word.slot() as usize)?.as_ref()?;
        Some((word.slot(), model.position(core as u64)?))
    }

    /// Whether the gate's read before a suspend that loses the context, or
    /// at a way out of one, finds the park request raised: the core's word
    /// carries the fence mark (D-146). Not "the request differs from
    /// `parked`": a core already off at a fence, or stopped by
    /// `sbi_hart_stop`, never wrote `parked`, and reused by a healthy node
    /// would park itself at its first suspend.
    fn park_raised(&self, core: usize) -> bool {
        self.marked(core)
    }

    /// An idle core enters its state that loses its context. ARM64: the
    /// gate, interrupts masked, calls `SDEI_PE_MASK`, then reads the park
    /// request: raised, it writes it into `parked` and runs the park routine
    /// with no unmask; otherwise `CPU_SUSPEND`. RISC-V: `sbi_hart_suspend`,
    /// the park request read at the return (`node_idle`, D-143). Both pass
    /// a window before the power-down where the fence's signal is lost.
    fn sleep(&mut self, core: usize) {
        let Some((node, position)) = self.position_of(core) else {
            return;
        };
        let Some(latency) = self.model(node).sleep_state(position) else {
            return;
        };
        match self.machine.arch {
            Arch::Aarch64 if self.park_raised(core) => {
                self.write_parked(core, false);
                self.hardware[core] = Hardware::Parking;
            }
            _ => {
                self.hardware[core] = Hardware::Suspending {
                    latency,
                    roused: false,
                };
            }
        }
        self.rouse();
    }

    /// The power-down of `CPU_SUSPEND` or `sbi_hart_suspend`; a core roused
    /// on its way wakes at once, after its exit latency.
    fn power_down(&mut self, core: usize) {
        if let Hardware::Suspending { latency, roused } = self.hardware[core] {
            let wake = roused.then_some(self.now + latency);
            self.hardware[core] = Hardware::Asleep { latency, wake };
        }
        self.rouse();
    }

    /// A core leaves its sleep. ARM64: `SDEI_PE_UNMASK` before anything
    /// else, then the park request read: raised, the gate writes it into
    /// `parked` and runs the park routine instead of returning to the node.
    /// RISC-V: the same read at the return from `sbi_hart_suspend`, and
    /// `sbi_hart_stop` when it is raised, which writes nothing. So a fence
    /// signal lost at a wake still parks the core (`node_idle`, D-143,
    /// D-146).
    fn wake_from_sleep(&mut self, core: usize) {
        if !matches!(self.hardware[core], Hardware::Asleep { .. }) {
            return;
        }
        match (self.machine.arch, self.park_raised(core)) {
            (Arch::Aarch64, true) => {
                self.write_parked(core, false);
                self.hardware[core] = Hardware::Parking;
            }
            (_, true) => self.off_unanswered(core),
            (_, false) => self.hardware[core] = Hardware::Running,
        }
    }

    /// What wakes a sleeping core: its timer due, an interrupt pending for
    /// it; then its exit latency runs.
    fn rouse(&mut self) {
        self.sync_sleep();
        for core in 0..self.hardware.len() {
            let Hardware::Asleep {
                latency,
                wake: None,
            } = self.hardware[core]
            else {
                continue;
            };
            let Some((node, position)) = self.position_of(core) else {
                continue;
            };
            let model = self.nodes[node as usize].as_ref().expect("a model");
            let pending = model.pending_ipis().iter().any(|&(c, _)| c == position);
            if pending || model.due_now(position) {
                self.hardware[core] = Hardware::Asleep {
                    latency,
                    wake: Some(self.now + latency),
                };
            }
        }
    }

    /// A fence signal, a park or a start's wake reaches a sleeping core: it
    /// wakes (the signal itself is lost at the wake, D-143); on its way to
    /// sleep (SDEI masked) it is roused, so that it wakes once down.
    /// Whether `core` is on its way to sleep or asleep: then whether it was
    /// roused already.
    fn roused(&self, core: usize) -> Option<bool> {
        match self.hardware[core] {
            Hardware::Suspending { roused, .. } => Some(roused),
            Hardware::Asleep { wake, .. } => Some(wake.is_some()),
            _ => None,
        }
    }

    fn rouse_one(&mut self, core: usize) {
        match self.hardware[core] {
            Hardware::Asleep {
                latency,
                wake: None,
            } => {
                self.hardware[core] = Hardware::Asleep {
                    latency,
                    wake: Some(self.now + latency),
                };
            }
            Hardware::Suspending { latency, .. } => {
                self.hardware[core] = Hardware::Suspending {
                    latency,
                    roused: true,
                };
            }
            _ => {}
        }
    }

    /// The next instant something is due: a timer of a core awake, a sleeping
    /// core's timer (which wakes it) or the end of its exit latency.
    fn next_instant(&self) -> Option<u64> {
        let timers = self.nodes.iter().flatten().flat_map(|m| {
            (0..m.core_count())
                .filter(|&p| !m.is_silent(p))
                .filter_map(move |p| m.timer_of(p))
        });
        let wakes = self.hardware.iter().filter_map(|h| match *h {
            Hardware::Asleep { wake, .. } => wake,
            _ => None,
        });
        timers.chain(wakes).min()
    }

    /// Carries out one event.
    pub fn fire(&mut self, event: Event) {
        if self.stopped {
            return;
        }
        match event {
            Event::Ipi { node, core } => {
                self.model(node).take_interrupts(core);
                self.after(node);
            }
            Event::Timer { node, core } => {
                self.model(node).timer_interrupt(core);
                self.after(node);
            }
            Event::Action(index) => {
                let (node, action) = self.actions.remove(index);
                self.act(node, action);
            }
            Event::StubHalt { core } => self.hardware[core] = Hardware::Stub { window: false },
            Event::CpuOff { core } => {
                let mut table = self.table().clone();
                table.stopped[core] = table.park_requests[core];
                self.share(table);
                self.hardware[core] = Hardware::Off;
            }
            Event::EntryStep { core } => self.entry_step(core),
            Event::Sleep { core } => self.sleep(core),
            Event::PowerDown { core } => self.power_down(core),
            Event::Wake { core } | Event::ShallowReturn { core } => {
                if let (Event::ShallowReturn { .. }, Hardware::Suspending { latency, .. }) =
                    (event, self.hardware[core])
                {
                    self.hardware[core] = Hardware::Asleep {
                        latency,
                        wake: Some(self.now),
                    };
                }
                self.wake_from_sleep(core);
                // The timer interrupt that woke the core is taken at once,
                // late by the exit latency (Idle: "An interrupt wakes it").
                if self.hardware[core] == Hardware::Running
                    && let Some((node, position)) = self.position_of(core)
                    && self.node(node).due_now(position)
                {
                    self.model(node).timer_interrupt(position);
                    self.after(node);
                }
            }
        }
        self.rouse();
    }

    /// Each node learns which of its cores sleep, and the exit latency of
    /// the state: a timer of such a core is taken when it wakes, up to that
    /// latency after its instant.
    fn sync_sleep(&mut self) {
        let hardware = self.hardware.clone();
        for model in self.nodes.iter_mut().flatten() {
            for position in 0..model.core_count() {
                let latency = match hardware[model.core_number(position)] {
                    Hardware::Suspending { latency, .. } | Hardware::Asleep { latency, .. } => {
                        latency
                    }
                    _ => 0,
                };
                model.cores[position].asleep = latency;
            }
        }
    }

    /// One event chosen by the generator among those that can happen now;
    /// returns whether there was one.
    pub fn step(&mut self, rng: &mut Rng) -> bool {
        let events = self.events();
        if events.is_empty() || self.stopped {
            return false;
        }
        let event = events[rng.below(events.len())];
        self.fire(event);
        true
    }

    /// Every event that can happen now, in their fixed order, until none
    /// can (at most `bound`); returns how many.
    pub fn settle(&mut self, bound: usize) -> usize {
        let mut fired = 0;
        while fired < bound {
            let Some(&event) = self.events().first() else {
                break;
            };
            self.fire(event);
            fired += 1;
        }
        fired
    }

    /// Time passes up to `to`: at each instant a core's timer is due, its
    /// interrupt, then what can happen at that instant.
    pub fn advance(&mut self, to: u64) {
        loop {
            self.settle(10_000);
            let next = self.next_instant().filter(|&at| at <= to);
            let Some(at) = next else { break };
            self.set_clock(at.max(self.now));
            if self.events().is_empty() {
                // A timer due at an instant already reached is taken, on a
                // core awake (a sleeping core's timer wakes it).
                let due = self.nodes.iter().enumerate().find_map(|(slot, m)| {
                    let m = m.as_ref()?;
                    (0..m.core_count())
                        .filter(|&p| !m.is_silent(p) && self.awake(m, p))
                        .find(|&p| m.timer_of(p).is_some_and(|at| at <= self.now))
                        .map(|p| (slot as u16, p))
                });
                let Some((node, core)) = due else { break };
                self.fire(Event::Timer { node, core });
            }
            if self.stopped {
                return;
            }
        }
        self.set_clock(to.max(self.now));
        self.settle(10_000);
    }

    /// Time passes up to `to` in steps of `step`.
    pub fn run(&mut self, to: u64, step: u64) {
        while self.now < to && !self.stopped {
            self.advance((self.now + step.max(1)).min(to));
        }
    }

    /// Time passes to `to` and nothing is taken: the events due become
    /// events that can happen now (a test's input, to choose among them).
    pub fn move_clock(&mut self, to: u64) {
        self.set_clock(to.max(self.now));
    }

    fn set_clock(&mut self, now: u64) {
        self.now = now;
        for model in self.nodes.iter_mut().flatten() {
            model.set_now(now);
        }
        self.rouse();
    }

    /// A physical core stops answering (a test's input): it runs nothing,
    /// and `nmi` says whether its kernel can still run the fence's handler.
    pub fn hang(&mut self, core: usize, nmi: bool) {
        self.hardware[core] = Hardware::Hung { nmi };
        if let Some(word) = CoreWord::from_raw(self.table().core_words[core])
            && let Some(Some(model)) = self.nodes.get_mut(word.slot() as usize)
            && let Some(position) = model.position(core as u64)
        {
            model.silence(position);
        }
    }

    /// A start's wake reaches a core late (a test's input). A core in the
    /// park stub, or off, takes it as a start. On x86-64 the wake is an NMI,
    /// the fence's own signal, which a core that runs its node's kernel
    /// takes as a fence only if its word carries the fence mark (D-134); on
    /// ARM64 it is `CPU_ON` and on RISC-V `sbi_hart_start`, which a core
    /// already on answers with `ALREADY_ON` or `SBI_ERR_ALREADY_AVAILABLE`
    /// and which starts nothing (Node slots and core words, the start
    /// request and the entry stub).
    pub fn late_wake(&mut self, core: usize) {
        match (self.machine.arch, self.hardware[core]) {
            (_, Hardware::Stub { .. } | Hardware::Off) => self.wake(core),
            (Arch::X86_64, _) => self.stop(core),
            (Arch::Aarch64 | Arch::Riscv64, _) => {}
        }
    }

    /// The debugger stub of a development image stops the machine for `ns`
    /// nanoseconds: it raises the release word, time passes (no node
    /// records an observation or accuses another meanwhile), it adds the
    /// stop to the stopped time, then lowers the release word (Two builds,
    /// and the debugger stub, D-137).
    #[cfg(test)]
    pub fn stub_stop(&mut self, ns: u64) {
        self.edit_table(|t| t.release = true);
        let to = self.now + ns;
        self.advance(to);
        self.edit_table(|t| {
            t.stopped_time += ns;
            t.release = false;
        });
    }

    /// A parked core is in the park stub between its read of the start
    /// request and its halt (a test's input): a wake that reaches it now
    /// is lost.
    pub fn set_window(&mut self, core: usize) {
        assert!(matches!(self.hardware[core], Hardware::Stub { .. }));
        self.hardware[core] = Hardware::Stub { window: true };
    }

    /// A core enters or leaves VMX root operation (a test's input), as its
    /// core beacon shows it.
    pub fn set_vmx(&mut self, core: usize, vmx: bool) {
        let mut table = self.table().clone();
        table.core_beacons[core].vmx = vmx;
        self.share(table);
    }

    /// A node finds a failure in itself, on its core at `position`.
    pub fn fail(&mut self, node: u16, position: usize) {
        let model = self.model(node);
        model.fail_on(position);
        self.after(node);
    }

    /// A faulty or dying sender leaves raw bytes at the head of its ring
    /// to `to` (a test's input): a header the receiver checks like any
    /// other, taking `entries` entries of the producer index.
    pub fn inject_raw(
        &mut self,
        from: u16,
        to: u16,
        user: bool,
        header: dodos_abi::RingHeader,
        entries: u64,
    ) {
        let mut table = self.table().clone();
        let pair = &mut std::sync::Arc::make_mut(&mut table.rings)[from as usize][to as usize];
        let ring = if user {
            &mut pair.user
        } else {
            &mut pair.kernel
        };
        let at = ring.produced;
        ring.set(at, crate::table::RingEntry::Raw(header));
        ring.produced += entries;
        self.share(table);
        self.interrupt(to, crate::Ipi::Ring);
    }

    /// A sender writes a whole message at the head of its ring to `to` (a
    /// test's input: a faulty, late or forged message that the receiver
    /// must judge like any other).
    pub fn inject(&mut self, from: u16, to: u16, message: crate::rings::Message) {
        let user = message.body.kind().ring() == dodos_abi::Ring::User;
        let entries = message.body.entries();
        let mut table = self.table().clone();
        let pair = &mut std::sync::Arc::make_mut(&mut table.rings)[from as usize][to as usize];
        let ring = if user {
            &mut pair.user
        } else {
            &mut pair.kernel
        };
        let at = ring.produced;
        ring.set(at, crate::table::RingEntry::Message(message));
        ring.produced += entries;
        self.share(table);
        self.interrupt(to, crate::Ipi::Ring);
    }

    /// The unit of a role stops confirming (a test's input).
    pub fn break_unit(&mut self, role: usize) {
        for model in self.nodes.iter_mut().flatten() {
            model.table.units.entry(role).or_default().broken = true;
        }
    }

    /// The hardware carries out an action.
    fn act(&mut self, from: u16, action: HwAction) {
        self.sent.push((self.now, from, action));
        match action {
            HwAction::Stop { core } => self.stop(core),
            HwAction::Start { core } => self.wake(core),
            // A sleeping core of the node is woken by the park's interrupt,
            // and its kernel parks it once awake: the park stays in flight
            // (D-146: the read at the wake finds no mark).
            HwAction::Park { core } if self.roused(core).is_some() => {
                self.rouse_one(core);
                self.actions.push((from, action));
            }
            HwAction::Park { core } => self.park(core),
            HwAction::Ring { node } => self.interrupt(node, crate::Ipi::Ring),
            HwAction::Interrupt { node } => self.interrupt(node, crate::Ipi::Reschedule),
            HwAction::StopMachine => self.stopped = true,
        }
    }

    /// An interrupt to the first core of a node that runs.
    fn interrupt(&mut self, node: u16, ipi: crate::Ipi) {
        if let Some(Some(model)) = self.nodes.get_mut(node as usize) {
            model.receive_interrupt(ipi);
        }
    }

    /// Writes the core beacon's `parked` with the park request the stub or
    /// the handler reads, and, on ARM64 and RISC-V once the core is off,
    /// the machine's answer to that request.
    fn write_parked(&mut self, core: usize, off: bool) {
        let mut table = self.table().clone();
        let request = table.park_requests[core];
        table.core_beacons[core].parked = request;
        table.core_beacons[core].started = 0;
        if off {
            table.stopped[core] = request;
        }
        self.share(table);
        self.silence(core);
    }

    /// The node the core's word names no longer hears from the core.
    fn silence(&mut self, core: usize) {
        if let Some(word) = CoreWord::from_raw(self.table().core_words[core])
            && let Some(Some(model)) = self.nodes.get_mut(word.slot() as usize)
            && let Some(position) = model.position(core as u64)
        {
            model.silence(position);
        }
    }

    /// The handler of the fence's signal, if the core's kernel can run it,
    /// writes what it can of its core's crash record (reason 5), unless the
    /// record holds a committed record of the same generation.
    fn fence_record(&mut self, core: usize) {
        // The node kernel's handler is armed from the entry stub on (it
        // installs it, or registers SDEI event 0, before it reads its word).
        if !matches!(
            self.hardware[core],
            Hardware::Running
                | Hardware::Hung { nmi: true }
                | Hardware::Entering
                | Hardware::EntryStub { installed: true }
        ) {
            return;
        }
        let mut table = self.table().clone();
        let Some(word) = CoreWord::from_raw(table.core_words[core]) else {
            return;
        };
        let member = table.member(word.slot());
        let record = crate::debug::Record {
            commit: 0,
            generation: word.generation(),
            reason: dodos_abi::CrashReason::Fenced as u32,
            instant: self.now,
            member: member.raw(),
        };
        std::sync::Arc::make_mut(&mut table.crash).write_record(core, record);
        self.share(table);
    }

    /// The fence's stop of a core, by the means of its architecture.
    fn stop(&mut self, core: usize) {
        let booted = self.table().booted[core];
        match (self.machine.arch, self.hardware[core]) {
            // x86-64: the NMI. The node kernel's handler parks its core only
            // if the core's word carries the fence mark, and otherwise
            // returns and changes nothing (D-134: a late start wake is not
            // a fence signal); the stop is sent only under a marked word,
            // so this guards a future path that would reuse it.
            (
                Arch::X86_64,
                Hardware::Running
                | Hardware::Hung { nmi: true }
                | Hardware::Entering
                | Hardware::EntryStub { installed: true },
            ) if !self.marked(core) => {}
            // A kernel that can run its handler writes the park request it
            // reads into `parked` and returns by IRET into the park stub;
            // the stub's own NMI handler does the same from its loop.
            (
                Arch::X86_64,
                Hardware::Running
                | Hardware::Hung { nmi: true }
                | Hardware::Stub { .. }
                | Hardware::Entering
                | Hardware::EntryStub { installed: true },
            ) => {
                // The handler leaves VMX root operation if it is in it.
                self.fence_record(core);
                self.set_vmx(core, false);
                self.write_parked(core, false);
                self.hardware[core] = Hardware::Stub { window: false };
            }
            // INIT (not to the bootstrap core, and blocked in VMX root
            // operation), then the SIPIs to the park stub, which writes
            // `parked`.
            // The entry stub before it installed the kernel's handler: the
            // stub's handler only returns (it writes `parked` only from its
            // own loop, D-134), so only INIT stops it; the entry stub then
            // reads its word, which the fence marked before its NMI.
            (
                Arch::X86_64,
                Hardware::Hung { nmi: false }
                | Hardware::WaitSipi
                | Hardware::EntryStub { installed: false },
            ) if !booted && !self.table().core_beacons[core].vmx => {
                self.write_parked(core, false);
                self.hardware[core] = Hardware::Stub { window: false };
            }
            (Arch::X86_64, _) => {}
            // ARM64: SDEI event 0 to a core whose kernel registered its
            // handler; the handler writes `parked` and completes the event
            // to the park routine, which calls `CPU_OFF` (a later event).
            // The fence is confirmed by OFF after the request, never by the
            // handler's own write.
            // A core that is entering its node registered the handler in the
            // entry stub, before it read its word.
            (
                Arch::Aarch64,
                Hardware::Running | Hardware::Hung { nmi: true } | Hardware::Entering,
            ) if !self.marked(core) => {}
            (
                Arch::Aarch64,
                Hardware::Running | Hardware::Hung { nmi: true } | Hardware::Entering,
            ) => {
                self.fence_record(core);
                self.write_parked(core, false);
                self.hardware[core] = Hardware::Parking;
            }
            // On its way to sleep, SDEI masked: the signal is lost (Trusted
            // Firmware-A loses an event that reaches a masked PE); asleep, it
            // wakes the core and is lost at the wake, firmware masking the PE
            // there, and the read at the wake parks it (D-143).
            (Arch::Aarch64, Hardware::Suspending { .. }) if self.marked(core) => {}
            (Arch::Aarch64, Hardware::Asleep { .. }) if self.marked(core) => self.rouse_one(core),
            // A core already off answers OFF to the new request at once (no
            // `CPU_ON` was sent since).
            (Arch::Aarch64, Hardware::Off) => {
                let mut table = self.table().clone();
                table.stopped[core] = table.park_requests[core];
                self.share(table);
            }
            (Arch::Aarch64, _) => {}
            // RISC-V: the stop extension stops the hart whatever it does; the
            // gate sends it only to a marked word (G10).
            (Arch::Riscv64, _) if !self.marked(core) => {}
            // The cooperative fence (a hart of fence class 0): an interrupt
            // whose handler, if it runs, calls `sbi_hart_stop`; lost on the
            // way into a non-retentive suspend, where nothing keeps it; it
            // wakes a hart asleep, and the read at the return stops it; a
            // hart already stopped reads STOPPED (D-146).
            (Arch::Riscv64, hw) if self.machine.core_list()[core].fence == 0 => match hw {
                Hardware::Running | Hardware::Hung { nmi: true } | Hardware::Entering => {
                    self.fence_record(core);
                    self.off_unanswered(core);
                }
                Hardware::Asleep { .. } => self.rouse_one(core),
                Hardware::Off => self.off_unanswered(core),
                _ => {}
            },
            // `fence_hart`: the stopped hart writes nothing, and the
            // extension answers the request.
            (Arch::Riscv64, _) => self.off_unanswered(core),
        }
    }

    /// A core of a node parks itself: x86-64 into the park stub, ARM64 and
    /// RISC-V off.
    fn park(&mut self, core: usize) {
        if matches!(self.hardware[core], Hardware::Hung { .. }) {
            return;
        }
        // ARM64 and RISC-V: a seal's own park (`CPU_OFF`, `sbi_hart_stop`)
        // writes no `parked`, confirmed by `AFFINITY_INFO` or STOPPED; on
        // x86-64 the park stub writes it (The fence, `node_idle`, D-146).
        if self.machine.arch == Arch::X86_64 {
            self.write_parked(core, false);
            self.hardware[core] = Hardware::Stub { window: false };
        } else {
            self.off_unanswered(core);
        }
    }

    /// A core off with no write of `parked`: `AFFINITY_INFO` or the SBI
    /// answers OFF, or STOPPED, to the park request it finds.
    fn off_unanswered(&mut self, core: usize) {
        let mut table = self.table().clone();
        table.stopped[core] = table.park_requests[core];
        self.share(table);
        self.unstarted(core);
        self.hardware[core] = Hardware::Off;
    }

    /// A core leaves its node: its `started` reads 0 again, and its node no
    /// longer hears from it.
    fn unstarted(&mut self, core: usize) {
        let mut table = self.table().clone();
        table.core_beacons[core].started = 0;
        self.share(table);
        self.silence(core);
    }

    /// The wake of a parked core for a start: x86-64 an NMI to the park
    /// stub (lost in the stub's window), ARM64 `CPU_ON`, RISC-V
    /// `sbi_hart_start`. The core enters the entry stub, which reads its
    /// core word and parks again unless the word is not marked and names a
    /// node that is starting or live with the word's generation.
    fn wake(&mut self, core: usize) {
        match (self.machine.arch, self.hardware[core]) {
            (Arch::X86_64, Hardware::Stub { window: true }) => return,
            (Arch::X86_64, Hardware::Stub { window: false }) => {}
            (Arch::Aarch64 | Arch::Riscv64, Hardware::Off) => {}
            _ => return,
        }
        let request = self.table().start_requests[core];
        let Some((node, generation)) = crate::table::Table::started_node(request) else {
            // The stub's handler interrupted its loop: it writes the park
            // request it reads into `parked`, finds no start request, and
            // the stub halts again.
            // ARM64 and RISC-V: the entry stub finds no request and the
            // core parks again, off.
            if self.machine.arch == Arch::X86_64 {
                self.write_parked(core, false);
                self.hardware[core] = Hardware::Stub { window: true };
            }
            return;
        };
        if self.machine.arch == Arch::X86_64 {
            // The park stub writes the request it takes into `started` and
            // leaves for the entry stub, which installs the kernel's NMI
            // handler before it reads its core word (D-134).
            let mut table = self.table().clone();
            table.core_beacons[core].started = request;
            self.share(table);
            self.hardware[core] = Hardware::EntryStub { installed: false };
            return;
        }
        self.enter_if_named(core, node, generation, request);
    }

    /// The x86-64 entry stub's next step: it installs the node kernel's NMI
    /// handler, then reads its core word and enters or parks again.
    fn entry_step(&mut self, core: usize) {
        match self.hardware[core] {
            Hardware::EntryStub { installed: false } => {
                self.hardware[core] = Hardware::EntryStub { installed: true };
            }
            Hardware::EntryStub { installed: true } => {
                let request = self.table().core_beacons[core].started;
                match crate::table::Table::started_node(request) {
                    Some((node, generation)) => {
                        self.enter_if_named(core, node, generation, request);
                    }
                    None => self.park(core),
                }
            }
            _ => {}
        }
    }

    /// Whether a core's word carries the fence mark.
    fn marked(&self, core: usize) -> bool {
        CoreWord::from_raw(self.table().core_words[core]).is_some_and(|w| w.is_marked())
    }

    /// The entry stub reads the core word and parks again unless the word
    /// is not marked and names a node that is starting or live with the
    /// word's generation; otherwise the core enters its node.
    fn enter_if_named(&mut self, core: usize, node: u16, generation: u32, request: u64) {
        let word = CoreWord::from_raw(self.table().core_words[core]);
        let member = self.table().member(node);
        let enters = word.is_some_and(|w| !w.is_marked() && w.names(node, generation))
            && member.generation() == generation
            && matches!(member.state(), MemberState::Starting | MemberState::Live);
        if !enters {
            match self.machine.arch {
                // The entry stub parks again: the park stub writes `parked`.
                Arch::X86_64 => self.park(core),
                _ => self.hardware[core] = Hardware::Off,
            }
            return;
        }
        // ARM64 and RISC-V: the entry stub writes the start request it
        // takes into `started`, as its first write (gap 8, D-134); on
        // x86-64 the park stub wrote it already.
        if self.table().core_beacons[core].started != request {
            let mut table = self.table().clone();
            table.core_beacons[core].started = request;
            self.share(table);
        }
        self.hardware[core] = Hardware::Entering;
        self.enter(core, node, generation);
    }

    /// A core enters its node: the core at position 0 of a fresh start
    /// builds the node's state; each core invalidates its whole TLB, clears
    /// its interrupt table and writes its word owned.
    fn enter(&mut self, core: usize, node: u16, generation: u32) {
        let joining = self.table().cores_of(node, generation);
        let fresh = self.nodes[node as usize]
            .as_ref()
            .is_none_or(|m| m.generation_built() != generation);
        if fresh {
            if joining.first() != Some(&core) {
                return;
            }
            let table = self.table().clone();
            // A start the node refuses (its start-up page does not fit)
            // leaves no core in a state no event leaves: the core parks
            // again, or goes off, and the starter's `START_NS` ends the
            // start (O22).
            let Ok((mut model, _)) =
                Model::start(&self.machine, &self.layout, table, node, &joining)
            else {
                match self.machine.arch {
                    Arch::X86_64 => self.park(core),
                    _ => self.hardware[core] = Hardware::Off,
                }
                return;
            };
            model.shared = true;
            model.manual = true;
            // The clock is the machine's (Clock): a kernel started at run
            // time reads it as the others do (pass 25: it read 0 until
            // the clock next moved, and stamped its first observations at
            // instant 0).
            model.set_now(self.now);
            model.begin_fresh();
            self.nodes[node as usize] = Some(model);
        } else if (0..self.model(node).core_count()).all(|p| self.model(node).is_silent(p)) {
            let readable = !self.table().resume_replacement[node as usize]
                || self.machine.replacement_reads_sealed;
            self.model(node).begin_resume(readable);
        }
        let entering: Vec<usize> = joining
            .iter()
            .copied()
            .filter(|&c| c == core || self.hardware[c] == Hardware::Entering)
            .collect();
        for c in entering {
            self.hardware[c] = Hardware::Running;
            let model = self.model(node);
            model.core_entered(c);
        }
        self.after(node);
    }
}

/// Test inputs that write a state no event makes: what a faulty kernel, a
/// corrupted word or a state the explorations do not reach would leave, so
/// that a test can judge a rule or an invariant on it.
#[cfg(test)]
impl System {
    /// The kernel of a node, to change what it keeps.
    pub(crate) fn node_mut(&mut self, node: u16) -> &mut Model {
        self.model(node)
    }

    /// An action in flight, as if a node had asked for it.
    pub(crate) fn push_action(&mut self, node: u16, action: HwAction) {
        self.actions.push((node, action));
    }

    /// RISC-V: the device drives the input of level line `gsi` (a test's
    /// input): a rising input sets the source pending, a low one clears it,
    /// and an MSI is sent if the source is enabled.
    pub fn line_input(&mut self, gsi: u32, high: bool) {
        assert_eq!(self.machine.arch, Arch::Riscv64);
        self.edit_table(|t| t.aplic.entry(gsi).or_default().input(high));
    }

    /// RISC-V: an MSI the APLIC sent for level line `gsi` reaches the core
    /// and vector its claim programmed, an arrival on the claiming node.
    /// Says whether one was in flight.
    pub fn aplic_deliver(&mut self, gsi: u32) -> bool {
        if self
            .table()
            .aplic
            .get(&gsi)
            .is_none_or(|line| line.sent == 0)
        {
            return false;
        }
        self.edit_table(|t| t.aplic.get_mut(&gsi).expect("in flight").sent -= 1);
        let target = self.models().find_map(|(slot, m)| {
            m.irqs
                .values()
                .find(|irq| irq.source.is_line() && irq.source.id() == gsi)
                .map(|irq| (slot, irq.core, irq.vector))
        });
        if let Some((node, core, vector)) = target {
            self.model(node).interrupt_on(core, vector);
            self.after(node);
        }
        true
    }

    /// The hardware state of `core`, set by a checker test.
    #[cfg(test)]
    pub(crate) fn set_hardware(&mut self, core: usize, state: Hardware) {
        self.hardware[core] = state;
    }

    /// The machine table, changed as every kernel then reads it.
    pub(crate) fn edit_table(&mut self, edit: impl FnOnce(&mut crate::table::Table)) {
        let mut table = self.table().clone();
        edit(&mut table);
        self.share(table);
    }
}
