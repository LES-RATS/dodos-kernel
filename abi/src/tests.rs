use super::*;

#[test]
fn error_codes_are_stable() {
    let errors = [
        Error::Ok,
        Error::Gone,
        Error::Again,
        Error::Denied,
        Error::Invalid,
        Error::NoMemory,
        Error::NoSlot,
        Error::Busy,
        Error::Budget,
        Error::WouldWait,
        Error::Unsupported,
        Error::Cancelled,
    ];
    for (number, error) in errors.into_iter().enumerate() {
        assert_eq!(error as usize, number, "{error:?}");
    }
}

#[test]
fn thread_cancel_comes_after_the_calls_that_were_there_before() {
    assert_eq!(Call::TraceEmit as u32, 44);
    assert_eq!(Call::ThreadCancel as u32, 45);
}

#[test]
fn irq_claim_and_sched_read_come_last() {
    assert_eq!(Call::IrqClaim as u32, 46);
    assert_eq!(Call::SchedRead as u32, 47);
}

#[test]
fn calls_keep_their_place() {
    use self::Call as C;
    use C::*;
    let calls = [
        CapDerive,
        CapMove,
        CapDelete,
        CapRevoke,
        CapInspect,
        CSpaceGrow,
        UntypedRetype,
        ASpaceMap,
        ASpaceUnmap,
        ASpaceProtect,
        C::Call,
        ReplyRecv,
        Recv,
        Send,
        Signal,
        Wait,
        Poll,
        NotifyBind,
        ThreadConfigure,
        ThreadReadRegs,
        ThreadWriteRegs,
        ThreadResume,
        ThreadSuspend,
        Yield,
        SchedConfigure,
        SchedBind,
        SchedUnbind,
        SchedSetCores,
        TimerSet,
        TimerCancel,
        IrqBind,
        IrqAck,
        IoSpaceMap,
        IoSpaceUnmap,
        IoSpaceAttach,
        TaskExempt,
        TaskAttach,
        TaskMark,
        TaskRevoke,
        TaskRead,
        CoreStart,
        CoreStop,
        CoreStatus,
        DebugWrite,
        TraceEmit,
        ThreadCancel,
        IrqClaim,
        SchedRead,
    ];
    assert_eq!(calls.len(), 48);
    for (number, call) in calls.into_iter().enumerate() {
        assert_eq!(call as usize, number, "{call:?}");
    }
}

#[test]
fn object_kinds_and_page_sizes_have_their_numbers() {
    use ObjectKind::*;
    let kinds = [
        Untyped, CSpace, ASpace, Frame, Thread, Sched, Endpoint, Reply, Notify, Timer, Irq,
        IoPorts, IoSpace, Task, Core, Debug, Vcpu, IrqControl, Iommu,
    ];
    for (number, kind) in kinds.into_iter().enumerate() {
        assert_eq!(kind as usize, number, "{kind:?}");
    }
    let pages = [PageSize::Size4K, PageSize::Size2M, PageSize::Size1G];
    for (number, page) in pages.into_iter().enumerate() {
        assert_eq!(page as usize, number, "{page:?}");
        assert_eq!(page.bytes(), 1 << (12 + 9 * number));
        assert_eq!(PageSize::from_raw(number as u64), Some(page));
    }
    assert_eq!(PageSize::from_raw(3), None);
}

#[test]
fn memory_types_have_their_numbers_and_suit_one_kind_of_memory() {
    use MemoryType::*;
    assert_eq!(
        [WriteBack as u8, Uncached as u8, WriteCombining as u8],
        [0, 1, 2]
    );
    assert_eq!(MemoryType::for_memory(0, false), Ok(WriteBack));
    assert_eq!(MemoryType::for_memory(1, true), Ok(Uncached));
    assert_eq!(MemoryType::for_memory(2, true), Ok(WriteCombining));
    for (raw, device) in [(0, true), (1, false), (2, false), (3, false), (3, true)] {
        assert_eq!(MemoryType::for_memory(raw, device), Err(Error::Invalid));
    }
}

#[test]
fn header_flags_have_their_bits() {
    assert_eq!(Header::FLAG_CAPS_DROPPED, 1);
    assert_eq!(Header::FLAG_NOTIFICATION, 2);
    assert_eq!(Header::FLAG_ADD_MARKS, 4);
    assert_eq!(Header::FLAG_BITS, 22);
    let top = Header::new(0, 0, 0, (1 << 22) - 1).unwrap();
    assert_eq!(top.flags(), (1 << 22) - 1);
    assert_eq!(top.raw() >> 63, 1);
}

#[test]
fn sizes_and_limits_have_their_values() {
    assert_eq!((REG_WORDS, MAX_WORDS, MAX_CAPS), (6, 64, 4));
    assert_eq!((SLOT_BYTES, CSPACE_SEGMENT_SLOTS), (64, 128));
    assert_eq!(SLOT_BYTES * CSPACE_SEGMENT_SLOTS as usize, 8192);
    assert_eq!((REMOVAL_STEP, REMOVAL_SLOTS), (32, 128));
    assert_eq!((TREE_DEPTH, LONGEST_WALK), (64, 191));
    assert_eq!((CSPACE_MAX_SLOTS, CSPACE_DIRECTORY_BYTES), (1 << 20, 8));
    assert_eq!((UNTYPED_MIN_BYTES, CLEAR_STEP_BYTES), (4096, 65_536));
    let small = [
        ENDPOINT_BYTES,
        REPLY_BYTES,
        NOTIFY_BYTES,
        TIMER_BYTES,
        IRQ_BYTES,
    ];
    assert_eq!(small, [64; 5]);
    assert_eq!((TASK_BYTES, SCHED_BYTES, THREAD_BYTES), (256, 128, 1024));
    assert_eq!((ASPACE_BYTES, IOSPACE_BYTES), (8192, 8192));
    assert_eq!(
        (TABLE_BYTES, TABLE_ENTRIES, FRAME_MAX_PAGES),
        (4096, 512, 512)
    );
    assert_eq!((IOSPACE_MAX_DEVICES, IOMMU_WAIT_NS), (16, 1_000_000));
}

#[test]
fn fault_values_have_their_numbers() {
    use FaultKind::*;
    let kinds = [
        Page,
        Undefined,
        Arithmetic,
        Protection,
        Debug,
        BadCall,
        NoAddressSpace,
        NoContext,
    ];
    for (number, kind) in kinds.into_iter().enumerate() {
        assert_eq!(kind as usize, number + 1, "{kind:?}");
    }
    assert_eq!(FAULT_WORDS, 5);
    assert_eq!((FAULT_REPLY_RESUME, FAULT_REPLY_REDIRECT), (0, 1));
    let access = [ACCESS_READ, ACCESS_WRITE, ACCESS_EXEC, ACCESS_PRESENT];
    assert_eq!(access, [1, 2, 4, 8]);
}

#[test]
fn scheduling_values_have_their_numbers() {
    assert_eq!(TIME_LIMIT, 1 << 63);
    assert_eq!(
        (SCHED_MIN_BUDGET_NS, SCHED_MAX_PERIOD_NS),
        (10_000, 1 << 36)
    );
    let classes = [
        SCHED_CLASS_FRAME,
        SCHED_CLASS_INTERACTIVE,
        SCHED_CLASS_BACKGROUND,
    ];
    assert_eq!(classes, [0, 1, 2]);
    assert_eq!(ADMISSION_WHOLE_CORE, 1 << 32);
    assert_eq!((CORE_WINDOWS, CORE_WINDOW_BITS), (1024, 64));
    assert_eq!((TIME_EVENT_STEP, START_SCHED_PERIOD_NS), (16, 10_000_000));
    let states = [
        SCHED_STATE_CONFIGURED,
        SCHED_STATE_THROTTLED,
        SCHED_STATE_HOME,
        SCHED_STATE_LENT,
    ];
    assert_eq!(states, [1, 2, 4, 8]);
}

#[test]
fn bandwidth_is_rounded_up_and_a_whole_core_for_a_full_budget() {
    assert_eq!(bandwidth(0, 0), 0);
    assert_eq!(bandwidth(10_000_000, 10_000_000), ADMISSION_WHOLE_CORE);
    assert_eq!(bandwidth(1, 3), 1_431_655_766);
    assert_eq!(bandwidth(1, 1 << 32), 1);
    assert_eq!(bandwidth(1, (1 << 32) + 1), 1);
}

#[test]
fn firing_instant_has_the_most_trailing_zeros_in_its_window() {
    assert_eq!(firing_instant(0, 100), 0);
    assert_eq!(firing_instant(1000, 0), 1000);
    assert_eq!(firing_instant(1000, 24), 1024);
    assert_eq!(firing_instant(1000, 23), 1008);
    assert_eq!(firing_instant(1025, 1000), 1536);
    assert_eq!(firing_instant(5, u64::MAX), 1 << 62);
    assert_eq!(firing_instant(TIME_LIMIT - 1, 9), TIME_LIMIT - 1);
}

#[test]
fn clock_page_is_40_bytes_and_counts_from_its_base() {
    assert_eq!(size_of::<ClockPage>(), 40);
    let page = ClockPage {
        version: 1,
        seq: 2,
        shift: 1,
        flags: 0,
        mult: 3,
        counter_base: 100,
        ns_base: 7,
    };
    assert_eq!(page.nanoseconds(110), 7 + 15);
    let wide = ClockPage {
        mult: u64::MAX,
        shift: 32,
        ..page
    };
    assert_eq!(wide.nanoseconds(100 + (1 << 32)), 6);
}

#[test]
fn irq_source_follows_the_table_of_the_source_word() {
    let msi = IrqSource::msi(0x1234, 0x56, 31, 7);
    assert_eq!(msi.raw(), 0x1234_56ff_0000_0001);
    assert_eq!(
        (msi.segment(), msi.bus(), msi.device(), msi.function()),
        (0x1234, 0x56, 31, 7)
    );
    let line = IrqSource::line(9, true, true);
    assert_eq!(line.raw(), 9 << 32 | 0x302);
    assert!(line.is_line() && line.level() && line.active_low() && line.id() == 9);
    assert_eq!(IrqSource::from_raw(line.raw()), Ok(line));
    // A kind other than 1 or 2, a reserved bit, bits 8-9 on an MSI.
    for raw in [0, 3, 1 << 10 | 2, 1 << 31 | 1, 0x101, 0x201] {
        assert_eq!(IrqSource::from_raw(raw), Err(Error::Invalid), "{raw:#x}");
    }
}

#[test]
fn task_values_have_their_bits() {
    assert_eq!((TaskFlags::AWARE, TaskFlags::REVOKED), (1, 2));
    let id = TaskId::new(3, 9);
    assert_eq!((id.raw(), id.node(), id.counter()), (3 << 48 | 9, 3, 9));
    assert_eq!(TaskId::NONE.raw(), 0);
    let at = NodeGen::new(5, 7);
    assert_eq!((at.raw(), at.node(), at.generation()), (5 << 32 | 7, 5, 7));
}

#[test]
fn mapping_range_refuses_in_the_order_of_the_contract() {
    let ok = mapping_range(0x20_0000, 1, 511, 512, PageSize::Size4K, 1 << 47);
    assert_eq!(ok, Ok(()));
    let page = PageSize::Size2M;
    assert_eq!(
        mapping_range(0x1000, 0, 1, 1, page, 1 << 47),
        Err(Error::Invalid)
    );
    assert_eq!(
        mapping_range(0, 1, 1, 1, page, 1 << 47),
        Err(Error::Invalid)
    );
    assert_eq!(
        mapping_range(0, 0, 0, 1, page, 1 << 47),
        Err(Error::Invalid)
    );
    assert_eq!(
        mapping_range(0, 0, 2, 1, page, 1 << 47),
        Err(Error::Invalid)
    );
    // The range ends exactly at the limit, or one page beyond it.
    let limit = 4 << 21;
    assert_eq!(mapping_range(2 << 21, 0, 2, 2, page, limit), Ok(()));
    assert_eq!(
        mapping_range(3 << 21, 0, 2, 2, page, limit),
        Err(Error::Invalid)
    );
    let top = u64::MAX - (1 << 21) + 1;
    assert_eq!(
        mapping_range(top, 0, 2, 2, page, u64::MAX),
        Err(Error::Invalid)
    );
}

#[test]
fn mapping_rights_are_read_and_write_or_exec() {
    let (r, w, x) = (Rights::READ, Rights::WRITE, Rights::EXEC);
    assert_eq!(mapping_rights(r.bits()), Ok(r));
    assert_eq!(mapping_rights(r.union(w).bits()), Ok(r.union(w)));
    assert_eq!(mapping_rights(r.union(x).bits()), Ok(r.union(x)));
    for word in [
        0,
        w.bits(),
        r.union(w).union(x).bits(),
        Rights::SEND.bits() | 1,
    ] {
        assert_eq!(mapping_rights(word), Err(Error::Invalid), "{word}");
    }
    assert_eq!(io_mapping_rights(w.bits()), Ok(w));
    for word in [0, x.bits(), r.union(x).bits()] {
        assert_eq!(io_mapping_rights(word), Err(Error::Invalid), "{word}");
    }
}

#[test]
fn x86_64_values_have_their_numbers() {
    use x86_64::*;
    assert_eq!((IRQ_VECTOR_FIRST, IRQ_VECTOR_LAST), (48, 239));
    let claimable = (IRQ_VECTOR_LAST - IRQ_VECTOR_FIRST) as usize + 1;
    assert_eq!((IRQ_TABLE_ENTRIES, claimable), (192, 192));
    assert_eq!(IRQ_MAX_NODES_REMAPPED, 341);
    // The last node that can claim has handles below 65,536.
    assert!(IRQ_MAX_NODES_REMAPPED as usize * IRQ_TABLE_ENTRIES - 1 < 1 << 16);
    assert!((IRQ_MAX_NODES_REMAPPED as usize + 1) * IRQ_TABLE_ENTRIES > 1 << 16);
    assert_eq!(REGS_WORDS, 20);
    let order = [REG_RIP, REG_RSP, REG_RFLAGS, REG_RAX, REG_RBX, REG_RCX];
    assert_eq!(order, [0, 1, 2, 3, 4, 5]);
    assert_eq!(
        [REG_RDX, REG_RSI, REG_RDI, REG_RBP, REG_R8],
        [6, 7, 8, 9, 10]
    );
    assert_eq!([REG_FS_BASE, REG_GS_BASE], [18, 19]);
    assert_eq!(REGS_CANONICAL, [0, 1, 18, 19]);
    assert_eq!((RFLAGS_USER_MASK, RFLAGS_FIXED), (0x0024_0DD5, 0x202));
    assert_eq!(rflags(0), 0x202);
    assert_eq!(rflags(u64::MAX), 0x0024_0FD7);
    assert!(canonical(0x7fff_ffff_ffff) && canonical(0xffff_8000_0000_0000));
    assert!(!canonical(0x8000_0000_0000) && !canonical(0xfff7_ffff_ffff_ffff));
}

#[test]
fn start_up_slots_and_layout_have_their_values() {
    let slots = [
        START_SLOT_CSPACE,
        START_SLOT_ASPACE,
        START_SLOT_THREAD,
        START_SLOT_SCHED,
        START_SLOT_DEBUG,
        START_SLOT_IRQ_CONTROL,
        START_SLOT_IOPORTS,
        START_SLOT_TASK,
        START_SLOT_START_PAGE,
        START_SLOT_MESSAGE_PAGE,
        START_SLOT_MESSAGE_PAGE_MAP,
    ];
    assert_eq!(slots, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    assert_eq!((START_SLOT_FIRST, START_FREE_SLOTS), (16, 4096));
    assert_eq!((START_VERSION, START_PAGE_MAX_PAGES), (1, 63));
    assert_eq!(START_RESERVE_BYTES, 256 * 1024);
    assert_eq!((START_MESSAGE_PAGE_ADDR, START_PAGE_ADDR), (0x1000, 0x2000));
    assert_eq!((FIRST_SERVICE_MIN_ADDR, SHARE_ALIGN), (0x20_0000, 2 << 20));
    // The start-up page and the message page fill the reserve.
    assert_eq!((START_PAGE_MAX_PAGES + 1) * 4096, START_RESERVE_BYTES);
    assert_eq!((START_UNTYPED_DEVICE, START_UNTYPED_IMAGE), (1, 2));
}

#[test]
fn start_up_page_has_its_magic_and_kinds() {
    assert_eq!(START_MAGIC, 0x0074_7353_4f64_6f44);
    use StartKind::*;
    let kinds = [
        ObjectSizes,
        Untyped,
        Cores,
        AcpiRoot,
        UefiMemoryMap,
        Framebuffer,
        EntropySeed,
        BootTimes,
        CrashRecord,
        Process,
        Modules,
        Node,
        Shares,
        Interrupts,
        Kept,
        Memory,
        Iommu,
    ];
    for (number, kind) in kinds.into_iter().enumerate() {
        assert_eq!(kind as usize, number, "{kind:?}");
    }
}

#[test]
fn fields_of_the_shared_layouts_have_their_offsets() {
    use core::mem::offset_of;
    let received = [
        offset_of!(Received, header),
        offset_of!(Received, badge),
        offset_of!(Received, marks),
        offset_of!(Received, task),
    ];
    assert_eq!(received, [0, 8, 16, 24]);
    let header = [
        offset_of!(StartHeader, magic),
        offset_of!(StartHeader, version),
        offset_of!(StartHeader, entries),
    ];
    assert_eq!(header, [0, 8, 12]);
    let entry = [
        offset_of!(StartEntry, kind),
        offset_of!(StartEntry, reserved),
        offset_of!(StartEntry, offset),
        offset_of!(StartEntry, len),
    ];
    assert_eq!(entry, [0, 4, 8, 16]);
    let untyped = [
        offset_of!(StartUntyped, base),
        offset_of!(StartUntyped, slot),
        offset_of!(StartUntyped, log2_size),
        offset_of!(StartUntyped, flags),
        offset_of!(StartUntyped, node),
    ];
    assert_eq!(untyped, [0, 8, 12, 13, 14]);
    let module = [
        offset_of!(StartModule, slot),
        offset_of!(StartModule, flags),
        offset_of!(StartModule, vaddr),
        offset_of!(StartModule, len),
        offset_of!(StartModule, name_offset),
        offset_of!(StartModule, name_len),
    ];
    assert_eq!(module, [0, 4, 8, 16, 24, 28]);
    let process = [
        offset_of!(StartProcess, entry),
        offset_of!(StartProcess, start_page_len),
        offset_of!(StartProcess, slots),
        offset_of!(StartProcess, first_empty),
    ];
    assert_eq!(process, [0, 24, 32, 36]);
    let share = [offset_of!(StartShare, base), offset_of!(StartShare, len)];
    assert_eq!(share, [8, 16]);
    let memory = [
        offset_of!(StartMemory, clock_page),
        offset_of!(StartMemory, page_sizes),
        offset_of!(StartMemory, reserved),
    ];
    assert_eq!(memory, [16, 24, 36]);
    let clock = [
        offset_of!(ClockPage, seq),
        offset_of!(ClockPage, flags),
        offset_of!(ClockPage, mult),
        offset_of!(ClockPage, ns_base),
    ];
    assert_eq!(clock, [4, 12, 16, 32]);
}

#[test]
fn layouts_are_fixed() {
    assert_eq!(size_of::<Received>(), 32);
    assert_eq!(size_of::<StartHeader>(), 16);
    assert_eq!(size_of::<StartEntry>(), 24);
    assert_eq!(size_of::<StartUntyped>(), 16);
    assert_eq!(size_of::<StartCore>(), 24);
    assert_eq!(size_of::<StartModule>(), 32);
    assert_eq!(size_of::<StartProcess>(), 40);
    assert_eq!(size_of::<StartNode>(), 16);
    assert_eq!(size_of::<StartShare>(), 24);
    assert_eq!(size_of::<StartKept>(), 16);
    assert_eq!(size_of::<StartIoApic>(), 8);
    assert_eq!(size_of::<StartInterrupts>(), 16);
    assert_eq!(size_of::<StartFramebuffer>(), 32);
    assert_eq!(size_of::<StartBootTimes>(), 40);
    assert_eq!(size_of::<StartMemory>(), 40);
    assert_eq!(size_of::<StartIommu>(), 16);
    assert_eq!(size_of::<ClockPage>(), 40);
    assert_eq!(PageSize::Size2M.bytes(), 2 * 1024 * 1024);
}

#[test]
fn rights_reject_unknown_bits() {
    assert_eq!(Rights::from_bits(0xff), Some(Rights::ALL));
    assert_eq!(Rights::from_bits(0x100), None);
}

#[test]
fn marks_only_add() {
    let m = Marks::NONE.union(Marks::UNTRUSTED);
    assert!(m.union(Marks::PRIVATE).contains(m));
    assert!(!m.contains(Marks::PRIVATE));
}

#[test]
fn without_removes_only_the_named_rights() {
    let rw = Rights::READ.union(Rights::WRITE);
    assert_eq!(rw.without(Rights::WRITE), Rights::READ);
    assert_eq!(rw.without(Rights::EXEC), rw);
}

#[test]
fn marks_reject_reserved_bits() {
    assert_eq!(
        Marks::from_bits(0b11),
        Ok(Marks::PRIVATE.union(Marks::UNTRUSTED))
    );
    assert_eq!(Marks::from_bits(1 << 2), Err(Error::Invalid));
}

#[test]
fn slot_address_splits_into_via_and_index() {
    let a = SlotAddr::through(7, 9);
    assert_eq!((a.via(), a.index()), (7, 9));
    assert_eq!(SlotAddr::own(9).via(), 0);
    assert!(SlotAddr::NONE.is_none());
    assert!(!SlotAddr::own(START_SLOT_CSPACE).is_none());
}

#[test]
fn header_round_trips_at_the_limits() {
    let h = Header::new(u32::MAX, MAX_WORDS, MAX_CAPS, Header::FLAG_CAPS_DROPPED).unwrap();
    assert_eq!(
        (h.label(), h.words(), h.caps(), h.flags()),
        (u32::MAX, MAX_WORDS, MAX_CAPS, Header::FLAG_CAPS_DROPPED)
    );
    assert_eq!(Header::from_raw(h.raw()), Ok(h));
}

#[test]
fn header_rejects_oversized_messages() {
    assert_eq!(Header::new(0, MAX_WORDS + 1, 0, 0), Err(Error::Invalid));
    assert_eq!(Header::new(0, 0, MAX_CAPS + 1, 0), Err(Error::Invalid));
    assert_eq!(Header::new(0, 0, 0, 1 << 22), Err(Error::Invalid));
    // Raw words from user space: 65 words, then 5 capabilities.
    assert_eq!(Header::from_raw(65 << 32), Err(Error::Invalid));
    assert_eq!(Header::from_raw(5 << 39), Err(Error::Invalid));
}

#[test]
fn fast_path_is_registers_without_capabilities_or_flags() {
    assert!(Header::new(1, REG_WORDS, 0, 0).unwrap().is_fast());
    assert!(!Header::new(1, REG_WORDS + 1, 0, 0).unwrap().is_fast());
    assert!(!Header::new(1, 0, 1, 0).unwrap().is_fast());
    assert!(
        !Header::new(1, 0, 0, Header::FLAG_ADD_MARKS)
            .unwrap()
            .is_fast()
    );
}
