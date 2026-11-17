//! Start-up state: the description of the machine and of the boot image,
//! the machine start-up that builds the machine table, and the node
//! start-up that every node runs from its floor and the kept image.

use crate::caps::Clearing;
use crate::memory::hooked;
use crate::table::{RoleRecord, Table};
use crate::{CapId, Frame, Iommu, Mapping, Model, ObjId, Place, live};
use dodos_abi::x86_64::{REG_RDI, REG_RIP};
use dodos_abi::{
    BootFloors, BootMembership, CORE_ANSWER_RANGE_NS, CORES_PER_KERNEL_MAX, CSPACE_SEGMENT_SLOTS,
    CoreWord, DEBUG_AREA_BYTES, DEVICE_TABLE_SPARE, DRIVER_NONE, DeviceEntry, Evidence, Geometry,
    Governs, Kind, Member, MemberState, MemoryType, NODES_MAX, ObjectKind, Owner, Ownership,
    PageSize, RESERVES_MAX, RESERVES_MIN, ROLES_PLAIN_MAX, Rights, RoleKind, RoleWord,
    SEAL_RANGE_NS, STALE_RANGE_NS, START_FREE_SLOTS, START_PAGE_MAX_PAGES, START_SCHED_BUDGET_NS,
    START_SCHED_PERIOD_NS, START_SLOT_ASPACE, START_SLOT_CSPACE, START_SLOT_DEBUG,
    START_SLOT_FIRST, START_SLOT_IOPORTS, START_SLOT_IRQ_CONTROL, START_SLOT_MACHINE_FIXED,
    START_SLOT_MACHINE_LIVING, START_SLOT_MAILBOX, START_SLOT_MEMORY_CONTROL,
    START_SLOT_MESSAGE_PAGE, START_SLOT_MESSAGE_PAGE_MAP, START_SLOT_SCHED, START_SLOT_START_PAGE,
    START_SLOT_TASK, START_SLOT_THREAD, SWEEP_RANGE_NS, TaskId, floors, module_frames,
};
use std::collections::BTreeSet;

/// The UEFI types of memory, as the start-up rules group them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RegionKind {
    /// Loader code and data, boot services code and data, conventional
    /// memory: the RAM that is usable after `ExitBootServices`.
    Usable,
    /// The UEFI type "unusable memory".
    Unusable,
    /// Any other type: MMIO, ACPI tables and NVS, reserved, runtime services.
    Reserved,
}

/// One descriptor of the UEFI memory map, on page boundaries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub base: u64,
    pub len: u64,
    pub kind: RegionKind,
}

/// One physical core, as the start-up page describes it (`StartCore`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Core {
    /// The hardware identifier (x86-64: the APIC ID); the core list is
    /// sorted by it, so a core's position is its core number.
    pub hw_id: u64,
    /// Its kind, an index in `Machine::kinds`.
    pub kind: u16,
    /// Its fence class (The fence).
    pub fence: u8,
    /// Firmware booted it.
    pub booted: bool,
    pub package: u32,
}

/// One IOMMU unit of the DMAR table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IommuUnit {
    pub segment: u16,
    /// The domain numbers it has.
    pub domains: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IoApic {
    pub first_gsi: u32,
    pub inputs: u32,
    /// The PCI segment of the IOAPIC's source ID, as the DMAR gives it:
    /// the remapping units of that segment serve its lines.
    pub segment: u16,
}

/// A segment of the first service: mapped at `vaddr` with `rights`, read
/// and at most one of write and exec.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub vaddr: u64,
    pub len: u64,
    pub rights: Rights,
    /// Bytes of its name.
    pub name: u32,
}

/// A file of another service of the boot image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct File {
    pub len: u64,
    pub name: u32,
}

/// The architecture: only x86-64 has I/O ports; ARM64 invalidates
/// translations on the cores of a node by a broadcast in the call, x86-64
/// and RISC-V by the request of the calling core (TLB); each confirms a
/// park its own way (Node slots and core words).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Arch {
    #[default]
    X86_64,
    Aarch64,
    Riscv64,
}

/// What the start-up state is determined by: the machine, the boot image,
/// and the memory the kernel keeps (Start-up state).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub arch: Arch,
    pub memory: Vec<Region>,
    /// MAXPHYADDR.
    pub phys_bits: u32,
    /// The physical cores, in any order: the start-up sorts them by
    /// hardware identifier.
    pub cores: Vec<Core>,
    /// The kinds of core (Unlike cores), by kind number.
    pub kinds: Vec<Kind>,
    pub iommus: Vec<IommuUnit>,
    pub ioapics: Vec<IoApic>,
    pub remapping: bool,
    pub x2apic: bool,
    /// The ranges the kernel keeps, base and length, on page boundaries:
    /// the first is the living part of the machine table, the second its
    /// fixed part (the frames of slots 13 and 14).
    pub kept: Vec<(u64, u64)>,
    /// Where the loader placed the modules of the boot image, aligned on
    /// 2^k with 2^k at least their size.
    pub image_base: u64,
    /// The bases of the current and of the previous crash area, each on a
    /// chunk boundary (The crash record).
    pub crash: (u64, Option<u64>),
    /// The hardware feedback tables: package, base and bytes.
    pub feedback: Vec<(u32, u64, u64)>,
    pub segments: Vec<Segment>,
    pub files: Vec<File>,
    pub entry: u64,
    /// The free tables that the address space of the first process keeps.
    pub reserve_tables: u64,
    /// Values of the machine: the page sizes the processor supports, *E*,
    /// *L* and the end of the user range.
    pub geometry: Geometry,
    /// The address width and the page sizes of the iospaces.
    pub io_width: u32,
    pub io_page_sizes: u32,
    /// Unlike cores: whether the processor signals a change of the feedback
    /// table or of the performance capabilities (source kind 3).
    pub core_events: bool,
    /// Instructions retired, on demand: the architecture layer names a
    /// counter (x86-64: a fixed counter of the PMU); without one the count
    /// reads 0. The model's cores retire one instruction per nanosecond.
    pub counter: bool,
    /// The slot of a replacement kernel image holds one, and whether that
    /// image reads the format of a sealed block of this one.
    pub replacement: bool,
    pub replacement_reads_sealed: bool,
    /// The performance-request register (x86-64 HWP): each field is 8
    /// bits; `false` where the core has none. The performance capabilities
    /// each core reports.
    pub perf_request: bool,
    /// The base port of the console's UART, if the image declares one on
    /// I/O ports; the loader's seed of the boot identifier; the crash area
    /// of the previous boot, if one is valid.
    pub console_port: Option<u64>,
    pub boot_seed: u64,
    pub previous_crash: Option<crate::debug::CrashArea>,
    pub perf_caps: u64,
    /// The class the processor gives the running code (Intel Thread
    /// Director), as the model's processor reads it when a counted context
    /// stops; `None` where there is no classification.
    pub classifier: Option<u8>,
    /// The devices the kernel uses itself: an IOMMU unit, by its index, and
    /// a device identifier.
    pub kernel_devices: Vec<(usize, u16)>,
    /// Of the UEFI memory map, for the size of the start-up page.
    pub descriptor_size: u32,
    pub framebuffer: bool,
    /// The records of the boot image that the kernel reads.
    pub floors: BootFloors,
    pub membership: BootMembership,
    /// Whether the boot image declares a console (a unit role).
    pub console: bool,
    /// `BootNodes`: the number of node slots and the nodes the machine
    /// start-up starts, each as the window and mask of its cores; `None`
    /// for the default, one node per usable core.
    pub boot_nodes: Option<(u16, Vec<(u64, u64)>)>,
    /// The cores per node the kernel image supports (entry `Node`).
    pub cores_supported: u32,
    /// The longest hold of a node's lock measured for the image, *H*, and
    /// the delivery of one interrupt *D*: the machine start-up refuses a
    /// `CORE_ANSWER_NS` not above the longest healthy wait, 2 × *E* + *D*
    /// with *E* = *N* × *H*, and a `STALE_NS` or a `STUCK_NS` not above it
    /// with an event (D-131, D-137, D-138).
    pub hold_ns: u64,
    pub delivery_ns: u64,
    /// The image declares the hardening monitor: every core is of fence
    /// class 2.
    pub monitor: bool,
    /// The image accepts a core of fence class 0 beside other nodes (a
    /// development setting).
    pub accept_unfenced: bool,
    /// The kernel can read a processor source of entropy.
    pub processor_entropy: bool,
    /// The largest map count of a chunk (G4): `MAP_COUNT_MAX`, 32 bits; the
    /// tests make it smaller so that the rule is reached.
    pub map_count_max: u64,
    /// The chunks one call of the gate checks (G3): *E*; the tests make it
    /// smaller so that a mapping made in steps is reached on a machine
    /// whose frames touch at most *E* chunks.
    pub gate_chunks: u64,
}

/// The second geometry of the tests (D-121): a page of 16 KB with *E* =
/// 2,048 and three levels, large pages of 32 MB and 64 GB. It is a set of
/// values to test the model with, not a claim about a processor.
pub const SIXTEEN_KB: Geometry = Geometry {
    page_shift: [14, 25, 36, 0],
    entries: 2048,
    levels: 3,
    // Three levels of 2,048 entries translate 47 bits: the lower half is
    // 2^46, less the root entry of the reserved slot.
    user_end: (1 << 46) - (1 << 36),
};

/// A third geometry, of 32 KB pages, that no shipping processor has: *E* is
/// 4,096 (the largest the contract accepts), three levels translate 51
/// bits, a chunk is 128 MB. The tests that depend on the page run with it
/// too, so that no constant of 4 or 16 KB stays hidden in them.
pub const THIRTY_TWO_KB: Geometry = Geometry {
    page_shift: [15, 27, 39, 0],
    entries: 4096,
    levels: 3,
    user_end: (1 << 50) - (1 << 39),
};

impl Machine {
    /// The machine of the tests on the geometry that the environment
    /// variable `DODOS_PAGE` names: `16k` for [`SIXTEEN_KB`], `32k` for
    /// [`THIRTY_TWO_KB`], otherwise the reference machine, x86-64 with four
    /// levels. `verify` runs the tests that depend on the page with all
    /// three.
    pub fn example() -> Self {
        match std::env::var("DODOS_PAGE").as_deref() {
            Ok("16k") => Self::example_on(SIXTEEN_KB),
            Ok("32k") => Self::example_on(THIRTY_TWO_KB),
            _ => Self::example_on(dodos_abi::x86_64::GEOMETRY),
        }
    }

    /// The machine of the tests: two cores of one kind, one IOMMU unit, one
    /// IOAPIC, usable memory of 4,098 chunks (8 GB and 4 MB on the reference
    /// machine; more where a run of the second large page past the floors
    /// would not fit, as with pages of 32 KB) from the first multiple of 4 GB
    /// that is aligned on the
    /// second large page, whose top two chunks hold the crash areas, a 2 MB
    /// device range at 2 MB, the kept ranges and the modules in the second
    /// megabyte, and a first service of two segments from the first large
    /// page with one other file. Its floors are three chunks, cut into
    /// blocks of two chunks and one; its two reserves are one chunk each.
    pub fn example_on(geometry: Geometry) -> Self {
        let region = |base, len, kind| Region { base, len, kind };
        let (mb, gb): (u64, u64) = (1 << 20, 1 << 30);
        let read = Rights::READ;
        let chunk = geometry.chunk();
        let large = geometry.bytes(PageSize::Large2).unwrap_or(chunk);
        let ram = (4 * gb).next_multiple_of(large);
        // 4,096 chunks of ordinary memory, then the two crash areas; at least
        // room for an aligned run of the second large page past the floors.
        let size = (4098 * chunk).max(2 * large + 2 * chunk);
        let top = (ram + size).next_power_of_two();
        let mut feature_bytes = [0; 32];
        feature_bytes[dodos_abi::x86_64::AVX512 as usize] = 2048;
        feature_bytes[dodos_abi::x86_64::AMX as usize] = 8192;
        let kind = Kind {
            features: 1 << dodos_abi::x86_64::AVX | 1 << dodos_abi::x86_64::AMX,
            vector_bytes: 0,
            baseline_bytes: dodos_abi::x86_64::XSTATE_X87_SSE_AVX_BYTES,
            feature_bytes,
        };
        let core = |hw_id, booted| Core {
            hw_id,
            kind: 0,
            fence: if booted { 1 } else { 2 },
            booted,
            package: 0,
        };
        Self {
            arch: Arch::X86_64,
            memory: vec![
                region(0, mb, RegionKind::Unusable),
                region(mb, mb, RegionKind::Usable),
                region(2 * mb, 2 * mb, RegionKind::Reserved),
                region(4 * mb, ram - 4 * mb, RegionKind::Unusable),
                region(ram, size, RegionKind::Usable),
                region(ram + size, top - ram - size, RegionKind::Unusable),
            ],
            phys_bits: top.trailing_zeros(),
            cores: vec![core(0, true), core(2, false)],
            kinds: vec![kind],
            iommus: vec![IommuUnit {
                segment: 0,
                domains: 256,
            }],
            ioapics: vec![IoApic {
                first_gsi: 0,
                inputs: 24,
                segment: 0,
            }],
            remapping: true,
            x2apic: false,
            kept: vec![(mb, mb / 4), (mb + mb / 4, mb / 4)],
            image_base: mb + mb / 2,
            crash: (ram + size - 2 * chunk, Some(ram + size - chunk)),
            feedback: Vec::new(),
            segments: vec![
                Segment {
                    vaddr: chunk,
                    len: 0x1_0000,
                    rights: read.union(Rights::EXEC),
                    name: 4,
                },
                Segment {
                    vaddr: chunk + 0x1_0000,
                    // 16 KB, or one page where a page is larger.
                    len: geometry.page().max(0x4000),
                    rights: read.union(Rights::WRITE),
                    name: 4,
                },
            ],
            files: vec![File {
                len: 0x8000,
                name: 7,
            }],
            entry: chunk,
            reserve_tables: u64::from(geometry.levels),
            geometry,
            io_width: 48,
            io_page_sizes: 0b001,
            core_events: false,
            counter: true,
            replacement: false,
            replacement_reads_sealed: true,
            perf_request: true,
            console_port: None,
            boot_seed: 0x005E_ED0F_B007,
            previous_crash: None,
            perf_caps: 0x0A_1E_28_30,
            classifier: None,
            kernel_devices: vec![(0, 0x00f8)],
            descriptor_size: 48,
            framebuffer: false,
            floors: BootFloors {
                floor_node: 3 * chunk,
                reserve_bytes: chunk,
                reserve_per_gib: 0,
                reserves: 2,
                plain_roles: 0,
            },
            membership: BootMembership {
                stale_ms: 100,
                stuck_ms: 100,
                sweep_ms: 1000,
                seal_ms: 1000,
                core_answer_ms: 10,
                reserved: 0,
            },
            console: false,
            boot_nodes: None,
            cores_supported: CORES_PER_KERNEL_MAX as u32,
            hold_ns: 10_000,
            delivery_ns: 5_000,
            monitor: false,
            accept_unfenced: false,
            processor_entropy: true,
            map_count_max: dodos_abi::MAP_COUNT_MAX,
            gate_chunks: u64::from(geometry.entries),
        }
    }

    /// A machine with a core of fence class 0, which no other core can
    /// stop, runs one node unless the image declares the hardening monitor
    /// or accepts an unfenced core (The fence: D-116, D-117).
    fn single(&self) -> bool {
        let unstoppable = self.cores.iter().any(|core| core.fence == 0);
        unstoppable && !self.monitor && !self.accept_unfenced
    }

    /// The node slots: `BootNodes`'s number, by default one per usable
    /// core; one on a machine that runs one node.
    pub fn nodes(&self) -> usize {
        match (&self.boot_nodes, self.single()) {
            (_, true) => 1,
            (Some((slots, _)), false) => usize::from(*slots),
            (None, false) => self.cores.len(),
        }
    }

    /// The nodes the machine start-up starts, each as the core numbers of
    /// its cores in position order: `BootNodes`'s layout, by default one
    /// node per usable core. A machine that runs one node runs it on every
    /// usable core if the image supports that many, otherwise on the core
    /// firmware booted.
    pub fn initial_nodes(&self) -> Vec<Vec<usize>> {
        let count = self.cores.len();
        if self.single() {
            // Every usable core if they lie in one window of 64 core
            // numbers, have the same state features and vector lengths, and
            // are no more than the image supports; the core firmware booted
            // otherwise (Start-up state, D-134).
            let one_window = count <= dodos_abi::CORE_WINDOW_BITS as usize;
            let alike = (0..count).all(|c| self.state_kind(c) == self.state_kind(0));
            if count <= self.cores_supported as usize && one_window && alike {
                return vec![(0..count).collect()];
            }
            let booted = self.core_list().iter().position(|c| c.booted).unwrap_or(0);
            return vec![vec![booted]];
        }
        match &self.boot_nodes {
            Some((_, nodes)) => nodes
                .iter()
                .map(|&(window, mask)| {
                    (0..64)
                        .filter(|i| mask >> i & 1 == 1)
                        .map(|i| (64 * window + i) as usize)
                        .collect()
                })
                .collect(),
            None => (0..count).map(|core| vec![core]).collect(),
        }
    }

    /// Unlike cores: the kind of a core, by core number.
    pub fn kind_of(&self, core: usize) -> dodos_abi::Kind {
        self.kinds[usize::from(self.core_list()[core].kind)]
    }

    /// The features the architecture lists.
    pub fn features(&self) -> dodos_abi::FeatureList {
        match self.arch {
            Arch::X86_64 => dodos_abi::x86_64::FEATURES,
            Arch::Aarch64 => dodos_abi::aarch64::FEATURES,
            Arch::Riscv64 => dodos_abi::riscv64::FEATURES,
        }
    }

    /// The state features and vector length of a core's kind: what the
    /// cores of one node must share (Unlike cores, D-129).
    pub fn state_kind(&self, core: usize) -> (u32, u32) {
        let kind = self.kind_of(core);
        (kind.features as u32, kind.vector_bytes)
    }

    /// The cores by core number: sorted by hardware identifier.
    pub fn core_list(&self) -> Vec<Core> {
        let mut cores = self.cores.clone();
        cores.sort_by_key(|core| core.hw_id);
        cores
    }
}

/// Why the kernel refuses to start (Start-up state).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Refusal {
    /// A segment of the first service lies below the first large page or
    /// beyond the user range end.
    SegmentPlace,
    /// Two segments overlap, or a segment's virtual address is not on a
    /// page boundary.
    SegmentLayout,
    /// A segment is both writable and executable.
    WriteAndExec,
    /// The start-up page would need more than 63 pages.
    StartPage,
    /// The floors, the reserves and the kept memory do not fit, k is out of
    /// its range, or a time of `BootMembership` is out of its range.
    Floors,
    /// *E* would be more than 4,096, or the values of the machine are out
    /// of range, or there are more usable cores than `NODES_MAX`.
    MachineValues,
    /// The number of node slots is more than `NODES_MAX` or than the number
    /// of usable cores, or `BootNodes` names a core twice, a core that does
    /// not exist, more cores in one node than the image supports, or two
    /// cores of different state features or vector lengths in one node.
    Nodes,
    /// The loader hands no entropy seed and the kernel can read no
    /// processor source (Entropy).
    NoEntropy,
}

/// Ranges as sorted, disjoint [start, end) pairs.
type Ranges = Vec<(u64, u64)>;

/// `from` without `cut`.
fn without(from: &Ranges, cut: &Ranges) -> Ranges {
    let mut left = from.clone();
    for &(cs, ce) in cut {
        let mut next = Vec::new();
        for (s, e) in left {
            if ce <= s || e <= cs {
                next.push((s, e));
                continue;
            }
            if s < cs {
                next.push((s, cs));
            }
            if ce < e {
                next.push((ce, e));
            }
        }
        left = next;
    }
    left
}

fn sorted(mut ranges: Ranges) -> Ranges {
    ranges.retain(|(s, e)| s < e);
    ranges.sort();
    ranges
}

/// Untypeds of a node, and entries of the device table: a range cut into
/// the fewest blocks that are each a naturally aligned power of two. At
/// address `a`, with `L` bytes left, the block is 2^min(trailing zeros of
/// a, floor(log2 L)) bytes.
fn blocks((mut at, end): (u64, u64)) -> Vec<(u64, u64)> {
    let mut found = Vec::new();
    while at < end {
        let left = end - at;
        let log = at.trailing_zeros().min(63 - left.leading_zeros());
        found.push((at, 1 << log));
        at += 1 << log;
    }
    found
}

/// What the machine start-up computes once: the layout of the kept image
/// and of the crash areas, the floors, the reserves, the ownership words,
/// the device table and the roles.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    /// Each module: its physical base, its virtual address if it is a
    /// segment, its length, and whether it is copied (a writable segment).
    modules: Vec<(u64, Option<u64>, u64, bool)>,
    image: (u64, u64),
    crash_len: u64,
    floors: Vec<Ranges>,
    reserves: Vec<Ranges>,
    owners: Vec<u64>,
    none: Vec<u64>,
    devices: Vec<DeviceEntry>,
    roles: Vec<RoleRecord>,
    /// The nodes of the initial layout, each as its core numbers.
    pub(crate) nodes: Vec<Vec<usize>>,
}

/// The start-up page: a header, a table of entries, then their contents,
/// each on an 8-byte boundary. Its bytes, from the counts it lists.
fn start_page_bytes(machine: &Machine, layout: &Layout, untypeds: u64, cores: u64) -> u64 {
    let modules = (machine.segments.len() + machine.files.len()) as u64;
    let names: u64 = machine
        .segments
        .iter()
        .map(|s| u64::from(s.name))
        .chain(machine.files.iter().map(|f| u64::from(f.name)))
        .sum();
    let floors: u64 = layout.floors.iter().map(|f| f.len() as u64).sum();
    let reserves: u64 = layout.reserves.iter().map(|r| r.len() as u64).sum();
    let kept = machine.kept.len() as u64 + 3 + machine.feedback.len() as u64;
    let kinds = machine.kinds.len() as u64;
    let mut contents = vec![
        21 * 8,
        16 * untypeds,
        32 * machine.cores.len() as u64,
        8,
        8 + u64::from(machine.descriptor_size) * machine.memory.len() as u64,
        64,
        40,
        40,
        40 * modules + names,
        24 + 4 * cores,
        24 * floors,
        16 + 8 * machine.ioapics.len() as u64,
        16 * kept,
        56,
        80,
        8 * layout.roles.len() as u64,
        24 * reserves,
        48,
        56 * kinds,
        128 * kinds,
        56 + 4 * 2,
    ];
    if machine.framebuffer {
        contents.push(32);
    }
    if machine.crash.1.is_some() {
        contents.push(32);
    }
    if !machine.iommus.is_empty() {
        contents.push(16 * machine.iommus.len() as u64);
    }
    if !machine.feedback.is_empty() {
        contents.push(16 * machine.feedback.len() as u64);
    }
    let entries = contents.len() as u64;
    let body: u64 = contents.iter().map(|bytes| bytes.next_multiple_of(8)).sum();
    16 + 24 * entries + body
}

impl Machine {
    /// The ranges the kernel keeps: the kept ranges, the kept image, the
    /// crash areas and the feedback tables.
    fn kept_ranges(&self, image: (u64, u64), crash_len: u64) -> Ranges {
        let mut kept: Ranges = self.kept.iter().map(|&(b, l)| (b, b + l)).collect();
        kept.push(image);
        kept.push((self.crash.0, self.crash.0 + crash_len));
        if let Some(previous) = self.crash.1 {
            kept.push((previous, previous + crash_len));
        }
        kept.extend(self.feedback.iter().map(|&(_, b, l)| (b, b + l)));
        sorted(kept)
    }

    /// The refusals of the start-up, and the layout of the machine table.
    pub(crate) fn layout(&self) -> Result<Layout, Refusal> {
        let geometry = self.geometry;
        let n = self.nodes() as u64;
        // Firmware boots one core (the bootstrap processor on x86-64).
        let list = self.core_list();
        let booted: Vec<usize> = (0..list.len()).filter(|&c| list[c].booted).collect();
        if !geometry.valid()
            || self.cores.is_empty()
            || self.cores.len() > NODES_MAX
            || booted.len() != 1
        {
            return Err(Refusal::MachineValues);
        }
        // BootNodes: at most NODES_MAX slots and one per usable core; each
        // core in at most one node; a node no larger than the image
        // supports, its cores of one set of state features and lengths.
        let nodes = self.initial_nodes();
        let mut named = BTreeSet::new();
        let fits = nodes.iter().all(|cores| {
            let alike = cores
                .iter()
                .all(|&c| c < self.cores.len() && self.state_kind(c) == self.state_kind(cores[0]));
            !cores.is_empty()
                && cores.len() <= self.cores_supported as usize
                && alike
                && cores.iter().all(|&c| named.insert(c))
        });
        let slots_ok = n >= 1 && n as usize <= NODES_MAX && n as usize <= self.cores.len();
        // Each IOMMU unit leaves every node slot at least one domain number
        // (Devices and shared hardware, domain numbers, D-134).
        let domains_ok = self
            .iommus
            .iter()
            .all(|u| u64::from(u.domains.saturating_sub(2)) / n.max(1) >= 1);
        // The layout leaves no core firmware booted in no node (Start-up
        // state: the kernel refuses to start if `BootNodes` "leaves the core
        // firmware booted in no node").
        let boot_core_placed = named.contains(&booted[0]);
        if !slots_ok || !domains_ok || nodes.len() > n as usize || !fits || !boot_core_placed {
            return Err(Refusal::Nodes);
        }
        if self.boot_seed == 0 && !self.processor_entropy {
            return Err(Refusal::NoEntropy);
        }
        let (page, chunk) = (geometry.page(), geometry.chunk());
        for segment in &self.segments {
            let end = segment.vaddr.checked_add(segment.len);
            if segment.vaddr < chunk || end.is_none_or(|end| end > geometry.user_end) {
                return Err(Refusal::SegmentPlace);
            }
        }
        let mut placed: Ranges = self
            .segments
            .iter()
            .map(|s| (s.vaddr, s.vaddr + s.len.next_multiple_of(page)))
            .collect();
        placed.sort();
        let overlap = placed.windows(2).any(|pair| pair[0].1 > pair[1].0);
        if overlap || placed.iter().any(|(s, _)| !s.is_multiple_of(page)) {
            return Err(Refusal::SegmentLayout);
        }
        let both = Rights::WRITE.union(Rights::EXEC);
        if self.segments.iter().any(|s| s.rights.contains(both)) {
            return Err(Refusal::WriteAndExec);
        }
        // The kept image: the modules in order, each on a page boundary; a
        // module at least one large page long on the largest large page
        // that is not longer than it.
        let mut at = self.image_base;
        let mut modules = Vec::new();
        let segments = self
            .segments
            .iter()
            .map(|s| (s.len, Some(s.vaddr), s.rights.contains(Rights::WRITE)));
        let files = self.files.iter().map(|f| (f.len, None, false));
        for (len, vaddr, copied) in segments.chain(files) {
            let large = PageSize::ALL[1..]
                .iter()
                .filter_map(|&size| geometry.bytes(size))
                .filter(|&bytes| bytes <= len)
                .max();
            at = at.next_multiple_of(large.unwrap_or(page));
            modules.push((at, vaddr, len, copied));
            at += len.next_multiple_of(page);
        }
        let image = (self.image_base, at);
        // A debug area per physical core (D-129).
        let cores = self.cores.len() as u64;
        let crash_len = (4096 + cores * DEBUG_AREA_BYTES as u64).next_multiple_of(chunk);
        // Ordinary memory: usable RAM without what is kept, shrunk inwards
        // to pages, then cut into chunks; the parts of a range smaller than
        // a chunk are left out.
        let kept = self.kept_ranges(image, crash_len);
        let regions = |kind| {
            let regions = self.memory.iter().filter(|region| region.kind == kind);
            sorted(regions.map(|r| (r.base, r.base + r.len)).collect())
        };
        let ordinary: Ranges = without(&regions(RegionKind::Usable), &kept)
            .into_iter()
            .map(|(s, e)| (s.next_multiple_of(page), e / page * page))
            .filter(|(s, e)| s < e)
            .collect();
        let chunks: Vec<u64> = ordinary
            .iter()
            .flat_map(|&(s, e)| {
                (s.next_multiple_of(chunk)..e / chunk * chunk).step_by(chunk as usize)
            })
            .collect();
        let k = self.floors.reserves;
        let k_range = if n == 1 {
            1..=RESERVES_MAX
        } else {
            RESERVES_MIN..=RESERVES_MAX
        };
        let in_range =
            |ms: u32, (low, high): (u64, u64)| (low..=high).contains(&(u64::from(ms) * 1_000_000));
        // CORE_ANSWER_NS must stay above the longest healthy wait for the
        // largest N the image supports, and STALE_NS and STUCK_NS above it
        // with an event (and the reserve's period for STUCK_NS): a core
        // that waits does not beat (D-131, D-137).
        let answer = u64::from(self.membership.core_answer_ms) * 1_000_000;
        let largest = u64::from(self.cores_supported.min(self.cores.len() as u32));
        let measured = crate::locks::Measured {
            cores: largest,
            node_hold: self.hold_ns,
            delivery: self.delivery_ns,
        };
        let held = measured.healthy_wait();
        let event = measured.event_with_waits();
        let stale = u64::from(self.membership.stale_ms) * 1_000_000;
        let stuck = u64::from(self.membership.stuck_ms) * 1_000_000;
        let beats = stale > answer.saturating_add(event)
            && stuck > (dodos_abi::RECOVERY_PERIOD_NS + answer).saturating_add(event);
        let timed = in_range(self.membership.stale_ms, STALE_RANGE_NS)
            && in_range(self.membership.stuck_ms, STALE_RANGE_NS)
            && in_range(self.membership.sweep_ms, SWEEP_RANGE_NS)
            && in_range(self.membership.seal_ms, SEAL_RANGE_NS)
            && in_range(self.membership.core_answer_ms, CORE_ANSWER_RANGE_NS)
            && answer > held
            && beats;
        let ordinary_bytes: u64 = ordinary.iter().map(|(s, e)| e - s).sum();
        let sizes = floors(ordinary_bytes, n, self.floors, chunk)
            .filter(|sizes| sizes.total <= chunks.len() as u64 * chunk);
        let Some(sizes) = sizes.filter(|_| k_range.contains(&k) && timed) else {
            return Err(Refusal::Floors);
        };
        // Floors in ascending address order, node 0 first, then reserves.
        let mut next = chunks.iter().copied();
        let mut take = |bytes: u64| {
            let mut ranges: Ranges = Vec::new();
            for _ in 0..bytes / chunk {
                let at = next.next().expect("checked by `floors`");
                match ranges.last_mut() {
                    Some(last) if last.1 == at => last.1 += chunk,
                    _ => ranges.push((at, at + chunk)),
                }
            }
            ranges
        };
        let floors: Vec<Ranges> = (0..n).map(|_| take(sizes.floor)).collect();
        let reserves: Vec<Ranges> = (0..k).map(|_| take(sizes.reserve)).collect();
        // The ownership words: none, but for the chunks of ordinary memory
        // in no floor, free and unclaimed; a reserve's chunks name it.
        let words = 1u64 << (self.phys_bits - geometry.chunk_shift());
        let mut owners = vec![Ownership::none().raw(); words as usize];
        let in_floor: BTreeSet<u64> = floors
            .iter()
            .flatten()
            .flat_map(|&(s, e)| (s..e).step_by(chunk as usize))
            .collect();
        for &at in chunks.iter().filter(|at| !in_floor.contains(at)) {
            owners[(at / chunk) as usize] = Ownership::free(Owner::Unclaimed).raw();
        }
        for (j, ranges) in reserves.iter().enumerate() {
            for &(s, e) in ranges {
                for at in (s..e).step_by(chunk as usize) {
                    owners[(at / chunk) as usize] = Ownership::free(Owner::Reserve(j as u16)).raw();
                }
            }
        }
        let none = (0..words).filter(|&c| owners[c as usize] == 0).collect();
        // Device memory: every page below the physical width that holds no
        // chunk, no kept byte and no byte of unusable memory, in blocks; so
        // the parts of usable memory left out of the chunks are device
        // memory, and no usable page is dropped.
        let outward = |(s, e): (u64, u64)| (s / page * page, e.next_multiple_of(page));
        let mut taken: Ranges = chunks.iter().map(|&at| (at, at + chunk)).collect();
        taken.extend(kept.iter().copied().map(outward));
        taken.extend(regions(RegionKind::Unusable).into_iter().map(outward));
        let device = without(&vec![(0, 1u64 << self.phys_bits)], &sorted(taken));
        let mut devices: Vec<DeviceEntry> = device
            .into_iter()
            .flat_map(blocks)
            .filter(|&(_, len)| len >= page)
            .map(|(base, len)| DeviceEntry::used(base, len.trailing_zeros()))
            .collect();
        devices.extend((0..DEVICE_TABLE_SPARE).map(|_| DeviceEntry::unused()));
        // The roles: a unit role per IOMMU unit, per IOAPIC and for the
        // console; the reserve roles; plain role 0, `platform`, then the
        // plain roles the boot image names.
        let unit = |governs, object: usize| RoleRecord {
            kind: RoleKind::Unit,
            governs,
            object: object as u16,
        };
        let mut roles: Vec<RoleRecord> = (0..self.iommus.len())
            .map(|i| unit(Governs::IommuUnit, i))
            .chain((0..self.ioapics.len()).map(|i| unit(Governs::IoApic, i)))
            .collect();
        if self.console {
            roles.push(unit(Governs::Console, 0));
        }
        roles.extend((0..k).map(|j| RoleRecord {
            kind: RoleKind::Reserve,
            governs: Governs::Reserve,
            object: j as u16,
        }));
        if self.floors.plain_roles >= ROLES_PLAIN_MAX {
            return Err(Refusal::Floors);
        }
        roles.extend((0..=self.floors.plain_roles).map(|i| RoleRecord {
            kind: RoleKind::Plain,
            governs: Governs::Plain,
            object: i as u16,
        }));
        Ok(Layout {
            modules,
            image,
            crash_len,
            floors,
            reserves,
            owners,
            none,
            devices,
            roles,
            nodes,
        })
    }

    /// The machine table, as the machine start-up writes it: every node of
    /// the initial layout live with generation 1 and its class once the
    /// node start-ups are done, its cores owned; the other node slots dead
    /// and never started (generation 0), the other cores free; unit role j
    /// held by node j mod m among the m nodes of the layout whose fence
    /// class is the best; the reserve and plain roles vacant.
    pub(crate) fn table(&self, layout: &Layout) -> Table {
        let n = self.nodes();
        let cores = self.cores.len();
        // Under the hardening monitor every core is of class 2.
        let fence: Vec<u8> = self
            .core_list()
            .iter()
            .map(|core| if self.monitor { 2 } else { core.fence })
            .collect();
        let booted: Vec<bool> = self.core_list().iter().map(|core| core.booted).collect();
        let class = |node: &Vec<usize>| node.iter().map(|&c| fence[c]).min().unwrap_or(0);
        let classes: Vec<u8> = layout.nodes.iter().map(class).collect();
        let best = classes.iter().copied().max().unwrap_or(0);
        let best_nodes: Vec<u16> = (0..layout.nodes.len() as u16)
            .filter(|&i| classes[i as usize] == best)
            .collect();
        let mut units = 0;
        let roles: Vec<RoleWord> = layout
            .roles
            .iter()
            .map(|record| match record.kind {
                RoleKind::Unit => {
                    let holder = best_nodes[units % best_nodes.len()];
                    units += 1;
                    RoleWord::held(holder, 1)
                }
                _ => RoleWord::vacant(0),
            })
            .collect();
        let mut members = vec![Member::never(); n];
        let mut core_words = vec![CoreWord::free(false).raw(); cores];
        for (node, cores_of) in layout.nodes.iter().enumerate() {
            let live = Member::new(MemberState::Live, Evidence::None, DRIVER_NONE, 1);
            members[node] = live.with_class(classes[node]);
            for &core in cores_of {
                core_words[core] = CoreWord::owned(node as u16, 1).raw();
            }
        }
        let ring = || crate::table::RingPair {
            kernel: crate::table::Ring::new(dodos_abi::RING_KERNEL_ENTRIES),
            user: crate::table::Ring::new(dodos_abi::RING_USER_ENTRIES),
        };
        let mut core_beacons = vec![
            crate::table::CoreBeacon {
                idle_until: crate::Ephemeral(u64::MAX),
                ran: (dodos_abi::CORE_NO_SLOT, 0),
                ..Default::default()
            };
            cores
        ];
        for (node, cores_of) in layout.nodes.iter().enumerate() {
            for &core in cores_of {
                core_beacons[core].ran = (node as u16, 1);
            }
        }
        Table {
            geometry: self.geometry,
            nodes: n as u16,
            floors: layout.floors.clone(),
            reserves: layout.reserves.clone(),
            role_records: layout.roles.clone(),
            kept: self.kept_ranges(layout.image, layout.crash_len),
            none: std::sync::Arc::new(layout.none.iter().copied().collect()),
            fence,
            booted,
            state_kinds: (0..cores).map(|c| self.state_kind(c)).collect(),
            cores_supported: self.cores_supported,
            beacons: vec![
                crate::table::NodeBeacon {
                    acked: vec![0; n],
                    maps_of: vec![false; n],
                    consumed: vec![(0, 0); n],
                    ..Default::default()
                };
                n
            ],
            core_beacons,
            core_words,
            park_requests: vec![0; cores],
            stopped: vec![u64::MAX; cores],
            start_requests: vec![0; cores],
            fences: vec![Default::default(); n],
            rings: std::sync::Arc::new((0..n).map(|_| (0..n).map(|_| ring()).collect()).collect()),
            replacement: self.replacement,
            resume_replacement: vec![false; n],
            crash: std::sync::Arc::new(crate::debug::CrashArea::new(
                self.boot_seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
                self.previous_crash.as_ref().map_or(0, |p| p.sequence),
                cores,
            )),
            previous: self.previous_crash.clone(),
            ports: Default::default(),
            stale_ns: u64::from(self.membership.stale_ms) * 1_000_000,
            stuck_ns: u64::from(self.membership.stuck_ms) * 1_000_000,
            sweep_ns: u64::from(self.membership.sweep_ms) * 1_000_000,
            seal_ns: u64::from(self.membership.seal_ms) * 1_000_000,
            core_answer_ns: u64::from(self.membership.core_answer_ms) * 1_000_000,
            units: layout
                .roles
                .iter()
                .zip(&roles)
                .enumerate()
                .filter(|(_, (record, _))| {
                    record.kind == RoleKind::Unit
                        && matches!(record.governs, Governs::IommuUnit | Governs::IoApic)
                })
                .map(|(role, (_, word))| {
                    let unit = crate::table::UnitState {
                        mapped: [word.holder()].into_iter().collect(),
                        ..Default::default()
                    };
                    (role, unit)
                })
                .collect(),
            member_writes: Default::default(),
            fence_trace: Default::default(),
            gated: Default::default(),
            role_writes: Default::default(),
            owners: std::sync::Arc::new(layout.owners.clone()),
            gate_words: std::sync::Arc::new(vec![0; layout.owners.len()]),
            classes: Default::default(),
            devices: std::sync::Arc::new(layout.devices.clone()),
            device_leaves: std::sync::Arc::new(vec![0; layout.devices.len()]),
            identifiers: Default::default(),
            attach: Default::default(),
            release: false,
            stopped_time: 0,
            lines: Default::default(),
            members,
            roles: roles.clone(),
            grants: vec![Default::default(); n],
            uses: vec![Default::default(); n],
            serials: vec![Default::default(); n],
            writes: Default::default(),
            checked: Default::default(),
        }
    }
}

impl Model {
    /// The state of node 0 at start-up, and its first thread, which can
    /// run: built from the machine and the boot image by the rules of
    /// Start-up state, on the cores the layout gives node 0. The other node
    /// slots hold phantom nodes: their member words say live, and nothing
    /// else of them is modelled here; they hold no core.
    ///
    /// Node 0 runs alone: it holds every unit role, as on a machine of one
    /// node (Roles: a unit role held by a phantom would leave every request
    /// to it unanswered).
    pub fn boot(machine: &Machine) -> Result<(Self, ObjId), Refusal> {
        let layout = machine.layout()?;
        let mut table = machine.table(&layout);
        for (word, record) in table.roles.iter_mut().zip(&table.role_records) {
            if record.kind == RoleKind::Unit {
                *word = RoleWord::held(0, word.epoch());
            }
        }
        for unit in table.units.values_mut() {
            unit.mapped = [0].into_iter().collect();
        }
        let live = Member::new(MemberState::Live, Evidence::None, DRIVER_NONE, 1);
        let class = table.members[0].class();
        for member in table.members.iter_mut().skip(1) {
            *member = live.with_class(class);
        }
        for word in table.core_words.iter_mut() {
            if CoreWord::from_raw(*word).is_some_and(|w| w.slot() != 0) {
                *word = CoreWord::free(false).raw();
            }
        }
        let cores = layout.nodes[0].clone();
        Self::start(machine, &layout, table, 0, &cores)
    }

    /// The node start-up of `node` on `cores` (core numbers, in position
    /// order), with the machine table it shares.
    pub(crate) fn start(
        machine: &Machine,
        layout: &Layout,
        table: Table,
        node: u16,
        cores: &[usize],
    ) -> Result<(Self, ObjId), Refusal> {
        let page = machine.geometry.page();
        let untypeds: Vec<(u64, u64)> = layout.floors[node as usize]
            .iter()
            .flat_map(|&range| blocks(range))
            .collect();
        let start_bytes =
            start_page_bytes(machine, layout, untypeds.len() as u64, cores.len() as u64);
        let start_pages = start_bytes.div_ceil(page);
        if start_pages > START_PAGE_MAX_PAGES {
            return Err(Refusal::StartPage);
        }
        let parts = cores
            .iter()
            .map(|&number| crate::cores::CorePart {
                number: number as u32,
                ..Default::default()
            })
            .collect();
        let mut model = Self {
            machine: machine.clone(),
            node,
            table,
            cores: parts,
            // The kernel epoch starts at 1; the barrier epoch at 0 covers
            // no watermark (D-135, D-138).
            kernel_epoch: 1,
            ..Self::default()
        };
        let thread = model.build(layout, &untypeds, start_pages);
        Ok((model, thread))
    }

    /// Slots: the start-up space and what it holds, in the order of the
    /// table of Start-up state.
    fn build(&mut self, layout: &Layout, untypeds: &[(u64, u64)], start_pages: u64) -> ObjId {
        let machine = self.machine.clone();
        let geometry = machine.geometry;
        let page = geometry.page();
        let space = ObjId(self.next_id);
        let all = Rights::ALL.without(Rights::EXEC);
        let add = |model: &mut Self, kind, size, index: u32, rights| {
            let cap = model.create(kind, size, None, rights, Place { space, index });
            model.caps[&cap].object
        };
        // The number of slots: the multiple of 128 at or above 16 + every
        // capability that depends on the machine + 4,096.
        let frames_of = |phys, virt, len| module_frames(phys, virt, len, geometry).count();
        let start_at = untypeds[0].0;
        let copied: usize = layout
            .modules
            .iter()
            .filter(|m| m.3)
            .map(|m| frames_of(start_at, m.1, m.2))
            .sum();
        let kept: usize = layout
            .modules
            .iter()
            .map(|m| frames_of(m.0, m.1, m.2))
            .sum();
        let crash = 1 + usize::from(machine.crash.1.is_some());
        let used = START_SLOT_FIRST as usize
            + machine.nodes()
            + machine.iommus.len()
            + layout.roles.len()
            + machine.feedback.len()
            + untypeds.len()
            + copied
            + kept
            + crash;
        let slots =
            (used + START_FREE_SLOTS as usize).next_multiple_of(CSPACE_SEGMENT_SLOTS as usize);
        add(
            self,
            ObjectKind::CSpace,
            slots as u64,
            START_SLOT_CSPACE,
            all,
        );
        let aspace = add(self, ObjectKind::ASpace, 0, START_SLOT_ASPACE, all);
        let thread = add(self, ObjectKind::Thread, 0, START_SLOT_THREAD, all);
        let sched = add(self, ObjectKind::Sched, 0, START_SLOT_SCHED, all);
        add(self, ObjectKind::Debug, 0, START_SLOT_DEBUG, all);
        add(self, ObjectKind::IrqControl, 0, START_SLOT_IRQ_CONTROL, all);
        // I/O ports: slot 7 is empty on other architectures.
        if machine.arch == Arch::X86_64 {
            let ports = add(self, ObjectKind::IoPorts, 0, START_SLOT_IOPORTS, all);
            let root = self.objects[&ports].root;
            live(&mut self.caps, root).badge = dodos_abi::IoPortRange::ALL.raw();
        }
        // The start-up task: identifier 1 of this node, marks 0, mark-aware.
        let task = add(self, ObjectKind::Task, 0, START_SLOT_TASK, all);
        let start = live(&mut self.tasks, task);
        (start.id, start.aware) = (TaskId::new(self.node, 1), true);
        self.task_counter = 1;
        // The memory_control has every right, `exec` included (rule 8).
        add(
            self,
            ObjectKind::MemoryControl,
            0,
            START_SLOT_MEMORY_CONTROL,
            Rights::ALL,
        );
        // The kept frames of the living and fixed parts, `read` and `derive`.
        let read_derive = Rights::READ.union(Rights::DERIVE);
        let parts = [START_SLOT_MACHINE_LIVING, START_SLOT_MACHINE_FIXED];
        for (index, &(base, len)) in parts.into_iter().zip(&machine.kept) {
            let place = Place { space, index };
            self.kept_frame(place, base, PageSize::Page, len / page, read_derive);
        }
        let mailbox = add(self, ObjectKind::Endpoint, 0, START_SLOT_MAILBOX, all);
        self.mailbox = Some(mailbox);
        self.start_space = Some(space);
        let mut index = START_SLOT_FIRST;
        let mut next = || {
            index += 1;
            index - 1
        };
        // One root `node` capability per node slot, in index order.
        for slot in 0..machine.nodes() {
            let node = add(self, ObjectKind::Node, 0, next(), all);
            self.nodes.insert(node, slot as u16);
        }
        for unit in 0..machine.iommus.len() {
            let iommu = add(self, ObjectKind::Iommu, 0, next(), all);
            let state = Iommu {
                unit,
                ..Iommu::default()
            };
            self.iommus.insert(iommu, state);
        }
        for role in 0..layout.roles.len() {
            let object = add(self, ObjectKind::Role, 0, next(), all);
            self.roles.insert(object, role);
        }
        for &(_, base, bytes) in &machine.feedback {
            let place = Place {
                space,
                index: next(),
            };
            self.kept_frame(
                place,
                base,
                PageSize::Page,
                bytes.div_ceil(page),
                read_derive,
            );
        }
        // The untypeds of the floor, by ascending address, with every
        // right, `exec` included; the clean mark of each is at its start.
        let mut floor = Vec::new();
        for &(base, bytes) in untypeds {
            let untyped = add(self, ObjectKind::Untyped, bytes, next(), Rights::ALL);
            live(&mut self.untypeds, untyped).base = base;
            floor.push(untyped);
        }
        let start_untyped = floor[0];
        let start_cap = self.objects[&start_untyped].root;
        // The copied segments, then the start-up page and the message page:
        // frames of the start-up untyped at depth 2, with every right but
        // `exec`.
        let child = |model: &mut Self, size: PageSize, pages: u64, index: u32| {
            let bytes = model.bytes(size);
            let taken = model
                .take(start_untyped, pages * bytes, bytes, Clearing::None)
                .expect("the start-up untyped holds the frames of the start-up");
            let place = Place { space, index };
            let cap = model.create(ObjectKind::Frame, pages, Some(start_cap), all, place);
            live(&mut model.objects, model.caps[&cap].object).boot = true;
            let base = model.untypeds[&start_untyped].base + taken.at;
            let frame = Frame {
                page: size,
                pages,
                memory: MemoryType::WriteBack,
                base,
                kept: false,
            };
            model.frames.insert(model.caps[&cap].object, frame);
            cap
        };
        let mut mappings = Vec::new();
        for &(_, vaddr, len, copied) in &layout.modules {
            let Some(vaddr) = vaddr.filter(|_| copied) else {
                continue;
            };
            let untyped = &self.untypeds[&start_untyped];
            let at = untyped.base + untyped.watermark;
            for frame in module_frames(at, Some(vaddr), len, geometry) {
                let cap = child(self, frame.size, frame.pages, next());
                let rights = Rights::READ.union(Rights::WRITE);
                mappings.push((cap, vaddr + frame.offset, rights));
            }
        }
        let start_page = child(self, PageSize::Page, start_pages, START_SLOT_START_PAGE);
        let message_page = child(self, PageSize::Page, 1, START_SLOT_MESSAGE_PAGE);
        let place = Place {
            space,
            index: START_SLOT_MESSAGE_PAGE_MAP,
        };
        self.derive(message_page, all, 0, place);
        let message_map = self.caps[&message_page].children[0];
        // The kept frames of the modules, in module order, each a run of
        // consecutive slots, `read`, `exec` and `derive`; a segment that is
        // not copied is mapped from them.
        let image_rights = Rights::READ.union(Rights::EXEC).union(Rights::DERIVE);
        let segments = machine.segments.iter().map(|s| Some(s.rights));
        let files = machine.files.iter().map(|_| None);
        for (&(phys, vaddr, len, copied), rights) in
            layout.modules.iter().zip(segments.chain(files))
        {
            for frame in module_frames(phys, vaddr, len, geometry) {
                let place = Place {
                    space,
                    index: next(),
                };
                let base = phys + frame.offset;
                let cap = self.kept_frame(place, base, frame.size, frame.pages, image_rights);
                if let (Some(vaddr), Some(rights), false) = (vaddr, rights, copied) {
                    mappings.push((cap, vaddr + frame.offset, rights));
                }
            }
        }
        // The kept frames of the crash areas, `read` and `derive`, of first
        // large pages.
        let chunk_pages = layout.crash_len / geometry.chunk();
        for base in std::iter::once(machine.crash.0).chain(machine.crash.1) {
            let place = Place {
                space,
                index: next(),
            };
            self.kept_frame(place, base, PageSize::Large1, chunk_pages, read_derive);
        }
        // Address space: each segment at its address with its rights, the
        // message page at P read and write through slot 11, the start-up
        // page at 2 × P read only through slot 9.
        mappings.push((start_page, 2 * page, Rights::READ));
        mappings.push((message_map, page, Rights::READ.union(Rights::WRITE)));
        for (cap, address, rights) in mappings {
            let count = self.frames[&self.caps[&cap].object].pages;
            let mapping = Mapping {
                space: aspace,
                address,
                first: 0,
                count,
                rights,
                partial: false,
                asked: count,
            };
            self.record_mapping(cap, aspace, mapping);
        }
        // The address space was built with the tables these mappings need
        // and the reserve of free tables the boot image states.
        let hooked = hooked(&self.aspaces[&aspace], geometry.levels, geometry);
        live(&mut self.aspaces, aspace).tables = hooked + machine.reserve_tables;
        // The first thread: bound to the space in slot 1, the address space
        // in slot 2 and the message page in slot 10; not suspended.
        self.bind_space(thread, Some(space));
        self.bind_aspace(thread, Some(aspace));
        self.bind_page(thread, Some(message_page));
        live(&mut self.tasks, task).threads.push(thread);
        live(&mut self.threads, thread).task = Some(task);
        // Registers: the entry point, and the address of the start-up page
        // in the first argument register; the others 0, the flags fixed.
        let first = live(&mut self.threads, thread);
        first.suspended = false;
        first.regs[REG_RIP] = machine.entry;
        first.regs[REG_RDI] = 2 * page;
        // Scheduling: budget 9,843,750 ns, period 10 ms, class 1, the cores
        // set of every core of the node; the recovery reserve holds the
        // rest. The node's other cores have nothing to run: they are idle.
        self.first_context(sched, thread, START_SCHED_BUDGET_NS, START_SCHED_PERIOD_NS);
        for position in 1..self.width() {
            self.enter_idle(position);
        }
        self.build_kernel_space();
        self.peers = (0..self.table.nodes)
            .map(|_| crate::rings::Peer::new())
            .collect();
        self.unit_requests = vec![None; dodos_abi::UNIT_REQUESTS];
        self.built = self.table.generation(self.node);
        self.ready = true;
        // Debugging: each core that starts in a node keeps its debug area
        // if its header is right, with the node slot and generation;
        // otherwise it is cleared. The first entry of the core at position
        // 0 is the text of the node's start (code 1).
        self.enter_debug_areas();
        let text = u64::from_le_bytes(*b"start\0\0\0");
        self.trace(dodos_abi::TraceCode::Text, None, [text, 0, 0]);
        thread
    }

    /// A kept frame: ordinary memory, write-back, never cleared, made from
    /// no untyped, with the rights its row gives.
    fn kept_frame(
        &mut self,
        place: Place,
        base: u64,
        size: PageSize,
        pages: u64,
        rights: Rights,
    ) -> CapId {
        let cap = self.create(ObjectKind::Frame, pages, None, rights, place);
        let frame = Frame {
            page: size,
            pages,
            memory: MemoryType::WriteBack,
            base,
            kept: true,
        };
        self.frames.insert(self.caps[&cap].object, frame);
        cap
    }
}
