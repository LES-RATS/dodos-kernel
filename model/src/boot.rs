//! Start-up state: the description of the machine and of the boot image,
//! and the state the kernel of the boot node builds from it by the rules of
//! the contract.

use crate::caps::Clearing;
use crate::memory::{USER_END, hooked};
use crate::{Frame, Mapping, Model, ObjId, Place, live};
use dodos_abi::x86_64::{REG_RDI, REG_RIP};
use dodos_abi::{
    CSPACE_MAX_SLOTS, CSPACE_SEGMENT_SLOTS, FIRST_SERVICE_MIN_ADDR, FRAME_MAX_PAGES, MemoryType,
    ObjectKind, PageSize, Rights, SHARE_ALIGN, START_FREE_SLOTS, START_MESSAGE_PAGE_ADDR,
    START_PAGE_ADDR, START_PAGE_MAX_PAGES, START_RESERVE_BYTES, START_SLOT_ASPACE,
    START_SLOT_CSPACE, START_SLOT_DEBUG, START_SLOT_FIRST, START_SLOT_IOPORTS,
    START_SLOT_IRQ_CONTROL, START_SLOT_MESSAGE_PAGE, START_SLOT_MESSAGE_PAGE_MAP, START_SLOT_SCHED,
    START_SLOT_START_PAGE, START_SLOT_TASK, START_SLOT_THREAD,
};

const PAGE: u64 = PageSize::Size4K.bytes();

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

/// One descriptor of the UEFI memory map, on 4 KB boundaries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub base: u64,
    pub len: u64,
    pub kind: RegionKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Core {
    pub apic_id: u32,
    /// 0 performance, 1 efficiency.
    pub class: u32,
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

/// What the start-up state is determined by: the machine, the boot image,
/// and the memory the kernel keeps (Start-up state).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub memory: Vec<Region>,
    /// MAXPHYADDR.
    pub phys_bits: u32,
    /// The boot core first, then the others in any order.
    pub cores: Vec<Core>,
    pub iommus: Vec<IommuUnit>,
    pub ioapics: Vec<IoApic>,
    pub remapping: bool,
    pub x2apic: bool,
    /// The ranges the kernel keeps, base and length, on 4 KB boundaries.
    pub kept: Vec<(u64, u64)>,
    /// Where the loader placed the image region, aligned on its size.
    pub image_base: u64,
    pub segments: Vec<Segment>,
    pub files: Vec<File>,
    pub entry: u64,
    /// The free tables that the address space of the first process keeps.
    pub reserve_tables: u64,
    /// The page sizes of the processor, bit `i` for `PageSize` `i`.
    pub page_sizes: u32,
    /// The address width and the page sizes of the iospaces.
    pub io_width: u32,
    pub io_page_sizes: u32,
    /// The devices the kernel uses itself: an IOMMU unit, by its index, and
    /// a device identifier.
    pub kernel_devices: Vec<(usize, u16)>,
    /// Of the UEFI memory map, for the size of the start-up page.
    pub descriptor_size: u32,
    pub framebuffer: bool,
    /// Bytes of the crash record of the previous boot, 0 if none.
    pub crash_record: u64,
}

impl Machine {
    /// The machine of the tests: two cores, one IOMMU unit, 8 GB of
    /// ordinary memory from 4 GB, a 2 MB device range at 2 MB, the kept
    /// ranges and the image region in the first 2 MB, and a first service of
    /// two segments with one other file.
    pub fn example() -> Self {
        let region = |base, len, kind| Region { base, len, kind };
        let (mb, gb) = (1 << 20, 1 << 30);
        let read = Rights::READ;
        Self {
            memory: vec![
                region(0, mb, RegionKind::Unusable),
                region(mb, mb, RegionKind::Usable),
                region(2 * mb, 2 * mb, RegionKind::Reserved),
                region(4 * mb, 4 * gb - 4 * mb, RegionKind::Unusable),
                region(4 * gb, 8 * gb, RegionKind::Usable),
                region(12 * gb, 4 * gb, RegionKind::Unusable),
            ],
            phys_bits: 34,
            cores: vec![
                Core {
                    apic_id: 0,
                    class: 0,
                },
                Core {
                    apic_id: 2,
                    class: 0,
                },
            ],
            iommus: vec![IommuUnit {
                segment: 0,
                domains: 256,
            }],
            ioapics: vec![IoApic {
                first_gsi: 0,
                inputs: 24,
            }],
            remapping: true,
            x2apic: false,
            kept: vec![(mb, mb / 2)],
            image_base: mb + mb / 2,
            segments: vec![
                Segment {
                    vaddr: 0x20_0000,
                    len: 0x1_0000,
                    rights: read.union(Rights::EXEC),
                    name: 4,
                },
                Segment {
                    vaddr: 0x21_0000,
                    len: 0x4000,
                    rights: read.union(Rights::WRITE),
                    name: 4,
                },
            ],
            files: vec![File {
                len: 0x8000,
                name: 7,
            }],
            entry: 0x20_0000,
            reserve_tables: 4,
            page_sizes: 0b111,
            io_width: 48,
            io_page_sizes: 0b011,
            kernel_devices: vec![(0, 0x00f8)],
            descriptor_size: 48,
            framebuffer: false,
            crash_record: 0,
        }
    }
}

/// Why the kernel refuses to start (Start-up state).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Refusal {
    /// A segment of the first service lies below 2 MB or beyond the user
    /// range.
    SegmentPlace,
    /// A segment is both writable and executable.
    WriteAndExec,
    /// The start-up page would need more than 63 pages.
    StartPage,
    /// The start-up space would exceed 2^20 slots.
    Slots,
    /// CONTRACT-GAP (Start-up state): "one frame per module", but a frame
    /// has at most 512 pages and the frames are of 4 KB pages, so a module
    /// of more than 2 MB has no frame; the refusals do not list it.
    ModuleSize,
    /// CONTRACT-GAP (Start-up state): a segment whose address is not on a
    /// 4 KB boundary, or two segments that overlap, cannot be mapped; the
    /// refusals do not list them.
    SegmentLayout,
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

/// Untypeds of the boot node: a range cut into the fewest blocks that are
/// each a naturally aligned power of two. At address `a`, with `L` bytes
/// left, the block is 2^min(trailing zeros of a, floor(log2 L)) bytes.
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

/// The bytes of a module once laid out: a whole number of pages.
fn laid_out(len: u64) -> u64 {
    len.next_multiple_of(PAGE)
}

/// The start-up page: a header, a table of entries, then their contents,
/// each on an 8-byte boundary. Its bytes, from the counts it lists.
fn start_page_bytes(machine: &Machine, untypeds: u64, nodes: u64) -> u64 {
    let modules = (machine.segments.len() + machine.files.len()) as u64;
    let names: u64 = machine
        .segments
        .iter()
        .map(|s| u64::from(s.name))
        .sum::<u64>()
        + machine.files.iter().map(|f| u64::from(f.name)).sum::<u64>();
    let mut contents = vec![
        19 * 8,
        16 * untypeds,
        24 * machine.cores.len() as u64,
        8,
        8 + u64::from(machine.descriptor_size) * machine.memory.len() as u64,
        64,
        40,
        40,
        32 * modules + names,
        16,
        24 * nodes,
        16 + 8 * machine.ioapics.len() as u64,
        16 * machine.kept.len() as u64,
        40,
    ];
    if machine.framebuffer {
        contents.push(32);
    }
    if machine.crash_record != 0 {
        contents.push(machine.crash_record);
    }
    if !machine.iommus.is_empty() {
        contents.push(16 * machine.iommus.len() as u64);
    }
    let entries = contents.len() as u64;
    let body: u64 = contents.iter().map(|bytes| bytes.next_multiple_of(8)).sum();
    16 + 24 * entries + body
}

impl Model {
    /// The state at start-up, and the first thread, which can run: built
    /// from the machine and the boot image by the rules of Start-up state.
    pub fn boot(machine: &Machine) -> Result<(Self, ObjId), Refusal> {
        let ordinary_ranges = |kind| {
            let regions = machine.memory.iter().filter(|region| region.kind == kind);
            sorted(regions.map(|r| (r.base, r.base + r.len)).collect())
        };
        // The refusals: where the segments lie, and write or execute.
        for segment in &machine.segments {
            let end = segment.vaddr.checked_add(segment.len);
            if segment.vaddr < FIRST_SERVICE_MIN_ADDR || end.is_none_or(|end| end > USER_END) {
                return Err(Refusal::SegmentPlace);
            }
            if segment.rights.contains(Rights::WRITE.union(Rights::EXEC)) {
                return Err(Refusal::WriteAndExec);
            }
        }
        let mut placed: Ranges = machine
            .segments
            .iter()
            .map(|s| (s.vaddr, s.vaddr + laid_out(s.len)))
            .collect();
        placed.sort();
        let overlap = placed.windows(2).any(|pair| pair[0].1 > pair[1].0);
        if overlap || placed.iter().any(|(s, _)| !s.is_multiple_of(PAGE)) {
            return Err(Refusal::SegmentLayout);
        }
        let lens = machine.segments.iter().map(|s| s.len);
        let lens: Vec<u64> = lens.chain(machine.files.iter().map(|f| f.len)).collect();
        if lens
            .iter()
            .any(|&len| laid_out(len) > FRAME_MAX_PAGES * PAGE)
        {
            return Err(Refusal::ModuleSize);
        }
        // 2. The image region: 2^k bytes, the smallest power of two that
        // holds the modules and the reserve of 256 KB.
        let modules: u64 = lens.iter().map(|&len| laid_out(len)).sum();
        let image = (modules + START_RESERVE_BYTES).next_power_of_two();
        let image_range = (machine.image_base, machine.image_base + image);
        assert!(
            machine.image_base.is_multiple_of(image),
            "the loader aligns the image"
        );
        // 1 and 3. The kept ranges; the ordinary memory: usable RAM without
        // the kept ranges and the image region, shrunk inwards to 4 KB.
        let kept = sorted(machine.kept.iter().map(|&(b, l)| (b, b + l)).collect());
        let mut cut = kept.clone();
        cut.push(image_range);
        let cut = sorted(cut);
        let ordinary: Ranges = without(&ordinary_ranges(RegionKind::Usable), &cut)
            .into_iter()
            .map(|(s, e)| (s.next_multiple_of(PAGE), e / PAGE * PAGE))
            .filter(|(s, e)| s < e)
            .collect();
        // 4. Shares: node 0, the boot node, takes the first S bytes in order
        // of ascending address; the last node takes the rest.
        let nodes = machine.cores.len() as u64;
        let total: u64 = ordinary.iter().map(|(s, e)| e - s).sum();
        let share = total / nodes / SHARE_ALIGN * SHARE_ALIGN;
        let mut wanted = if nodes == 1 { total } else { share };
        let mut boot_share = Ranges::new();
        for &(s, e) in &ordinary {
            let take = (e - s).min(wanted);
            if take > 0 {
                boot_share.push((s, s + take));
            }
            wanted -= take;
        }
        // 5 and 6. The untypeds: the image untyped, the ordinary ones of the
        // boot share, then device memory: every address below 2^MAXPHYADDR
        // that is not RAM of a share, not in the image region, not kept and
        // not unusable.
        //
        // CONTRACT-GAP (Start-up state, rule 5): the image region is "added
        // to the share of the boot node", and then "each range of the boot
        // share" is cut into untypeds: read literally, the image region
        // would be given twice, once as the image untyped. Here it is given
        // once, as the image untyped.
        let ordinary_blocks: Vec<_> = boot_share.iter().flat_map(|&r| blocks(r)).collect();
        let mut taken = ordinary.clone();
        taken.extend(cut.iter().copied());
        taken.extend(ordinary_ranges(RegionKind::Unusable));
        let device = without(&vec![(0, 1u64 << machine.phys_bits)], &sorted(taken));
        let device_blocks: Vec<_> = device.into_iter().flat_map(blocks).collect();
        let untypeds = 1 + ordinary_blocks.len() + device_blocks.len();
        // The start-up page, and the number of slots.
        let start_bytes = start_page_bytes(machine, untypeds as u64, nodes);
        let start_pages = start_bytes.div_ceil(PAGE);
        if start_pages > START_PAGE_MAX_PAGES {
            return Err(Refusal::StartPage);
        }
        let units = machine.iommus.len();
        let used = START_SLOT_FIRST as usize + machine.cores.len() + units + untypeds + lens.len();
        let slots =
            (used + START_FREE_SLOTS as usize).next_multiple_of(CSPACE_SEGMENT_SLOTS as usize);
        if slots > CSPACE_MAX_SLOTS as usize {
            return Err(Refusal::Slots);
        }
        let mut model = Self {
            machine: machine.clone(),
            page_sizes: machine.page_sizes,
            ..Self::default()
        };
        let blocks = [&ordinary_blocks[..], &device_blocks[..]];
        let thread = model.build(machine, slots as u64, blocks, image, start_pages);
        Ok((model, thread))
    }

    /// Slots: the start-up space and what it holds, in the order of the
    /// table of Start-up state. Every start-up capability but the frames is
    /// the root of its object, at depth 1, with every right of its kind.
    fn build(
        &mut self,
        machine: &Machine,
        slots: u64,
        [ordinary, device]: [&[(u64, u64)]; 2],
        image: u64,
        start_pages: u64,
    ) -> ObjId {
        let space = ObjId(self.next_id);
        let all = Rights::ALL;
        let add = |model: &mut Self, kind, size, index: u32, rights| {
            let cap = model.create(kind, size, None, rights, Place { space, index });
            model.caps[&cap].object
        };
        add(self, ObjectKind::CSpace, slots, START_SLOT_CSPACE, all);
        let aspace = add(self, ObjectKind::ASpace, 0, START_SLOT_ASPACE, all);
        let thread = add(self, ObjectKind::Thread, 0, START_SLOT_THREAD, all);
        add(self, ObjectKind::Sched, 0, START_SLOT_SCHED, all);
        add(self, ObjectKind::Debug, 0, START_SLOT_DEBUG, all);
        add(self, ObjectKind::IrqControl, 0, START_SLOT_IRQ_CONTROL, all);
        add(self, ObjectKind::IoPorts, 0, START_SLOT_IOPORTS, all);
        add(self, ObjectKind::Task, 0, START_SLOT_TASK, all);
        let mut index = START_SLOT_FIRST;
        let mut next = || {
            index += 1;
            index - 1
        };
        // One `core` capability per core: the boot core first, then the
        // others by ascending APIC ID; a core's place is its node index.
        let mut others: Vec<Core> = machine.cores[1..].to_vec();
        others.sort_by_key(|core| core.apic_id);
        for (node, _) in machine.cores[..1].iter().chain(&others).enumerate() {
            let core = add(self, ObjectKind::Core, 0, next(), all);
            self.cores.insert(core, node as u32);
        }
        for _ in &machine.iommus {
            add(self, ObjectKind::Iommu, 0, next(), all);
        }
        // The untypeds: the image untyped, the ordinary ones, the device
        // ones without `exec`. The clean mark of each is at its start.
        let image_untyped = add(self, ObjectKind::Untyped, image, next(), all);
        for &(_, bytes) in ordinary {
            add(self, ObjectKind::Untyped, bytes, next(), all);
        }
        for &(_, bytes) in device {
            let untyped = add(
                self,
                ObjectKind::Untyped,
                bytes,
                next(),
                all.without(Rights::EXEC),
            );
            live(&mut self.untypeds, untyped).device = true;
        }
        // The frames, children of the image untyped at depth 2, in the order
        // modules, start-up page, message page; its watermark stands after
        // the message page, and its clean mark at its watermark.
        let image_cap = self.objects[&image_untyped].root;
        let frame = |model: &mut Self, pages: u64, index: u32| {
            let bytes = pages * PAGE;
            model
                .take(image_untyped, bytes, PAGE, Clearing::None)
                .expect("the image region holds the modules and the reserve");
            let place = Place { space, index };
            let cap = model.create(ObjectKind::Frame, pages, Some(image_cap), all, place);
            let frame = Frame {
                page: PageSize::Size4K,
                pages,
                memory: MemoryType::WriteBack,
            };
            model.frames.insert(model.caps[&cap].object, frame);
            cap
        };
        let lens = machine.segments.iter().map(|s| s.len);
        let lens: Vec<u64> = lens.chain(machine.files.iter().map(|f| f.len)).collect();
        let module_caps: Vec<_> = lens
            .iter()
            .map(|&len| frame(self, laid_out(len) / PAGE, next()))
            .collect();
        let start_page = frame(self, start_pages, START_SLOT_START_PAGE);
        let message_page = frame(self, 1, START_SLOT_MESSAGE_PAGE);
        let place = Place {
            space,
            index: START_SLOT_MESSAGE_PAGE_MAP,
        };
        self.derive(message_page, all, 0, place);
        let message_map = self.caps[&message_page].children[0];
        // Address space: each segment at its address with its rights, the
        // message page at 0x1000 read and write through slot 11, the
        // start-up page at 0x2000 read only through slot 9.
        let mut mappings: Vec<_> = machine
            .segments
            .iter()
            .zip(&module_caps)
            .map(|(segment, &cap)| (cap, segment.vaddr, segment.rights))
            .collect();
        mappings.push((start_page, START_PAGE_ADDR, Rights::READ));
        let read_write = Rights::READ.union(Rights::WRITE);
        mappings.push((message_map, START_MESSAGE_PAGE_ADDR, read_write));
        for (cap, address, rights) in mappings {
            let count = self.frames[&self.caps[&cap].object].pages;
            let mapping = Mapping {
                space: aspace,
                address,
                first: 0,
                count,
                rights,
            };
            self.record_mapping(cap, aspace, mapping);
        }
        // The address space was built with the tables these mappings need
        // and the reserve of free tables the boot image states.
        let tables = hooked(&self.aspaces[&aspace], 4) + machine.reserve_tables;
        live(&mut self.aspaces, aspace).tables = tables;
        // The first thread: bound to the space in slot 1, the address space
        // in slot 2 and the message page in slot 10; not suspended.
        self.bind_space(thread, Some(space));
        self.bind_aspace(thread, Some(aspace));
        self.bind_page(thread, Some(message_page));
        // Registers: the entry point, and the address of the start-up page
        // in the first argument register; the others 0, the flags fixed.
        let first = live(&mut self.threads, thread);
        first.suspended = false;
        first.regs[REG_RIP] = machine.entry;
        first.regs[REG_RDI] = START_PAGE_ADDR;
        thread
    }
}
