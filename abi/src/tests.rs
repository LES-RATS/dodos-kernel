extern crate std;
use super::*;
use std::vec::Vec;

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
        NodeStart,
        NodeStop,
        NodeStatus,
        DebugWrite,
        TraceEmit,
        ThreadCancel,
        IrqClaim,
        SchedRead,
        MemoryTake,
        MemoryGive,
        MemorySplit,
        NodeIdle,
        NodeWatch,
        NodeSend,
        RoleClaim,
        RoleHand,
        ThreadSetFeatures,
        GrantSignal,
        SchedCount,
        IoportIn,
        IoportOut,
        IoportsIssue,
        EntropyRead,
        NodePerf,
    ];
    assert_eq!(calls.len(), 64);
    for (number, call) in calls.into_iter().enumerate() {
        assert_eq!(call as usize, number, "{call:?}");
    }
}

#[test]
fn object_kinds_and_page_sizes_have_their_numbers() {
    use ObjectKind::*;
    let kinds = [
        Untyped,
        CSpace,
        ASpace,
        Frame,
        Thread,
        Sched,
        Endpoint,
        Reply,
        Notify,
        Timer,
        Irq,
        IoPorts,
        IoSpace,
        Task,
        Node,
        Debug,
        Vcpu,
        IrqControl,
        Iommu,
        MemoryControl,
        Role,
        ItsTable,
    ];
    assert_eq!(kinds, ObjectKind::ALL);
    // D-153, gap 39: `its_table` takes the next free kind, the others'
    // numbers pinned.
    assert_eq!(ItsTable as u32, 21);
    assert_eq!(IrqControl as u32, 17);
    for (number, kind) in kinds.into_iter().enumerate() {
        assert_eq!(kind as usize, number, "{kind:?}");
    }
    for (number, page) in PageSize::ALL.into_iter().enumerate() {
        assert_eq!(page as usize, number, "{page:?}");
        assert_eq!(PageSize::from_raw(number as u64), Some(page));
    }
    assert_eq!(PageSize::from_raw(4), None);
}

#[test]
fn the_geometry_reads_page_sizes_from_the_machine() {
    let x86 = x86_64::GEOMETRY;
    assert!(x86.valid());
    assert_eq!(x86.page(), 4096);
    assert_eq!(x86.bytes(PageSize::Large1), Some(2 << 20));
    assert_eq!(x86.bytes(PageSize::Large2), Some(1 << 30));
    assert_eq!(x86.bytes(PageSize::Large3), None);
    assert_eq!(
        (x86.chunk(), x86.chunk_shift(), x86.sizes()),
        (2 << 20, 21, 0b111)
    );
    let sixteen = Geometry {
        page_shift: [14, 25, 36, 0],
        entries: 2048,
        levels: 3,
        user_end: (1 << 46) - (1 << 36),
    };
    assert!(sixteen.valid());
    assert_eq!((sixteen.page(), sixteen.chunk()), (16 << 10, 32 << 20));
    assert_eq!((x86.address_bits(), sixteen.address_bits()), (48, 47));
    // The largest E the contract accepts, on pages of 32 KB; and two levels.
    let thirty_two = Geometry {
        page_shift: [15, 27, 39, 0],
        entries: TABLE_ENTRIES_MAX,
        levels: 3,
        user_end: (1 << 50) - (1 << 39),
    };
    assert!(thirty_two.valid());
    let two_levels = Geometry {
        page_shift: [12, 21, 0, 0],
        levels: LEVELS_MIN,
        user_end: (1 << 29) - (1 << 21),
        ..x86
    };
    assert!(two_levels.valid());
    let bad = [
        Geometry {
            page_shift: [11, 21, 0, 0],
            ..x86
        },
        Geometry {
            page_shift: [12, 12, 0, 0],
            ..x86
        },
        Geometry {
            page_shift: [12, 21, 0, 30],
            ..x86
        },
        Geometry {
            page_shift: [12, 21, 20, 0],
            ..x86
        },
        Geometry {
            entries: 8192,
            ..x86
        },
        Geometry {
            entries: 500,
            ..x86
        },
        Geometry { levels: 1, ..x86 },
        Geometry {
            user_end: 4096,
            ..x86
        },
        // Mistral F4: a 64 KB granule holds 8,192 entries per table, more
        // than the contract accepts, whatever E is stated; and E must be
        // the entries of one page (2 on a 4 KB page is not).
        Geometry {
            page_shift: [16, 29, 0, 0],
            entries: 4096,
            levels: 3,
            user_end: 63 << 42,
        },
        Geometry {
            page_shift: [16, 29, 0, 0],
            entries: 8192,
            levels: 3,
            user_end: 63 << 42,
        },
        Geometry {
            entries: 2,
            user_end: 1 << 30,
            ..x86
        },
        // DeepSeek F6: three levels of 16 KB translate 47 bits, so a user
        // range up to 2^47 lies outside the lower half; no second page size.
        Geometry {
            user_end: (1 << 47) - (1 << 36),
            ..sixteen
        },
        Geometry {
            page_shift: [12, 0, 0, 0],
            ..x86
        },
    ];
    for geometry in bad {
        assert!(!geometry.valid(), "{geometry:?}");
    }
}

/// DeepSeek F6: the canonical rule follows the geometry: four levels of
/// 4 KB sign-extend bit 47, three levels of 16 KB bit 46.
#[test]
fn canonical_addresses_follow_the_address_bits_of_the_geometry() {
    let x86 = x86_64::GEOMETRY;
    let sixteen = Geometry {
        page_shift: [14, 25, 36, 0],
        entries: 2048,
        levels: 3,
        user_end: (1 << 46) - (1 << 36),
    };
    for (geometry, bit) in [(x86, 47), (sixteen, 46)] {
        assert!(geometry.canonical(0));
        assert!(geometry.canonical((1 << bit) - 1));
        assert!(!geometry.canonical(1 << bit));
        assert!(!geometry.canonical(u64::MAX << bit >> 1 & !(u64::MAX << 63)));
        assert!(geometry.canonical(u64::MAX << bit));
        assert!(!geometry.canonical((u64::MAX << bit) - 1));
        assert!(geometry.canonical(u64::MAX));
    }
    // The reference value of the architecture layer agrees.
    for address in [0, 1 << 46, (1 << 47) - 1, 1 << 47, u64::MAX << 47, 1 << 63] {
        assert_eq!(x86.canonical(address), x86_64::canonical(address));
    }
}

/// DeepSeek F1 of the seventh review: the canonical boundary is the
/// architecture's (the address bits of the geometry), not the user range
/// end: a machine whose user range ends at 2^(bits − 2) still accepts every
/// address of both halves.
#[test]
fn the_canonical_boundary_follows_the_address_bits_and_not_the_user_range_end() {
    let x86 = x86_64::GEOMETRY;
    let sixteen = Geometry {
        page_shift: [14, 25, 36, 0],
        entries: 2048,
        levels: 3,
        user_end: (1 << 46) - (1 << 36),
    };
    for g in [x86, sixteen] {
        let bits = g.address_bits();
        let low = Geometry {
            user_end: 1 << (bits - 2),
            ..g
        };
        assert!(low.valid());
        for address in [(1 << (bits - 1)) - 0x1000, u64::MAX << (bits - 1), u64::MAX] {
            assert!(low.canonical(address), "{address:#x}");
            assert_eq!(low.canonical(address), g.canonical(address));
        }
        assert!(!low.canonical(1 << (bits - 1)));
        assert!(!low.canonical(1 << 63));
    }
    for address in [(1 << 47) - 1, 0xffff_8000_0000_0000, 1 << 47, 1 << 46] {
        let low = Geometry {
            user_end: 1 << 46,
            ..x86
        };
        assert_eq!(low.canonical(address), x86_64::canonical(address));
    }
}

/// Hydrogen N1 and Opus N1 of the seventh review: the user range leaves the
/// whole root entry of the reserved slot below the half; neither the half
/// nor the half less one chunk is a user range end.
#[test]
fn the_user_range_end_leaves_the_root_entry_of_the_reserved_slot() {
    for g in [
        x86_64::GEOMETRY,
        Geometry {
            page_shift: [14, 25, 36, 0],
            entries: 2048,
            levels: 3,
            user_end: (1 << 46) - (1 << 36),
        },
    ] {
        let half = 1u64 << (g.address_bits() - 1);
        assert_eq!(g.user_end, half - g.root_entry());
        assert!(g.valid());
        for user_end in [half, half - g.chunk(), half - g.root_entry() + g.chunk()] {
            assert!(!Geometry { user_end, ..g }.valid(), "{user_end:#x}");
        }
    }
    assert_eq!(x86_64::GEOMETRY.root_entry(), 512 << 30);
}

/// Hydrogen N5 and DeepSeek F10 of the seventh review: page sizes are one
/// per level (each log2 E above the one before), and none is larger than
/// E chunks, so that one leaf fits one call of the gate (G3).
#[test]
fn page_sizes_are_one_per_level_and_at_most_e_chunks() {
    let one_per_level = Geometry {
        page_shift: [12, 18, 28, 0],
        entries: 512,
        levels: 3,
        user_end: (1 << 38) - (1 << 30),
    };
    assert!(!one_per_level.valid());
    let chunk_off = Geometry {
        page_shift: [12, 20, 0, 0],
        ..x86_64::GEOMETRY
    };
    assert!(!chunk_off.valid());
    // A third large page of 512 GB: 2^18 chunks in one leaf.
    let large3 = Geometry {
        page_shift: [12, 21, 30, 39],
        ..x86_64::GEOMETRY
    };
    assert!(!large3.valid());
    // A large page whose leaf would be the root's on two levels.
    let two_levels = Geometry {
        page_shift: [12, 21, 30, 0],
        levels: 2,
        user_end: (1 << 29) - (1 << 21),
        ..x86_64::GEOMETRY
    };
    assert!(!two_levels.valid());
    assert!(
        Geometry {
            page_shift: [12, 21, 0, 0],
            ..two_levels
        }
        .valid()
    );
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
    assert_eq!((CSPACE_MAX_SLOTS, CSPACE_DIRECTORY_BYTES), (1 << 20, 16));
    assert_eq!(CLEAR_STEP_BYTES, 65_536);
    let small = [
        ENDPOINT_BYTES,
        REPLY_BYTES,
        NOTIFY_BYTES,
        TIMER_BYTES,
        IRQ_BYTES,
    ];
    assert_eq!(small, [64; 5]);
    assert_eq!(
        (TASK_BYTES, SCHED_BYTES, THREAD_FIXED_BYTES),
        (256, 256, 1024)
    );
    assert_eq!((ASPACE_PAGES, IOSPACE_PAGES), (2, 2));
    assert_eq!((TABLE_ENTRIES_MAX, LEVELS_MIN), (4096, 2));
    assert_eq!((IOSPACE_MAX_DEVICES, IOMMU_WAIT_NS), (16, 10_000_000));
    assert_eq!(ENTROPY_MAX_BYTES, 64);
}

#[test]
fn numbers_of_the_multi_node_part_have_their_values() {
    assert_eq!((NODES_MAX, GRANTS), (1024, 4096));
    assert_eq!((GRANT_ENTRY_BYTES, USE_ENTRY_BYTES), (128, 32));
    assert_eq!(DEVICE_TABLE_SPARE, 8192);
    assert_eq!((RESERVES_MIN, RESERVES_MAX, ROLES_PLAIN_MAX), (2, 8, 64));
    assert_eq!(
        (NODE_WATCH_SLOTS, IDLE_STATES_MAX, OBSERVATIONS_MAX),
        (4, 15, 8)
    );
    assert_eq!(IDLE_ENTRY_WORDS * IDLE_STATES_MAX, 60);
    assert_eq!((STALE_NS, STUCK_NS), (100_000_000, 100_000_000));
    assert_eq!(STALE_RANGE_NS, (10_000_000, 1_000_000_000));
    assert_eq!(
        (SWEEP_NS, SWEEP_RANGE_NS),
        (1_000_000_000, (100_000_000, 60_000_000_000))
    );
    let fence = [NMI_GRACE_NS, FENCE_NS, SETTLE_NS, START_NS];
    // D-141: two rounds of a fence (11.2 ms each) and their stub; the range
    // from those two rounds and the stub since D-142.
    assert_eq!(fence, [1_000_000, 30_000_000, 1_000_000, 1_000_000_000]);
    assert_eq!(FENCE_RANGE_NS, (23_000_000, 1_000_000_000));
    assert!(FENCE_RANGE_NS.0 > 2 * 11_200_000);
    assert_eq!(
        (SEAL_NS, SEAL_RANGE_NS),
        (1_000_000_000, (10_000_000, 60_000_000_000))
    );
    assert_eq!(
        (CORE_ANSWER_NS, CORE_ANSWER_RANGE_NS),
        (10_000_000, (1_000_000, 100_000_000))
    );
    assert_eq!(CORES_PER_KERNEL_MAX, 64);
    assert_eq!(
        (RECOVERY_BANDWIDTH, RECOVERY_PERIOD_NS),
        (1 << 26, 10_000_000)
    );
    assert_eq!(OBSERVATION_LIFE, 4);
    // The first context and the recovery reserve fill one core.
    let first = bandwidth(START_SCHED_BUDGET_NS, START_SCHED_PERIOD_NS);
    assert_eq!(first + RECOVERY_BANDWIDTH, ADMISSION_WHOLE_CORE);
    assert_eq!(
        bandwidth(RECOVERY_PERIOD_NS / 64, RECOVERY_PERIOD_NS),
        RECOVERY_BANDWIDTH
    );
}

#[test]
fn values_of_the_nodes_have_their_numbers() {
    assert_eq!((NODE_START_FRESH, NODE_START_RESUME), (0, 1));
    assert_eq!((NODE_IMAGE_BOOT, NODE_IMAGE_REPLACEMENT), (0, 1));
    assert_eq!(
        (NODE_STOP_KILL, NODE_STOP_SEAL, NODE_STOP_RETIRE),
        (0, 1, 2)
    );
    assert_eq!(
        (NODE_STATUS_CORES_MASK, NODE_STATUS_CLASS_SHIFT),
        (0xFFFF, 16)
    );
    assert_eq!((NODE_STATUS_UNUSABLE, NODE_STATUS_WORDS), (1 << 18, 7));
    let methods = [
        IDLE_METHOD_HALT,
        IDLE_METHOD_MWAIT,
        IDLE_METHOD_PSCI,
        IDLE_METHOD_SBI,
    ];
    assert_eq!(methods, [0, 1, 2, 3]);
    assert_eq!(IDLE_FLAG_LOSES_CONTEXT, 1);
    let classes = [
        FENCE_CLASS_COOPERATIVE,
        FENCE_CLASS_SIGNALLED,
        FENCE_CLASS_FORCED,
    ];
    assert_eq!(classes, [0, 1, 2]);
    assert_eq!(CORE_EVENT_FEEDBACK, 1);
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
        HardwareBreakpoint,
    ];
    for (number, kind) in kinds.into_iter().enumerate() {
        assert_eq!(kind as usize, number + 1, "{kind:?}");
        assert_eq!(FaultKind::from_raw(number as u32 + 1), Some(kind));
    }
    assert_eq!(
        (FaultKind::from_raw(0), FaultKind::from_raw(10)),
        (None, None)
    );
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
    assert_eq!(START_SCHED_BUDGET_NS, 9_843_750);
    assert_eq!(SCHED_STATE_COUNTED, 1 << 4);
    assert_eq!(SCHED_STATE_HW_CLASS_SHIFT, 8);
    assert_eq!(SCHED_STATE_HW_CLASS_MASK, 0xFF00);
    assert_eq!((SCHED_COUNT_OFF, SCHED_COUNT_ON), (0, 1));
    let states = [
        SCHED_STATE_CONFIGURED,
        SCHED_STATE_THROTTLED,
        SCHED_STATE_HOME,
        SCHED_STATE_LENT,
    ];
    assert_eq!(states, [1, 2, 4, 8]);
}

#[test]
fn bandwidth_is_the_rounded_up_quotient_of_128_bits() {
    // xorshift64*, fixed seed: many budgets and periods against the
    // definition computed in 128 bits.
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..100_000 {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let r = x.wrapping_mul(0x2545_f491_4f6c_dd1d);
        let period = 1 + (r >> 16) % (1 << 40);
        let budget = (r ^ r >> 7) % (period + 1);
        let wanted = ((u128::from(budget) << 32).div_ceil(u128::from(period))) as u64;
        assert_eq!(bandwidth(budget, period), wanted, "{budget} {period}");
    }
}

#[test]
fn bandwidth_is_rounded_up_and_a_whole_core_for_a_full_budget() {
    assert_eq!(bandwidth(0, 0), 0);
    assert_eq!(bandwidth(10_000_000, 10_000_000), ADMISSION_WHOLE_CORE);
    assert_eq!(bandwidth(1, 3), 1_431_655_766);
    assert_eq!(bandwidth(1, 1 << 32), 1);
    assert_eq!(bandwidth(1, (1 << 32) + 1), 1);
    assert_eq!(bandwidth((1 << 47) - 1, (1 << 48) - 1), 1 << 31);
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
    // A kind other than 1 to 3, a reserved bit, bits 8-9 on an MSI, a bit
    // beyond the event on a core event.
    for raw in [
        0,
        4,
        1 << 10 | 2,
        1 << 31 | 1,
        0x101,
        0x201,
        1 << 16 | 3,
        1 << 40 | 3,
    ] {
        assert_eq!(IrqSource::from_raw(raw), Err(Error::Invalid), "{raw:#x}");
    }
    let event = IrqSource::core_event(CORE_EVENT_FEEDBACK);
    assert_eq!(event.raw(), 0x103);
    assert_eq!(IrqSource::from_raw(0x103), Ok(event));
    assert!(event.is_core_event() && event.event() == 1 && !event.level() && !event.is_line());
    assert_eq!(IrqSource::KIND_CORE_EVENT, 3);
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
    let ok = mapping_range(0x20_0000, 1, 511, 512, 4096, 1 << 47);
    assert_eq!(ok, Ok(()));
    let page = 1 << 21;
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
    assert_eq!(IRQ_MAX_CORES_REMAPPED, 341);
    // The last node that can claim has handles below 65,536.
    assert!(IRQ_MAX_CORES_REMAPPED as usize * IRQ_TABLE_ENTRIES - 1 < 1 << 16);
    assert!((IRQ_MAX_CORES_REMAPPED as usize + 1) * IRQ_TABLE_ENTRIES > 1 << 16);
    assert_eq!(REGS_WORDS, 26);
    let order = [REG_RIP, REG_RSP, REG_RFLAGS, REG_RAX, REG_RBX, REG_RCX];
    assert_eq!(order, [0, 1, 2, 3, 4, 5]);
    assert_eq!(
        [REG_RDX, REG_RSI, REG_RDI, REG_RBP, REG_R8],
        [6, 7, 8, 9, 10]
    );
    assert_eq!([REG_FS_BASE, REG_GS_BASE], [18, 19]);
    let debug = [REG_DR0, REG_DR1, REG_DR2, REG_DR3, REG_DR7, REG_UINTR];
    assert_eq!(debug, [20, 21, 22, 23, 24, 25]);
    assert_eq!(HW_BREAKPOINTS, 4);
    assert_eq!(REGS_CANONICAL, [0, 1, 18, 19]);
    assert_eq!((RFLAGS_USER_MASK, RFLAGS_FIXED), (0x0024_0DD5, 0x202));
    assert_eq!(rflags(0), 0x202);
    assert_eq!(rflags(u64::MAX), 0x0024_0FD7);
    assert!(canonical(0x7fff_ffff_ffff) && canonical(0xffff_8000_0000_0000));
    assert!(!canonical(0x8000_0000_0000) && !canonical(0xfff7_ffff_ffff_ffff));
    assert_eq!(
        (PAGE, LARGE, TABLE_ENTRIES, LEVELS),
        (4096, [2 << 20, 1 << 30], 512, 4)
    );
    assert_eq!(USER_RANGE_END, 255 << 39);
    assert_eq!(
        (ASPACE_BYTES, IOSPACE_BYTES, TABLE_BYTES),
        (8192, 8192, 4096)
    );
    assert_eq!((UNTYPED_MIN_BYTES, CHUNK_BYTES), (4096, 2 << 20));
    assert_eq!((START_MESSAGE_PAGE_ADDR, START_PAGE_ADDR), (0x1000, 0x2000));
    assert_eq!(FIRST_SERVICE_MIN_ADDR, 0x20_0000);
    assert_eq!(XSTATE_X87_SSE_AVX_BYTES, 832);
    assert_eq!((AVX, AVX512, AMX, APX), (0, 1, 2, 3));
    assert_eq!(
        (FEATURES.state, FEATURES.with_length, FEATURES.large),
        (0b1111, 0, 0b100)
    );
    assert_eq!(KERNEL_PORTS, [(0x20, 0x21), (0xA0, 0xA1)]);
    assert_eq!((PCI_CONFIG_PORTS, UART_PORTS), ((0xCF8, 0xCFF), 8));
}

#[test]
fn the_kinds_of_a_report_key_are_the_contracts_numbers() {
    let kinds = [
        REPORT_KIND_DEVICE,
        REPORT_KIND_HANDLE,
        REPORT_KIND_LINE,
        REPORT_KIND_ITS_DEVICE,
        REPORT_KIND_ITS_EVENT,
    ];
    assert_eq!(kinds, [0, 1, 2, 3, 4]);
    assert_eq!(REPORT_KIND_SHIFT, 61);
    // Every kind fits bits 61-63, and the largest key of each is below the
    // next kind's first and below `REPORT_END`.
    for kind in kinds {
        let last = kind << REPORT_KIND_SHIFT | ((1 << REPORT_KIND_SHIFT) - 1);
        assert_eq!(last >> REPORT_KIND_SHIFT, kind);
        assert!(last < (kind + 1) << REPORT_KIND_SHIFT && last < REPORT_END);
    }
}

#[test]
fn a_vtd_unit_keeps_1_mb_and_4_kb_of_tables() {
    assert_eq!(x86_64::VTD_UNIT_TABLE_BYTES, (1 << 20) + 4096);
}

#[test]
fn the_arm64_breakpoint_control_word_has_its_user_mask() {
    use aarch64::{DBGBCR_USER_MASK, dbgbcr_valid};
    assert_eq!(DBGBCR_USER_MASK, 0x1E7);
    // Disabled, and an enabled A64 address match at EL0 only.
    assert!(dbgbcr_valid(0) && dbgbcr_valid(0x1E4) && dbgbcr_valid(0x1E5));
    // PMC other than 0b10 (EL1, both, none); BAS for T32 or none.
    for word in [0x1E3, 0x1E7, 0x1E1, 0x065, 0x185, 0x005] {
        assert!(!dbgbcr_valid(word), "{word:#x}");
    }
    // BT2 (3), bit 4, the reserved bits 12:9, HMC (13), SSC (14, 15), LBN
    // (16), BT (20, a linked or context match), MASK (24), SSCE (29),
    // LBNX (30), bit 32.
    for bit in [3, 4, 9, 12, 13, 14, 15, 16, 20, 21, 24, 29, 30, 32, 63] {
        assert!(!dbgbcr_valid(0x1E5 | 1 << bit), "bit {bit}");
    }
}

#[test]
fn the_arm64_watchpoint_control_word_has_its_user_mask() {
    use aarch64::{DBGWCR_USER_MASK, dbgwcr_valid};
    assert_eq!(DBGWCR_USER_MASK, 0x1F00_1FFF);
    // PAC 0b10 (0x4), LSC load and store (0x18), BAS one byte (0x20) or
    // all eight (0x1FE0), MASK 3 or 31.
    let good = 0x4 | 0x18 | 0x20 | 1;
    assert!(dbgwcr_valid(0) && dbgwcr_valid(good));
    assert!(dbgwcr_valid(0x4 | 0x8 | 0x1FE0) && dbgwcr_valid(0x4 | 0x10 | 0x60));
    assert!(dbgwcr_valid(good | 3 << 24) && dbgwcr_valid(good | 31 << 24));
    // PAC 0b01 or 0b11; LSC 0b00; BAS 0 or not contiguous; MASK 1 or 2.
    for word in [
        0x2 | 0x18 | 0x20,
        0x6 | 0x18 | 0x20,
        0x4 | 0x20,
        0x4 | 0x18,
        0x4 | 0x18 | 0xA0,
        good | 1 << 24,
        good | 2 << 24,
    ] {
        assert!(!dbgwcr_valid(word), "{word:#x}");
    }
    // HMC (13), SSC (14, 15), LBN (16), WT (20, linked), bit 21, WT2 (22,
    // a mismatch), bit 23, SSCE (29), LBNX (30), bit 32.
    for bit in [13, 14, 15, 16, 20, 21, 22, 23, 29, 30, 32, 63] {
        assert!(!dbgwcr_valid(good | 1 << bit), "bit {bit}");
    }
}

#[test]
fn the_breakpoint_control_word_has_its_user_mask() {
    use x86_64::{DR7_USER_MASK, dr7_valid};
    assert_eq!(DR7_USER_MASK, 0xFFFF_0055);
    // The local enables, and an execute breakpoint and data watchpoints.
    assert!(dr7_valid(0) && dr7_valid(0x55) && dr7_valid(0b0001_1101 << 16 | 1));
    // Global enables, LE, GE, GD, a reserved bit: outside the mask.
    for bit in [1, 3, 5, 7, 8, 9, 10, 11, 13, 32] {
        assert!(!dr7_valid(1 << bit), "bit {bit}");
    }
    // The I/O condition in any of the four fields.
    for n in 0..4 {
        assert!(!dr7_valid(0b10 << (16 + 4 * n)), "field {n}");
    }
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
        START_SLOT_MEMORY_CONTROL,
        START_SLOT_MACHINE_LIVING,
        START_SLOT_MACHINE_FIXED,
        START_SLOT_MAILBOX,
    ];
    assert_eq!(slots, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
    assert_eq!((START_SLOT_FIRST, START_FREE_SLOTS), (16, 4096));
    assert_eq!((START_VERSION, START_PAGE_MAX_PAGES), (3, 63));
    assert_eq!(START_UNTYPED_DEVICE, 1);
    assert_eq!((START_CORE_BOOTED, START_CORE_FENCE_SHIFT), (1, 1));
    let module = [
        START_MODULE_MAPPED,
        START_MODULE_READ,
        START_MODULE_WRITE,
        START_MODULE_EXEC,
        START_MODULE_FIRST_SERVICE,
    ];
    assert_eq!(module, [1, 2, 4, 8, 16]);
    assert_eq!((START_NODE_FRESH, START_NODE_RESUMED), (1, 2));
    let irq = [
        START_IRQ_REMAPPING,
        START_IRQ_X2APIC,
        START_IRQ_DIRECTED_EOI,
    ];
    assert_eq!(irq, [1, 2, 4]);
    let memory = [
        START_MEMORY_MAP_THROUGH_ROLE,
        START_MEMORY_GATE_NO_WINDOW,
        START_MEMORY_CONTROL_FLOW,
        START_MEMORY_MONITOR,
    ];
    assert_eq!(memory, [1, 2, 4, 8]);
}

#[test]
fn start_up_page_has_its_magic_and_kinds() {
    assert_eq!(START_MAGIC, 0x0074_7353_4f64_6f44);
    use StartKind::*;
    let kinds = [
        ObjectSizes,
        Untyped,
        Cores,
        FirmwareTables,
        MemoryRanges,
        Framebuffer,
        EntropySeed,
        BootTimes,
        CrashRecord,
        Process,
        Modules,
        Node,
        Floors,
        Interrupts,
        Kept,
        Memory,
        Iommu,
        Machine,
        Roles,
        Reserves,
        Image,
        Kinds,
        Features,
        Feedback,
        Debug,
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
        offset_of!(StartModule, first_slot),
        offset_of!(StartModule, slots),
        offset_of!(StartModule, vaddr),
        offset_of!(StartModule, len),
        offset_of!(StartModule, flags),
        offset_of!(StartModule, name_offset),
        offset_of!(StartModule, name_len),
        offset_of!(StartModule, reserved),
    ];
    assert_eq!(module, [0, 4, 8, 16, 24, 28, 32, 36]);
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
        offset_of!(StartMemory, page_shift),
        offset_of!(StartMemory, table_entries),
        offset_of!(StartMemory, levels),
        offset_of!(StartMemory, iospace_width),
        offset_of!(StartMemory, iospace_page_sizes),
        offset_of!(StartMemory, flags),
        offset_of!(StartMemory, reserved),
    ];
    assert_eq!(memory, [16, 24, 28, 32, 36, 40, 44, 48]);
    let core = [
        offset_of!(StartCore, slot),
        offset_of!(StartCore, kind),
        offset_of!(StartCore, flags),
        offset_of!(StartCore, threads),
        offset_of!(StartCore, efficiency_class),
        offset_of!(StartCore, package),
        offset_of!(StartCore, reserved),
    ];
    assert_eq!(core, [8, 12, 14, 16, 19, 20, 24]);
    let machine = [
        offset_of!(StartMachine, uses),
        offset_of!(StartMachine, chunks),
        offset_of!(StartMachine, device_capacity),
        offset_of!(StartMachine, layout),
        offset_of!(StartMachine, identifiers),
        offset_of!(StartMachine, device_leaves),
        offset_of!(StartMachine, core_words),
        offset_of!(StartMachine, park_requests),
        offset_of!(StartMachine, start_requests),
        offset_of!(StartMachine, fence_records),
        offset_of!(StartMachine, lines),
        offset_of!(StartMachine, line_count),
        offset_of!(StartMachine, attach_words),
        offset_of!(StartMachine, retired_words),
        offset_of!(StartMachine, domain_words),
        offset_of!(StartMachine, its_words),
        offset_of!(StartMachine, request_records),
        offset_of!(StartMachine, r1_words),
        offset_of!(StartMachine, identifier_bitmaps),
        offset_of!(StartMachine, line_bitmaps),
        offset_of!(StartMachine, seal_parked),
        offset_of!(StartMachine, suspend_word),
    ];
    assert_eq!(
        machine,
        [
            28, 32, 40, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92, 96, 100, 104, 108, 112, 116,
            120, 124
        ]
    );
    // D-152: 128 bytes, layout 6.
    assert_eq!(size_of::<StartMachine>(), 128);
    assert_eq!(START_MACHINE_LAYOUT, 6);
    assert_eq!(RETIRED_WORD_BYTES, 24);
    let node = [
        offset_of!(StartNode, slot),
        offset_of!(StartNode, slots),
        offset_of!(StartNode, generation),
        offset_of!(StartNode, flags),
        offset_of!(StartNode, cores),
        offset_of!(StartNode, cores_supported),
    ];
    assert_eq!(node, [0, 4, 8, 12, 16, 20]);
    let membership = [
        offset_of!(BootMembership, sweep_ms),
        offset_of!(BootMembership, seal_ms),
        offset_of!(BootMembership, core_answer_ms),
        offset_of!(BootMembership, flags),
    ];
    assert_eq!(membership, [8, 12, 16, 20]);
    // D-149: bit 0 the bootstrap core's cleared BSP flag declared honoured,
    // the only flag.
    assert_eq!(BOOT_MEMBERSHIP_BSP_CLEARED, 1);
    assert_eq!(BOOT_MEMBERSHIP_FLAGS, 1);
    let ring = [
        offset_of!(RingHeader, kind),
        offset_of!(RingHeader, length),
        offset_of!(RingHeader, sender),
        offset_of!(RingHeader, sender_generation),
        offset_of!(RingHeader, receiver_generation),
        offset_of!(RingHeader, credit),
    ];
    assert_eq!(ring, [0, 1, 2, 4, 8, 12]);
    let kind = [
        offset_of!(StartKindRecord, baseline),
        offset_of!(StartKindRecord, id),
        offset_of!(StartKindRecord, cores),
        offset_of!(StartKindRecord, baseline_bytes),
    ];
    assert_eq!(kind, [8, 16, 40, 48]);
    let debug = [
        offset_of!(StartDebug, console_address),
        offset_of!(StartDebug, crash_base),
        offset_of!(StartDebug, slot),
        offset_of!(StartDebug, kernel_entries),
        offset_of!(StartDebug, reserved),
    ];
    assert_eq!(debug, [8, 16, 32, 40, 50]);
    let header = [
        offset_of!(DebugAreaHeader, core),
        offset_of!(DebugAreaHeader, generation),
        offset_of!(DebugAreaHeader, slot),
        offset_of!(DebugAreaHeader, next_kernel),
        offset_of!(DebugAreaHeader, next_user),
        offset_of!(DebugAreaHeader, suppressed),
    ];
    assert_eq!(header, [6, 8, 12, 16, 24, 32]);
    // The core record sits at offset 64 of the core's debug area.
    let record = [
        offset_of!(CoreCrashRecord, generation),
        offset_of!(CoreCrashRecord, instant),
        offset_of!(CoreCrashRecord, member),
        offset_of!(CoreCrashRecord, detail),
        offset_of!(CoreCrashRecord, text),
        offset_of!(CoreCrashRecord, kernel),
        offset_of!(CoreCrashRecord, user),
        offset_of!(CoreCrashRecord, reserved),
    ];
    let at = record.map(|offset| offset + DEBUG_HEADER_BYTES);
    assert_eq!(at, [72, 80, 88, 96, 352, 480, 2528, 3552]);
    let crash = [
        offset_of!(CrashAreaHeader, cores),
        offset_of!(CrashAreaHeader, boot_id),
        offset_of!(CrashAreaHeader, sequence),
        offset_of!(CrashAreaHeader, area_bytes),
        offset_of!(CrashAreaHeader, build),
        offset_of!(CrashAreaHeader, stop),
    ];
    assert_eq!(crash, [12, 16, 24, 32, 36, 48]);
    let floors = [
        offset_of!(BootFloors, floor_tables),
        offset_of!(BootFloors, reserve_bytes),
        offset_of!(BootFloors, reserve_per_gib),
        offset_of!(BootFloors, reserves),
        offset_of!(BootFloors, plain_roles),
    ];
    // D-154: `floor_tables` after `floor_node`, as the contract lists them.
    assert_eq!(floors, [8, 16, 24, 32, 36]);
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
    assert_eq!(size_of::<FenceRecord>(), FENCE_RECORD_BYTES);
    assert_eq!(FENCE_RECORD_BYTES, 544);
    assert_eq!(size_of::<StartEntry>(), 24);
    assert_eq!(size_of::<StartUntyped>(), 16);
    assert_eq!(size_of::<StartCore>(), 32);
    assert_eq!(size_of::<StartCrash>(), 32);
    assert_eq!(size_of::<StartModule>(), 40);
    assert_eq!(size_of::<StartProcess>(), 40);
    assert_eq!(size_of::<StartNode>(), 24);
    assert_eq!(size_of::<StartShare>(), 24);
    assert_eq!(size_of::<StartKept>(), 16);
    assert_eq!(size_of::<StartIoApic>(), 8);
    assert_eq!(size_of::<StartInterrupts>(), 16);
    assert_eq!(size_of::<StartFramebuffer>(), 32);
    assert_eq!(size_of::<StartBootTimes>(), 48);
    assert_eq!(size_of::<StartFirmwareTable>(), 24);
    assert_eq!(size_of::<StartMemoryRange>(), 24);
    let stages = [
        START_STAGE_UEFI,
        START_STAGE_DEVICE_TREE,
        START_STAGE_UBIOS,
        START_STAGE_LIMINE,
        START_STAGE_OTHER,
    ];
    assert_eq!(stages, [0, 1, 2, 3, 4]);
    let tables = [
        START_TABLE_ACPI_ROOT,
        START_TABLE_DEVICE_TREE,
        START_TABLE_UBIOS,
        START_TABLE_SMBIOS,
    ];
    assert_eq!(tables, [1, 2, 3, 4]);
    let ranges = [
        START_RANGE_USABLE,
        START_RANGE_KEPT_BY_STAGE,
        START_RANGE_FIRMWARE,
        START_RANGE_UNUSABLE,
        START_RANGE_DEVICE,
    ];
    assert_eq!(ranges, [0, 1, 2, 3, 4]);
    assert_eq!(size_of::<StartMemory>(), 56);
    assert_eq!(size_of::<StartIommu>(), 16);
    assert_eq!(size_of::<StartMachine>(), 128);
    assert_eq!(size_of::<StartRole>(), 8);
    assert_eq!(size_of::<StartImage>(), 48);
    assert_eq!(size_of::<StartKindRecord>(), 56);
    assert_eq!(size_of::<StartFeedback>(), 16);
    assert_eq!(size_of::<StartDebug>(), 56);
    assert_eq!(size_of::<BootFloors>(), 40);
    assert_eq!(size_of::<BootMembership>(), 24);
    assert_eq!(size_of::<BootNodes>(), 8);
    assert_eq!(size_of::<BootNode>(), 16);
    assert_eq!(size_of::<RingHeader>(), RING_HEADER_BYTES);
    assert_eq!(size_of::<CoreWord>(), 8);
    assert_eq!(size_of::<ClockPage>(), 40);
    assert_eq!(size_of::<DeviceEntry>(), 16);
    assert_eq!(size_of::<TraceEntry>(), TRACE_ENTRY_BYTES);
    assert_eq!(size_of::<DebugAreaHeader>(), DEBUG_HEADER_BYTES);
    assert_eq!(size_of::<CoreCrashRecord>(), CORE_CRASH_RECORD_BYTES);
    assert_eq!(size_of::<CrashAreaHeader>(), CRASH_HEADER_BYTES);
    assert_eq!(size_of::<Ownership>(), 8);
    assert_eq!(size_of::<Member>(), 8);
    assert_eq!(size_of::<RoleWord>(), 8);
    assert_eq!(size_of::<FeatureMask>(), 8);
    assert_eq!(size_of::<IoPortRange>(), 8);
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

#[test]
fn slot_layout_has_its_bits() {
    let states = [SLOT_EMPTY, SLOT_FULL, SLOT_DEAD, SLOT_REMOTE];
    assert_eq!((states, SLOT_STATE_MASK), ([0, 1, 2, 3], 3));
    assert_eq!((SLOT_DYING, SLOT_ROOT), (4, 8));
    let shifts = [
        SLOT_KIND_SHIFT,
        SLOT_RIGHTS_SHIFT,
        SLOT_DEPTH_SHIFT,
        SLOT_BY_KIND_SHIFT,
    ];
    assert_eq!(shifts, [4, 10, 18, 25]);
    let frame = [
        SLOT_FRAME_PAGE_SHIFT,
        SLOT_FRAME_MEMORY_SHIFT,
        SLOT_FRAME_PAGES_SHIFT,
        SLOT_FRAME_FIRST_SHIFT,
    ];
    assert_eq!((frame, SLOT_FRAME_DEVICE), ([25, 27, 30, 42], 1 << 29));
    // The number of pages − 1 and the first page have 12 bits: E ≤ 4,096.
    assert_eq!(SLOT_FRAME_FIRST_SHIFT - SLOT_FRAME_PAGES_SHIFT, 12);
    assert_eq!(1 << 12, TABLE_ENTRIES_MAX);
    let record = [
        SLOT_RECORD_EMPTY,
        SLOT_RECORD_MAPPING,
        SLOT_RECORD_BINDING,
        SLOT_RECORD_UNMAPPING,
    ];
    assert_eq!((record, SLOT_RECORD_RIGHTS_SHIFT), ([0, 1, 2, 3], 2));
    // An irq names its core in bits 16-31 of word 1 (D-129).
    assert_eq!(SLOT_IRQ_CORE_SHIFT, 16);
    // A partial mapping: the mark in word 2, the count asked − 1 in the 12
    // low bits of word 1, which the smallest page (4 KB) leaves 0.
    assert_eq!(
        (SLOT_RECORD_PARTIAL, SLOT_FRAME_ASKED_MASK),
        (1 << 5, 0xfff)
    );
    assert_eq!(SLOT_FRAME_ASKED_MASK + 1, 4096);
    let untyped = [
        SLOT_UNTYPED_DEVICE,
        SLOT_UNTYPED_CLEAN,
        SLOT_UNTYPED_BLOCK,
        SLOT_UNTYPED_ENTRY,
    ];
    assert_eq!(untyped, [1 << 31, 1 << 32, 1 << 33, 1 << 34]);
    assert_eq!(
        (SLOT_UNTYPED_LOG2_SHIFT, SLOT_UNTYPED_ENTRY_SHIFT),
        (25, 35)
    );
    // Word 6 of an untyped's root slot: the epoch its watermark became
    // ready to return at (D-135).
    assert_eq!(SLOT_UNTYPED_READY_WORD, 6);
    let remote = RemoteRef::new(1023, 4095, (1 << 30) - 1, 4095);
    assert_eq!(remote.raw(), u64::MAX);
    let remote = RemoteRef::new(5, 6, 7, 8);
    assert_eq!(remote.raw(), 5 | 6 << 10 | 7 << 22 | 8 << 52);
    assert_eq!(
        (
            remote.node(),
            remote.grant(),
            remote.serial(),
            remote.use_entry()
        ),
        (5, 6, 7, 8)
    );
}

#[test]
fn ownership_words_carry_their_check() {
    assert_eq!(Ownership::none().raw(), 0);
    let free = Ownership::free(Owner::Unclaimed);
    assert_eq!(free.state(), OwnState::Free);
    assert_eq!((free.owner(), free.generation()), (Owner::Unclaimed, 0));
    assert_eq!(free.raw() >> 16 & 0xFFFF, 0xFFFF);
    let v = 1 | 0xFFFF << 2;
    assert_eq!(
        free.raw() >> 2 & 0x3FFF,
        (v as u64).wrapping_mul(CHECK_MUL) >> 50
    );
    let taken = Ownership::taken(Owner::Node(3), 7);
    assert_eq!(
        (taken.state(), taken.owner(), taken.generation()),
        (OwnState::Taken, Owner::Node(3), 7)
    );
    assert_eq!(Ownership::from_raw(taken.raw()), Some(taken));
    // A flipped bit fails the check; state 3 never passes.
    assert_eq!(Ownership::from_raw(taken.raw() ^ 1 << 40), None);
    assert_eq!(Ownership::from_raw(taken.raw() | 3), None);
    // A reserve keeps its code; an owner that names nothing fails.
    let reserve = Ownership::taken(Owner::Reserve(2), 9);
    assert_eq!(reserve.raw() >> 16 & 0xFFFF, 0xFFF2);
    assert_eq!(Owner::from_code(0xFFF8), None);
    assert_eq!(Owner::from_code(1024), None);
    assert_eq!(Owner::from_code(1023), Some(Owner::Node(1023)));
    assert_eq!(
        (OWNER_UNCLAIMED, OWNER_RESERVE_BASE, CHECK_MUL),
        (0xFFFF, 0xFFF0, 0x9E37_79B9_7F4A_7C15)
    );
}

#[test]
fn may_take_follows_rules_1_to_4() {
    let (x, g) = (2, 5);
    let live = |generation| Member::new(MemberState::Live, Evidence::None, DRIVER_NONE, generation);
    let dead = |generation| Member::new(MemberState::Dead, Evidence::None, DRIVER_NONE, generation);
    let any = live(g);
    // 1. Its own word: free, or taken by another generation.
    assert!(Ownership::free(Owner::Node(x)).may_take(x, g, any, false));
    assert!(Ownership::taken(Owner::Node(x), g - 1).may_take(x, g, any, false));
    assert!(!Ownership::taken(Owner::Node(x), g).may_take(x, g, any, false));
    // 2. Unclaimed, free or taken.
    assert!(Ownership::free(Owner::Unclaimed).may_take(x, g, any, false));
    // 3. A dead node's word, whatever its state and generation.
    assert!(Ownership::taken(Owner::Node(1), 3).may_take(x, g, dead(3), false));
    assert!(Ownership::free(Owner::Node(1)).may_take(x, g, dead(3), false));
    assert!(!Ownership::taken(Owner::Node(1), 3).may_take(x, g, live(3), false));
    assert!(!Ownership::free(Owner::Node(1)).may_take(x, g, live(3), true));
    // 3. A word taken by a generation that is over: its owner started
    // again (Argon 1.1 of the seventh review, and S1, S2); a free word
    // given to a live node stays that node's.
    assert!(Ownership::taken(Owner::Node(1), 3).may_take(x, g, live(4), false));
    let starting = Member::new(MemberState::Starting, Evidence::None, DRIVER_NONE, 4);
    assert!(Ownership::taken(Owner::Node(1), 3).may_take(x, g, starting, false));
    assert!(!Ownership::free(Owner::Node(1)).may_take(x, g, live(4), false));
    // 4. A reserve only by the holder of its role.
    assert!(Ownership::free(Owner::Reserve(0)).may_take(x, g, any, true));
    assert!(!Ownership::free(Owner::Reserve(0)).may_take(x, g, dead(1), false));
    assert!(!Ownership::none().may_take(x, g, dead(1), true));
}

#[test]
fn device_entries_hold_a_range_and_an_owner() {
    let entry = DeviceEntry::used(0xFE00_0000, 21);
    assert!(entry.is_used());
    assert_eq!((entry.base(), entry.log2()), (0xFE00_0000, 21));
    assert_eq!(entry.owner, Ownership::free(Owner::Unclaimed));
    assert!(!DeviceEntry::unused().is_used());
    assert_eq!(DeviceEntry::USED, 1 << 6);
}

#[test]
fn member_words_read_back_and_refuse_what_is_never_written() {
    let word = Member::new(MemberState::Fencing, Evidence::Stale, 4, 9);
    assert_eq!(word.raw(), 5 | 2 << 4 | 4 << 16 | 9 << 32);
    assert_eq!(Member::from_raw(word.raw()), Some(word));
    assert_eq!(
        (
            word.state(),
            word.evidence(),
            word.driver(),
            word.generation()
        ),
        (MemberState::Fencing, Evidence::Stale, 4, 9)
    );
    assert_eq!(Member::never().raw(), 0xFFFF << 16);
    for raw in [8, 15, 7 << 4, 3 << 7, 1 << 9, 1 << 10, 1 << 15] {
        assert_eq!(Member::from_raw(raw), None, "{raw:#x}");
    }
    // Bit 9, the resume bit, only in a starting word (D-144).
    assert_eq!(MEMBER_RESUME, 1 << 9);
    let resuming = Member::new(MemberState::Starting, Evidence::None, DRIVER_NONE, 3).with_resume();
    assert_eq!(Member::from_raw(resuming.raw()), Some(resuming));
    assert!(resuming.resuming());
    assert!(!Member::new(MemberState::Starting, Evidence::None, DRIVER_NONE, 3).resuming());
    for state in [
        MemberState::Live,
        MemberState::Sealed,
        MemberState::Dead,
        MemberState::Fenced,
    ] {
        let raw = Member::new(state, Evidence::None, DRIVER_NONE, 3).raw() | MEMBER_RESUME;
        assert_eq!(Member::from_raw(raw), None, "{state:?}");
    }
    // Bits 7-8 hold the node's fence class (D-129), 0 to 2.
    let classed = word.with_class(2);
    assert_eq!(classed.raw(), word.raw() | 2 << 7);
    assert_eq!(Member::from_raw(classed.raw()), Some(classed));
    assert_eq!((classed.class(), classed.with_class(1).class()), (2, 1));
    assert!(Member::new(MemberState::Sealing, Evidence::None, DRIVER_NONE, 1).writes());
    assert!(!Member::new(MemberState::Sealed, Evidence::None, DRIVER_NONE, 1).writes());
}

#[test]
fn the_transition_check_accepts_the_rows_of_the_table() {
    use MemberState::*;
    let m = |state, evidence, driver, generation| Member::new(state, evidence, driver, generation);
    let none = |state, generation| m(state, Evidence::None, DRIVER_NONE, generation);
    let kill = |state, generation| m(state, Evidence::Kill, 1, generation);
    let rows = [
        (none(Dead, 3), none(Starting, 4), Transition::Start),
        (none(Sealed, 3), none(Starting, 3), Transition::Resume),
        (none(Starting, 3), none(Live, 3), Transition::Up),
        (none(Live, 3), none(Sealing, 3), Transition::Seal),
        (none(Sealing, 3), none(Sealed, 3), Transition::Sealed),
        (none(Sealing, 3), none(Live, 3), Transition::Abandon),
        (
            none(Dead, 3),
            none(Starting, 4).with_class(2),
            Transition::Start,
        ),
        (
            kill(Unfenced, 3),
            m(Unfenced, Evidence::Kill, 2, 3),
            Transition::Redrive,
        ),
        (none(Live, 3), kill(Fencing, 3), Transition::Accuse),
        (none(Sealed, 3), kill(Fencing, 3), Transition::Accuse),
        (
            kill(Fencing, 3),
            m(Fencing, Evidence::Kill, 2, 3),
            Transition::Redrive,
        ),
        (kill(Fencing, 3), kill(Fenced, 3), Transition::Fence),
        (kill(Fencing, 3), kill(Unfenced, 3), Transition::Unfence),
        (kill(Unfenced, 3), kill(Fenced, 3), Transition::LateFence),
        (kill(Fenced, 3), kill(Dead, 3), Transition::Reclaim),
    ];
    for (from, to, row) in rows {
        assert_eq!(member_transition(from, to), Some(row), "{from:?} {to:?}");
    }
    let refused = [
        (none(Dead, 3), none(Starting, 3)),
        (none(Fenced, 3), none(Live, 3)),
        (kill(Dead, 3), none(Live, 4)),
        (none(Live, 3), none(Fencing, 3)),
        (none(Dead, 3), none(Fencing, 3)),
        (kill(Fencing, 3), kill(Fencing, 3)),
        (none(Live, 3), none(Sealing, 4)),
        (kill(Fenced, 3), none(Dead, 3)),
        (none(Live, 3), none(Sealed, 4)),
        (kill(Unfenced, 3), kill(Unfenced, 3)),
        (kill(Fenced, 3), m(Fenced, Evidence::Kill, 2, 3)),
        // No row lets another node seal a live node (the co-hosting row of
        // D-123 is gone with D-128), and only a fresh start writes a class.
        (none(Live, 3), none(Sealed, 3)),
        (none(Starting, 3), none(Sealed, 3)),
        (none(Live, 3).with_class(2), none(Sealing, 3).with_class(1)),
        (none(Sealed, 3).with_class(2), none(Starting, 3)),
    ];
    for (from, to) in refused {
        assert_eq!(member_transition(from, to), None, "{from:?} {to:?}");
    }
}

#[test]
fn the_successor_is_the_next_live_node_of_the_best_class() {
    use MemberState::*;
    let at = |state, class| Member::new(state, Evidence::None, DRIVER_NONE, 1).with_class(class);
    let classed = |classes: [u8; 4]| {
        let states = [Live, Live, Fenced, Live];
        [0, 1, 2, 3].map(|i| at(states[i], classes[i]))
    };
    // The class is read from the member words (D-129).
    assert_eq!(successor(&classed([2, 1, 2, 2]), 2), Some(3));
    assert_eq!(successor(&classed([2, 1, 2, 2]), 3), Some(0));
    assert_eq!(successor(&classed([1, 2, 2, 1]), 2), Some(1));
    assert_eq!(successor(&classed([1, 1, 1, 1]), 0), Some(1));
    assert_eq!(best_class(&classed([1, 2, 2, 1])), Some(2));
    let gone = [at(Dead, 2), at(Sealed, 2), at(Fencing, 2), at(Unfenced, 2)];
    assert_eq!(successor(&gone, 0), None);
    assert_eq!(best_class(&gone), None);
}

#[test]
fn the_barrier_waits_for_every_node_that_could_reach_the_memory() {
    use MemberState::*;
    let view = |state, acked, maps, all| BarrierView {
        member: Member::new(state, Evidence::None, DRIVER_NONE, 1),
        acked,
        maps,
        acknowledged_by_all: all,
    };
    assert!(barrier(
        4,
        &[view(Live, 4, true, false), view(Dead, 0, true, false)],
        true
    ));
    assert!(!barrier(4, &[view(Live, 3, false, false)], true));
    assert!(!barrier(4, &[view(Live, 4, false, false)], false));
    assert!(barrier(4, &[view(Sealed, 0, false, false)], true));
    assert!(!barrier(4, &[view(Sealed, 4, true, false)], true));
    assert!(!barrier(4, &[view(Unfenced, 4, true, false)], true));
    assert!(barrier(4, &[view(Fenced, 0, true, true)], true));
    assert!(!barrier(4, &[view(Fenced, 0, true, false)], true));
}

#[test]
fn role_words_read_back() {
    let word = RoleWord::handing(3, 5, 77);
    assert_eq!(word.raw(), 2 | 3 << 2 | 5 << 18 | 77 << 34);
    assert_eq!(
        (word.state(), word.holder(), word.target(), word.epoch()),
        (RoleState::Handing, 3, 5, 77)
    );
    assert_eq!(RoleWord::from_raw(word.raw()), Some(word));
    assert_eq!(RoleWord::held(2, 1).target(), ROLE_NOBODY);
    assert_eq!(RoleWord::vacant(4).holder(), ROLE_NOBODY);
    assert_eq!(RoleWord::from_raw(3), None);
    assert_eq!(RoleWord::from_raw(1 | 7 << 18), None);
    assert_eq!(RoleWord::from_raw(7 << 2), None);
    // The epoch has 29 bits (34-62) since D-131; bit 63 is *failed*.
    assert_eq!(ROLE_EPOCH_MASK, (1 << 29) - 1);
    assert_eq!(RoleWord::held(0, 1 << 29).epoch(), 0);
    assert_eq!(RoleWord::held(0, ROLE_EPOCH_MASK).raw() >> 63, 0);
    let failed = RoleWord::held(2, 9).failed_now();
    assert_eq!((failed.raw() >> 63, ROLE_FAILED), (1, 1 << 63));
    assert!(failed.failed() && !RoleWord::held(2, 9).failed());
    assert_eq!(RoleWord::from_raw(failed.raw()), Some(failed));
    assert_eq!((failed.epoch(), failed.holder()), (9, 2));
    let next = failed.keeping_failed(RoleWord::held(4, 10));
    assert!(next.failed() && next.holder() == 4 && next.epoch() == 10);
    assert_eq!(
        [
            RoleKind::Unit as u8,
            RoleKind::Reserve as u8,
            RoleKind::Plain as u8
        ],
        [0, 1, 2]
    );
    assert_eq!(
        [
            Governs::IommuUnit as u8,
            Governs::Console as u8,
            Governs::Plain as u8,
            Governs::Aplic as u8
        ],
        [0, 4, 6, 7]
    );
    assert_eq!(ROLE_PLAIN_PLATFORM, 0);
}

#[test]
fn feature_masks_are_judged_invalid_then_unsupported() {
    let list = aarch64::FEATURES;
    let mut bytes = [0; 32];
    bytes[0] = 512;
    bytes[1] = 4096;
    let kind = Kind {
        features: 0b011,
        vector_bytes: 32,
        baseline_bytes: 528,
        feature_bytes: bytes,
    };
    assert_eq!(kind.baseline(list), 0b001);
    assert_eq!(FeatureMask::new(0b001, 32).check(list, &kind), Ok(()));
    assert_eq!(FeatureMask::new(0b011, 32).check(list, &kind), Ok(()));
    let invalid = [1 << 48, 1 << 3, 32 << 32, 0b001];
    for raw in invalid {
        let mask = FeatureMask::from_raw(raw);
        assert_eq!(mask.check(list, &kind), Err(Error::Invalid), "{raw:#x}");
    }
    let unsupported = [
        1 << 31,
        FeatureMask::new(0b101, 32).raw(),
        FeatureMask::new(0b001, 16).raw(),
    ];
    for raw in unsupported {
        let mask = FeatureMask::from_raw(raw);
        assert_eq!(mask.check(list, &kind), Err(Error::Unsupported), "{raw:#x}");
    }
    // Invalid comes first: a reserved bit with bit 31.
    assert_eq!(
        FeatureMask::from_raw(1 << 31 | 1 << 50).check(list, &kind),
        Err(Error::Invalid)
    );
    assert_eq!(FEATURE_USER_INTERRUPTS, 31);
    assert_eq!(FEATURE_INSTRUCTION_FIRST, 32);
}

#[test]
fn the_size_of_a_thread_follows_its_mask() {
    let list = x86_64::FEATURES;
    let mut bytes = [0; 32];
    bytes[x86_64::AVX512 as usize] = 2048;
    bytes[x86_64::AMX as usize] = 8192;
    let kind = Kind {
        features: 0b0111,
        vector_bytes: 0,
        baseline_bytes: x86_64::XSTATE_X87_SSE_AVX_BYTES,
        feature_bytes: bytes,
    };
    // AVX and AVX-512 are baseline; AMX is large.
    assert_eq!(kind.baseline(list), 0b011);
    assert_eq!(thread_bytes(FeatureMask::NONE, list, &kind), Some(2048));
    assert_eq!(
        thread_bytes(FeatureMask::new(0b011, 0), list, &kind),
        Some(2048)
    );
    assert_eq!(
        thread_bytes(FeatureMask::new(0b100, 0), list, &kind),
        Some(16384)
    );
    let host = Kind {
        features: 0b1111,
        ..kind
    };
    assert!(kind_covers(&host, &kind) && !kind_covers(&kind, &host));
    let longer = Kind {
        vector_bytes: 64,
        ..kind
    };
    let shorter = Kind {
        vector_bytes: 32,
        ..kind
    };
    assert!(!kind_covers(&longer, &shorter) && kind_covers(&kind, &kind));
    assert!(FeatureMask::new(0b100, 0).fits(&kind) && !FeatureMask::new(0b1000, 0).fits(&kind));
    assert_eq!(riscv64::FEATURES.with_length, 1);
}

#[test]
fn floors_are_whole_chunks_of_the_boot_numbers() {
    let boot = BootFloors {
        floor_node: (64 << 20) + 1,
        floor_tables: 1,
        reserve_bytes: 128 << 20,
        reserve_per_gib: 1 << 20,
        reserves: 2,
        plain_roles: 0,
    };
    let chunk = 2 << 20;
    let f = floors(8 << 30, 4, boot, chunk).unwrap();
    assert_eq!(f.floor, 66 << 20);
    // D-154: the table floor, rounded up to a chunk, once per node slot.
    assert_eq!(f.tables, 2 << 20);
    assert_eq!(f.reserve, 136 << 20);
    assert_eq!(f.total, 4 * (68 << 20) + 2 * (136 << 20));
    assert_eq!(floors(256 << 20, 4, boot, chunk), None);
    let huge = BootFloors {
        floor_node: u64::MAX,
        ..boot
    };
    assert_eq!(floors(u64::MAX, 1, huge, chunk), None);
}

#[test]
fn module_frames_take_the_largest_size_that_fits() {
    let g = x86_64::GEOMETRY;
    let frames: Vec<_> = module_frames(0x20_0000 - 0x1000, None, 0x40_1000, g).collect();
    let expected = [
        ModuleFrame {
            size: PageSize::Page,
            pages: 1,
            offset: 0,
        },
        ModuleFrame {
            size: PageSize::Large1,
            pages: 2,
            offset: 0x1000,
        },
    ];
    assert_eq!(frames, expected);
    // A virtual address that is not aligned keeps small pages.
    let frames: Vec<_> = module_frames(0x20_0000, Some(0x20_1000), 0x20_0000, g).collect();
    assert_eq!(
        frames,
        [ModuleFrame {
            size: PageSize::Page,
            pages: 512,
            offset: 0
        }]
    );
    // At most E pages; the last one ends at the length rounded up.
    let frames: Vec<_> = module_frames(0, Some(0x1000), 513 * 4096 - 5, g).collect();
    let pages: Vec<_> = frames.iter().map(|f| (f.size, f.pages)).collect();
    assert_eq!(pages, [(PageSize::Page, 512), (PageSize::Page, 1)]);
    let frames: Vec<_> = module_frames(0, None, 513 * 4096 - 5, g).collect();
    let pages: Vec<_> = frames.iter().map(|f| (f.size, f.pages)).collect();
    assert_eq!(pages, [(PageSize::Large1, 1), (PageSize::Page, 1)]);
    // Small pages end at the first position aligned on a larger size.
    let frames: Vec<_> = module_frames(0x1000, None, 0x40_0000, g).collect();
    assert_eq!(
        frames[0],
        ModuleFrame {
            size: PageSize::Page,
            pages: 511,
            offset: 0
        }
    );
    assert_eq!(frames[1].size, PageSize::Large1);
}

#[test]
fn a_creation_clears_64_kb_per_call() {
    assert_eq!(clear_step(0, 1 << 20), 65_536);
    assert_eq!(clear_step(65_536, 100_000), 100_000);
    assert_eq!(clear_step(5, 5), 5);
    assert_eq!(clear_steps(0, 2 << 20), 32);
    assert_eq!(clear_steps(0, 1), 1);
    assert_eq!(clear_steps(9, 9), 1);
}

#[test]
fn port_ranges_and_checks_follow_the_contract() {
    let range = IoPortRange::new(0x60, 0x64).unwrap();
    assert_eq!(
        (range.raw(), range.first(), range.last()),
        (0x64_0060, 0x60, 0x64)
    );
    assert_eq!(IoPortRange::ALL.raw(), 0xFFFF_0000);
    assert_eq!(IoPortRange::new(5, 4), None);
    assert_eq!(IoPortRange::new(0, 0x1_0000), None);
    assert!(range.contains(0x60, 4) && range.contains(0x64, 1));
    assert!(!range.contains(0x62, 4) && !range.contains(0x5F, 2) && !range.contains(0x60, 0));
    assert!(!IoPortRange::ALL.contains(u64::MAX, 2));
    assert!(IoPortRange::ALL.covers(range) && !range.covers(IoPortRange::ALL));
    assert_eq!(ioport_check(range, 0x60, 3, None), Err(Error::Invalid));
    assert_eq!(ioport_check(range, 0x63, 2, None), Ok(()));
    assert_eq!(ioport_check(range, 0x64, 2, None), Err(Error::Invalid));
    assert_eq!(ioport_check(range, 0x60, 1, Some(0xFF)), Ok(()));
    assert_eq!(
        ioport_check(range, 0x60, 1, Some(0x100)),
        Err(Error::Invalid)
    );
    assert_eq!(ioport_check(range, 0x60, 4, Some(u32::MAX as u64)), Ok(()));
    assert!(ports_meet((0x20, 0x21), (0x21, 0x30)) && !ports_meet((0x20, 0x21), (0x22, 0x30)));
}

#[test]
fn debug_values_have_their_numbers() {
    assert_eq!(
        (
            DEBUG_AREA_BYTES,
            DEBUG_HEADER_BYTES,
            CORE_CRASH_RECORD_BYTES
        ),
        (69_632, 64, 4_032)
    );
    assert_eq!(DEBUG_AREA_BYTES, 17 * 4096);
    assert_eq!(
        DEBUG_HEADER_BYTES + CORE_CRASH_RECORD_BYTES,
        TRACE_KERNEL_OFFSET
    );
    assert_eq!(
        TRACE_KERNEL_OFFSET + TRACE_KERNEL_ENTRIES as usize * TRACE_ENTRY_BYTES,
        TRACE_USER_OFFSET
    );
    assert_eq!(
        TRACE_USER_OFFSET + TRACE_USER_ENTRIES as usize * TRACE_ENTRY_BYTES,
        DEBUG_AREA_BYTES
    );
    assert_eq!(
        (CRASH_HEADER_BYTES, TRACE_TEXT_BYTES, DEBUG_WRITE_MAX),
        (4096, 24, 512)
    );
    assert_eq!((TRACE_BURST, TRACE_BURST_NS), (16, 1_000_000));
    assert_eq!(
        (CONSOLE_STEP_BYTES, CONSOLE_TICK_NS, STUB_STOP_NS),
        (16, 1_500_000, 10_000_000)
    );
    assert_eq!(
        (CRASH_RECORD_KERNEL_ENTRIES, CRASH_RECORD_USER_ENTRIES),
        (32, 16)
    );
    use TraceCode::*;
    let codes = [
        Text,
        DmaFault,
        Suppressed,
        UnitTimeout,
        FaultRecorded,
        Failure,
        HardwareError,
        StrayVector,
        Transition,
        BadWord,
        Refused,
        AreaCleared,
    ];
    for (number, code) in codes.into_iter().enumerate() {
        assert_eq!(code as u16, number as u16 + 1);
        assert!(!code.user());
    }
    assert_eq!((UserText as u16, UserEmit as u16), (16, 17));
    assert!(UserText.user() && UserEmit.user());
    assert_eq!(TRACE_FLAG_CONTINUED, 1 << 16);
    // Bits 17-31 of word 2 hold the node slot the core ran (D-129).
    assert_eq!((TRACE_SLOT_SHIFT, TRACE_SLOT_MASK), (17, 0x7fff));
    let word = TraceEntry::word2_of(5, true, 300, 7);
    assert_eq!(word, 5 | 1 << 16 | 300 << 17 | 7 << 32);
    let traced = TraceEntry {
        word2: word,
        ..TraceEntry::default()
    };
    assert_eq!((traced.slot(), traced.generation()), (300, 7));
    assert_eq!(
        (DEBUG_AREA_VERSION, CRASH_AREA_VERSION, DEBUG_AREA_BYTES),
        (2, 2, 69_632)
    );
    assert_eq!(
        (
            trace_index(1, 256),
            trace_index(256, 256),
            trace_index(257, 256)
        ),
        (0, 255, 0)
    );
    let entry = TraceEntry {
        word2: 16 | TRACE_FLAG_CONTINUED | 3 << 32,
        ..TraceEntry::default()
    };
    assert_eq!(
        (entry.code(), entry.continued(), entry.generation()),
        (16, true, 3)
    );
    let reasons = [
        CrashReason::Assertion as u32,
        CrashReason::Fenced as u32,
        CrashReason::StartRefused as u32,
        CrashReason::CoreSilent as u32,
    ];
    assert_eq!(reasons, [1, 5, 8, 9]);
    use ConsoleKind as C;
    assert_eq!(
        [
            C::None as u32,
            C::Uart16550Port as u32,
            C::Uart16550Mmio as u32,
            C::Pl011 as u32,
            C::SbiDbcn as u32
        ],
        [0, 1, 2, 3, 4]
    );
    assert_eq!(
        (
            ConsoleMessage::KernelStarts as u32,
            ConsoleMessage::Suppressed as u32
        ),
        (1, 14)
    );
    assert_eq!(
        (Build::Production as u32, Build::Development as u32),
        (0, 1)
    );
}

/// The check of an ownership word is a product of 64 bits that Kani does
/// not prove in reasonable time: here every owner code, with extreme and
/// random generations, reads back through `from_raw`, and flipped bits are
/// refused.
#[test]
fn ownership_words_read_back_and_refuse_flipped_bits() {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let owners = (0..NODES_MAX as u16)
        .map(Owner::Node)
        .chain([Owner::Unclaimed])
        .chain((0..RESERVES_MAX as u16).map(Owner::Reserve));
    let (mut flips, mut caught) = (0u32, 0u32);
    for owner in owners {
        for generation in [0, 1, u32::MAX, next() as u32, next() as u32] {
            for word in [Ownership::taken(owner, generation), Ownership::free(owner)] {
                assert_eq!(Ownership::from_raw(word.raw()), Some(word));
                assert_eq!(word.owner(), owner);
                for bit in 0..64 {
                    let flipped = Ownership::from_raw(word.raw() ^ 1 << bit);
                    // A flip of a check bit is always caught.
                    if (2..16).contains(&bit) {
                        assert_eq!(flipped, None, "{word:?} bit {bit}");
                    }
                    flips += 1;
                    caught += flipped.is_none() as u32;
                }
            }
        }
    }
    // A flip elsewhere escapes only when the product keeps its top 14 bits:
    // about one in 16,384.
    assert!(flips - caught <= flips / 4096, "{caught} of {flips}");
    assert_eq!(Ownership::free(Owner::Unclaimed).generation(), 0);
}

/// DeepSeek F2: the six classes of G2 have their bytes, and their permission
/// rules are those of G3 (4).
#[test]
fn page_classes_have_their_bytes_and_the_rules_of_g3() {
    use crate::{Leaf, PageClass};
    let raw = [
        PageClass::DATA,
        PageClass::GATE,
        PageClass::GLOBAL_READ_ONLY,
        PageClass::DEVICE,
    ]
    .map(PageClass::raw);
    assert_eq!(raw, [0, 1, 2, 3]);
    // D-152: bits 0-5 the class, bits 6-7 the hook state. An iospace table
    // of level k is 32 + k (k up to 7), an iospace root with n devices 40 +
    // n, n up to 16, *live* (1) or, with none, *dying* (3); the former 4
    // names no class (D-137's 48 + n does not fit six bits).
    assert_eq!(PageClass::iospace_table(1).map(PageClass::raw), Some(33));
    assert_eq!(PageClass::iospace_table(0), None);
    assert_eq!(PageClass::iospace_table(8), None);
    assert_eq!(
        PageClass::iospace_root(0).map(PageClass::raw),
        Some(40 | 1 << 6)
    );
    assert_eq!(
        PageClass::iospace_root(16).map(PageClass::raw),
        Some(56 | 1 << 6)
    );
    assert_eq!(PageClass::iospace_root(17), None);
    assert_eq!(PageClass::IOSPACE_ROOT_DYING.raw(), 40 | 3 << 6);
    assert_eq!(PageClass::its_table(false).raw(), 8);
    assert_eq!(PageClass::its_table(true).raw(), 8 | 1 << 6);
    assert_eq!(PageClass::from_raw(4), None);
    let root = PageClass::iospace_root(3).unwrap();
    assert_eq!((root.root_devices(), root.iospace_level()), (Some(3), None));
    let lower = PageClass::iospace_table(2).unwrap();
    assert_eq!(
        (lower.root_devices(), lower.iospace_level()),
        (None, Some(2))
    );
    assert_eq!(PageClass::table(1).map(PageClass::raw), Some(17));
    assert_eq!(PageClass::table(0), None);
    assert_eq!(PageClass::table(9), None);
    for byte in [5, 16, 25, 40, 48, 64, 255] {
        assert_eq!(PageClass::from_raw(byte), None, "{byte}");
    }
    // DeepSeek F11 of the seventh review, with D-152's hook states: for an
    // aspace whose root is at level 4, the bytes the page classes hold are
    // data (0); an ITS table unpinned or pinned; a table of level 1 to 3
    // loose, hooked or unhooking; the root unbound, live or dying; an
    // iospace table of level 1 to 7 loose, hooked or unhooking; an iospace
    // root of 0 to 16 devices live, of none dying. Gate, global read-only
    // and device pages lie in no chunk.
    let stored: Vec<u8> = (0..=255)
        .filter(|&b| PageClass::stored_at(b, 4).is_some())
        .collect();
    let mut expected: Vec<u8> = Vec::from([0, 8, 8 | 64]);
    for hook in 0..3u8 {
        expected.extend((17..=19).map(|c| c | hook << 6));
        expected.extend((33..=39).map(|c| c | hook << 6));
    }
    expected.extend([20, 20 | 64, 20 | 192]);
    expected.extend((40..=56).map(|c| c | 64));
    expected.push(40 | 192);
    expected.sort_unstable();
    assert_eq!(stored, expected);
    // A table of level 4 below a root at level 5 may be unhooking; a root
    // at level 4 never.
    let unhooking = PageClass::table_hooked(4, crate::Hook::Unhooking).unwrap();
    assert!(PageClass::stored_at(unhooking.raw(), 5).is_some());
    assert!(PageClass::stored_at(unhooking.raw(), 4).is_none());
    let dying = PageClass::aspace_root(4, crate::RootState::Dying).unwrap();
    assert!(PageClass::stored_at(dying.raw(), 4).is_some());
    assert!(PageClass::stored_at(dying.raw(), 5).is_none());
    assert!(PageClass::stored(dying.raw()).is_some());
    let leaf = |write, exec, device_type| Leaf {
        write,
        exec,
        device_type,
    };
    let data = PageClass::DATA;
    assert!(data.permits(leaf(true, false, false)));
    assert!(data.permits(leaf(false, true, false)));
    assert!(!data.permits(leaf(true, true, false)));
    assert!(!data.permits(leaf(false, false, true)));
    let shared = PageClass::GLOBAL_READ_ONLY;
    assert!(shared.permits(leaf(false, true, false)));
    assert!(!shared.permits(leaf(true, false, false)));
    let device = PageClass::DEVICE;
    assert!(device.permits(leaf(true, false, true)));
    assert!(!device.permits(leaf(false, true, true)));
    assert!(!device.permits(leaf(false, false, false)));
    for class in [
        PageClass::GATE,
        PageClass::iospace_table(1).unwrap(),
        PageClass::iospace_root(1).unwrap(),
        PageClass::table(1).unwrap(),
        PageClass::table(4).unwrap(),
        PageClass::its_table(false),
        PageClass::its_table(true),
    ] {
        assert!(!class.permits(leaf(false, false, false)), "{class:?}");
    }
}

/// D-152: the ITS word, the request record's state word, the R1 word and
/// the suspend word read back and refuse the words their checks refuse.
#[test]
fn the_words_of_d152_read_back_and_check() {
    use crate::{
        AttachState, ItsWord, R1State, R1Word, RequestState, RequestWord, SuspendState, SuspendWord,
    };
    let its = ItsWord::new(AttachState::Pinned, 0x4_0000, 2).unwrap();
    assert_eq!(
        (its.state(), its.table(), its.k()),
        (Some(AttachState::Pinned), 0x4_0000, 2)
    );
    assert_eq!(ItsWord::from_raw(its.raw()), Some(its));
    // A table not aligned on its 2^k pages, k above 4, a table of 0.
    assert_eq!(ItsWord::new(AttachState::Pinned, 0x1000, 1), None);
    assert_eq!(ItsWord::new(AttachState::Pinned, 0x10_0000, 5), None);
    assert_eq!(ItsWord::new(AttachState::Pinned, 0, 0), None);
    // State 0 with another bit; a bit between k and the address.
    assert_eq!(ItsWord::from_raw(0x4_0000), None);
    assert_eq!(ItsWord::from_raw(its.raw() | 1 << 5), None);
    assert_eq!(ItsWord::from_raw(0), Some(ItsWord::NONE));

    let opened = RequestWord::new(7, RequestState::Done)
        .unwrap()
        .opened()
        .unwrap();
    assert_eq!((opened.serial(), opened.state()), (8, RequestState::Open));
    assert_eq!(opened.opened(), None, "an open record opened again");
    assert_eq!(
        opened.answered(7, RequestState::Done),
        None,
        "another serial"
    );
    assert_eq!(opened.answered(8, RequestState::Open), None);
    let done = opened.answered(8, RequestState::Done).unwrap();
    assert_eq!((done.serial(), done.state()), (8, RequestState::Done));
    assert_eq!(done.answered(8, RequestState::Done), None, "answered twice");
    let void = opened.voided();
    assert_eq!((void.serial(), void.state()), (8, RequestState::Void));
    assert_eq!(done.voided(), done);
    assert_eq!(RequestWord::from_raw(5), None);
    assert_eq!(RequestWord::new(1 << 61, RequestState::Void), None);

    assert_eq!(size_of::<R1Word>(), 16);
    assert_eq!(R1Word::START.checked(), Some(R1State::Done));
    let walking = R1Word {
        generation: 3,
        state: 1,
        cursor: 9,
    };
    assert_eq!(walking.checked(), Some(R1State::Walking));
    assert_eq!(
        R1Word {
            state: 2,
            ..walking
        }
        .checked(),
        None,
        "done with a cursor"
    );
    assert_eq!(
        R1Word {
            state: 3,
            ..walking
        }
        .checked(),
        None
    );

    let word = SuspendWord::new(SuspendState::Asleep, 5, 9);
    assert_eq!(
        (word.state(), word.node(), word.generation()),
        (SuspendState::Asleep, 5, 9)
    );
    assert_eq!(SuspendWord::from_raw(word.raw()), Some(word));
    assert_eq!(
        SuspendWord::new(SuspendState::None, 5, 9),
        SuspendWord::NONE
    );
    assert_eq!(SuspendWord::from_raw(1 << 16), None, "state 0 with a node");
    assert_eq!(SuspendWord::from_raw(word.raw() | 1 << 4), None);
}

/// D-137: the attach word of a device identifier, 0 or a state and the
/// root's page-aligned address; a state 0 with an address, or an address
/// not aligned on a page, fails its check.
#[test]
fn an_attach_word_holds_a_state_and_a_page_aligned_root() {
    use crate::{AttachState, AttachWord};
    assert_eq!(AttachWord::from_raw(0), Some(AttachWord::NONE));
    assert_eq!(AttachWord::NONE.state(), None);
    for state in [
        AttachState::Pinned,
        AttachState::Attached,
        AttachState::Detached,
    ] {
        let word = AttachWord::new(state, 0x1234_5000).unwrap();
        assert_eq!((word.state(), word.root()), (Some(state), 0x1234_5000));
        assert_eq!(AttachWord::from_raw(word.raw()), Some(word));
    }
    assert_eq!(
        AttachWord::new(AttachState::Pinned, 0x1000).map(AttachWord::raw),
        Some(0x1001)
    );
    assert_eq!(AttachWord::new(AttachState::Pinned, 0), None);
    assert_eq!(AttachWord::new(AttachState::Pinned, 0x1800), None);
    assert_eq!(AttachWord::from_raw(0x1000), None);
    assert_eq!(AttachWord::from_raw(0x1006), None);
    assert_eq!(AttachWord::from_raw(2), None);
}

/// DeepSeek F4: the map count is the low 32 bits of the gate word.
#[test]
fn the_gate_word_holds_a_32_bit_map_count_and_the_pages_not_of_class_data() {
    use crate::{GateWord, MAP_COUNT_MAX};
    assert_eq!(MAP_COUNT_MAX, (1 << 32) - 1);
    let word = GateWord::new(u32::MAX, 4096).unwrap();
    assert_eq!((word.map_count(), word.not_data()), (u32::MAX, 4096));
    assert_eq!(word.raw(), 0xFFFF_FFFF | 4096 << 32);
    assert_eq!(GateWord::from_raw(word.raw()), Some(word));
    assert_eq!(GateWord::new(0, 4097), None);
    // D-148: bits 45-57 the grant count; D-154: bit 58 the table bit, a
    // bit above 58 refused.
    assert_eq!(GateWord::from_raw(1 << 58), Some(GateWord::TABLE));
    assert_eq!(GateWord::from_raw(1 << 59), None);
    assert_eq!(GateWord::from_raw(1 << 45).map(GateWord::grants), Some(1));
    assert_eq!(GateWord::from_raw(4097 << 32), None);
    let granted = word.with_grants(crate::GRANT_COUNT_MAX).unwrap();
    assert_eq!(
        (granted.map_count(), granted.not_data(), granted.grants()),
        (u32::MAX, 4096, crate::GRANT_COUNT_MAX)
    );
    assert_eq!(word.with_grants(crate::GRANT_COUNT_MAX + 1), None);
    assert_eq!(crate::device_leaf_word(3, 2), 3 | 2 << 32);
}

/// DeepSeek F9: the MSI formats of the result of `irq_claim`.
#[test]
fn msi_addresses_follow_the_remappable_and_the_direct_formats() {
    use x86_64::{msi_direct, msi_remapped};
    assert_eq!(msi_remapped(0), 0xFEE0_0010);
    assert_eq!(msi_remapped(1), 0xFEE0_0030);
    assert_eq!(msi_remapped(0x7FFF), 0xFEE0_0000 | 0x7FFF << 5 | 1 << 4);
    assert_eq!(msi_remapped(0x8000), 0xFEE0_0014);
    assert_eq!(msi_direct(1, 48), Some((0xFEE0_1000, 48)));
    assert_eq!(msi_direct(254, 239), Some((0xFEE0_0000 | 254 << 12, 239)));
    // 255 is the broadcast to every local APIC (D-141).
    assert_eq!(msi_direct(255, 239), None);
    assert_eq!(msi_direct(256, 48), None);
}

/// DeepSeek F11: the bits of the marks (Tasks and marks) and of the task
/// flags.
#[test]
fn marks_and_task_flags_have_their_bits() {
    assert_eq!((Marks::PRIVATE.bits(), Marks::UNTRUSTED.bits()), (1, 2));
    assert_eq!((TaskFlags::AWARE, TaskFlags::REVOKED), (1, 2));
}

/// Argon F3: the values and rules of unlike cores, I/O ports, the debug
/// area and the module frames that no test pinned (mutants 3, 4, 5, 13,
/// 14, 18, 69, 70, 73, 101, 109, 118, 160, 168).
#[test]
fn values_of_unlike_cores_ports_debug_and_modules_that_no_test_pinned() {
    use crate::{
        DEBUG_AREA_MAGIC, FEATURE_USER_INTERRUPTS, FeatureList, FeatureMask, IoPortRange, Kind,
        aarch64, ioport_check, kind_covers,
    };

    // #3, #4: Kind::baseline excludes bit 31 and bits not in list.state
    let list = FeatureList {
        state: 0b0011 | (1 << FEATURE_USER_INTERRUPTS),
        with_length: 0,
        large: 0,
    };
    let kind = Kind {
        features: 0b0111 | (1 << FEATURE_USER_INTERRUPTS),
        vector_bytes: 0,
        baseline_bytes: 64,
        feature_bytes: [0; 32],
    };
    assert_eq!(kind.baseline(list), 0b0011);

    // #5: FeatureMask::vector_bytes reads all 16 bits (bits 32..47)
    let m256 = FeatureMask::new(1 << aarch64::SVE, 256);
    assert_eq!(m256.vector_bytes(), 256);

    // #13, #14, #109: FeatureMask::fits checks vector_bytes when non-zero, allows 0, and ignores bit 31
    let k32 = Kind {
        features: 1 << aarch64::SVE,
        vector_bytes: 32,
        baseline_bytes: 64,
        feature_bytes: [0; 32],
    };
    assert!(!FeatureMask::new(1 << aarch64::SVE, 64).fits(&k32));
    assert!(FeatureMask::new(0, 0).fits(&k32));
    assert!(FeatureMask::new(1 << FEATURE_USER_INTERRUPTS, 0).fits(&k32));

    // #101: FeatureMask::check rejects bit 31 with Unsupported even if kind.features has bit 31 set
    assert_eq!(
        FeatureMask::new(1 << FEATURE_USER_INTERRUPTS, 0).check(list, &kind),
        Err(crate::Error::Unsupported)
    );

    // #18: kind_covers checks instruction features in bits 32..63
    let host_no_inst = Kind { features: 1, ..k32 };
    let node_with_inst = Kind {
        features: 1 | (1u64 << 32),
        ..k32
    };
    assert!(!kind_covers(&host_no_inst, &node_with_inst));

    // #69, #70, #73: IoPortRange::new boundary values and ioport_check width=8
    assert!(IoPortRange::new(0x3F8, 0x3F8).is_some());
    assert!(IoPortRange::new(0, 0xFFFF).is_some());
    assert_eq!(
        ioport_check(IoPortRange::ALL, 0x3F8, 8, None),
        Err(crate::Error::Invalid)
    );

    // #118: module_frames allows up to geometry.entries (> 512) pages in one frame on 16 KB geometry
    let geo16k = crate::Geometry {
        page_shift: [14, 25, 36, 0],
        entries: 2048,
        levels: 3,
        user_end: (1 << 46) - (1 << 36),
    };
    let frames: Vec<_> = crate::module_frames(1 << 14, None, 600 * (1 << 14), geo16k).collect();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].pages, 600);

    // #160: aarch64::FEATURES includes SME in with_length
    assert_ne!(aarch64::FEATURES.with_length & (1 << aarch64::SME), 0);

    // #168: DEBUG_AREA_MAGIC is b"DoDb"
    assert_eq!(DEBUG_AREA_MAGIC, 0x6244_6F44);
}

/// Argon section 2.3 of the seventh review: the survivors of the abi.
#[test]
fn r7_abi_survivors() {
    // #002: START_MEMORY_UNFENCED_ACCEPTED == 1 << 4
    assert_eq!(START_MEMORY_UNFENCED_ACCEPTED, 1 << 4);
    // #015: the user range end leaves the root entry of the reserved slot
    // (turned from Argon's "accepts the half", which pass 14 refuses: Opus N1
    // of the verification of the new findings).
    let g = Geometry {
        page_shift: [12, 21, 30, 0],
        entries: 512,
        levels: 4,
        user_end: (1u64 << 47) - (1 << 39),
    };
    assert!(g.valid());
    assert!(
        !Geometry {
            user_end: 1 << 47,
            ..g
        }
        .valid()
    );
    // #016: SLOT_RECORD_PARTIAL == 1 << 5
    assert_eq!(SLOT_RECORD_PARTIAL, 1 << 5);
    // #017: PageClass::table(8) == Some(PageClass(24))
    assert!(PageClass::table(8).is_some());
    assert_ne!(PageClass::table(8), PageClass::table(7));
    // #023: PageClass::GLOBAL_READ_ONLY refuses device_type leaves
    assert!(!PageClass::GLOBAL_READ_ONLY.permits(Leaf {
        write: false,
        exec: false,
        device_type: true,
    }));
    // #027, revised by D-148 and D-154: GateWord::from_raw rejects a bit
    // above 58 (bit 58 is the table bit).
    assert_eq!(GateWord::from_raw((1u64 << 59) | (1u64 << 32)), None);
    assert!(GateWord::from_raw((1u64 << 58) | (1u64 << 32)).is_some_and(GateWord::is_table));
    // #030 (restated for D-128, which removed the co-hosting row): a
    // sealed word is reached only from sealing, keeping evidence and driver.
    let live = Member::new(MemberState::Live, Evidence::None, DRIVER_NONE, 1);
    let sealing = Member::new(MemberState::Sealing, Evidence::None, DRIVER_NONE, 1);
    let sealed_bad = Member::new(MemberState::Sealed, Evidence::Seal, 0, 1);
    let sealed_ok = Member::new(MemberState::Sealed, Evidence::None, DRIVER_NONE, 1);
    assert_eq!(member_transition(live, sealed_ok), None);
    assert_eq!(member_transition(sealing, sealed_bad), None);
    assert_eq!(
        member_transition(sealing, sealed_ok),
        Some(Transition::Sealed)
    );
}

/// The exception classes of ARM64 (`ESR_ELx.EC`) against Linux's
/// `arch/arm64/include/asm/esr.h` at commit c03c1a1 (tag v7.3, 18 October
/// 2026, github.com/torvalds/linux): each of Linux's `ESR_ELx_EC_*` values,
/// copied here by hand with its name, is the value of the class `abi/`
/// names. Linux names no constant for 0x14 (FEAT_SYSREG128), 0x1B
/// (FEAT_TME), 0x1E (FEAT_RME) and 0x3D (FEAT_EBEP), which Arm DDI 0487
/// K.a defines: they are listed apart.
#[test]
fn arm64_exception_classes_agree_with_linux() {
    use crate::aarch64::*;
    let linux: [(&str, u8, u8); 45] = [
        ("UNKNOWN", 0x00, EC_UNKNOWN),
        ("WFx", 0x01, EC_WFX),
        ("CP15_32", 0x03, EC_CP15_MCR_MRC),
        ("CP15_64", 0x04, EC_CP15_MCRR_MRRC),
        ("CP14_MR", 0x05, EC_CP14_MCR_MRC),
        ("CP14_LS", 0x06, EC_CP14_LDC_STC),
        ("FP_ASIMD", 0x07, EC_FP_SIMD),
        ("CP10_ID", 0x08, EC_CP10_VMRS),
        ("PAC", 0x09, EC_PAC_TRAP),
        ("OTHER", 0x0A, EC_LD64B),
        ("CP14_64", 0x0C, EC_CP14_MRRC),
        ("BTI", 0x0D, EC_BTI),
        ("ILL", 0x0E, EC_ILLEGAL_STATE),
        ("SVC32", 0x11, EC_SVC32),
        ("HVC32", 0x12, EC_HVC32),
        ("SMC32", 0x13, EC_SMC32),
        ("SVC64", 0x15, EC_SVC64),
        ("HVC64", 0x16, EC_HVC64),
        ("SMC64", 0x17, EC_SMC64),
        ("SYS64", 0x18, EC_SYS64),
        ("SVE", 0x19, EC_SVE),
        ("ERET", 0x1a, EC_ERET),
        ("FPAC", 0x1C, EC_PAC_FAIL),
        ("SME", 0x1D, EC_SME),
        ("IMP_DEF", 0x1f, EC_IMPDEF_EL3),
        ("IABT_LOW", 0x20, EC_IABT_LOWER),
        ("IABT_CUR", 0x21, EC_IABT_SAME),
        ("PC_ALIGN", 0x22, EC_PC_ALIGN),
        ("DABT_LOW", 0x24, EC_DABT_LOWER),
        ("DABT_CUR", 0x25, EC_DABT_SAME),
        ("SP_ALIGN", 0x26, EC_SP_ALIGN),
        ("MOPS", 0x27, EC_MOPS),
        ("FP_EXC32", 0x28, EC_FP_TRAP32),
        ("FP_EXC64", 0x2C, EC_FP_TRAP64),
        ("GCS", 0x2D, EC_GCS),
        ("SERROR", 0x2F, EC_SERROR),
        ("BREAKPT_LOW", 0x30, EC_BREAKPOINT_LOWER),
        ("BREAKPT_CUR", 0x31, EC_BREAKPOINT_SAME),
        ("SOFTSTP_LOW", 0x32, EC_STEP_LOWER),
        ("SOFTSTP_CUR", 0x33, EC_STEP_SAME),
        ("WATCHPT_LOW", 0x34, EC_WATCHPOINT_LOWER),
        ("WATCHPT_CUR", 0x35, EC_WATCHPOINT_SAME),
        ("BKPT32", 0x38, EC_BKPT32),
        ("VECTOR32", 0x3A, EC_VECTOR_CATCH32),
        ("BRK64", 0x3C, EC_BRK64),
    ];
    for (name, value, ours) in linux {
        assert_eq!(value, ours, "ESR_ELx_EC_{name}");
        assert!(EXCEPTION_CLASSES.contains(&ours), "{name}");
    }
    let only_arm = [EC_SYS128, EC_TSTART, EC_GPC, EC_PMU];
    assert_eq!(only_arm, [0x14, 0x1B, 0x1E, 0x3D]);
    let mut all: Vec<u8> = linux.iter().map(|l| l.1).chain(only_arm).collect();
    all.sort();
    let mut ours = EXCEPTION_CLASSES.to_vec();
    ours.sort();
    assert_eq!(
        all, ours,
        "abi defines exactly Linux's classes and Arm's four others"
    );
    assert_eq!((ESR_EC_SHIFT, ESR_EC_MASK >> ESR_EC_SHIFT), (26, 0x3F));
}

/// The exception codes of RISC-V against Linux's
/// `arch/riscv/include/asm/csr.h` at commit c03c1a1 (tag v7.3): each of
/// Linux's `EXC_*` values, copied by hand with its name. Linux names no
/// constant for code 19, the hardware error of the privileged
/// specification (Supervisor-Level ISA 1.13).
#[test]
fn riscv_exception_codes_agree_with_linux() {
    use crate::riscv64::*;
    let linux: [(&str, u64, u64); 19] = [
        ("INST_MISALIGNED", 0, CAUSE_INSTRUCTION_MISALIGNED),
        ("INST_ACCESS", 1, CAUSE_INSTRUCTION_ACCESS),
        ("INST_ILLEGAL", 2, CAUSE_ILLEGAL_INSTRUCTION),
        ("BREAKPOINT", 3, CAUSE_BREAKPOINT),
        ("LOAD_MISALIGNED", 4, CAUSE_LOAD_MISALIGNED),
        ("LOAD_ACCESS", 5, CAUSE_LOAD_ACCESS),
        ("STORE_MISALIGNED", 6, CAUSE_STORE_MISALIGNED),
        ("STORE_ACCESS", 7, CAUSE_STORE_ACCESS),
        ("SYSCALL", 8, CAUSE_ECALL_U),
        ("HYPERVISOR_SYSCALL", 9, CAUSE_ECALL_S),
        ("SUPERVISOR_SYSCALL", 10, CAUSE_ECALL_VS),
        ("INST_PAGE_FAULT", 12, CAUSE_INSTRUCTION_PAGE),
        ("LOAD_PAGE_FAULT", 13, CAUSE_LOAD_PAGE),
        ("STORE_PAGE_FAULT", 15, CAUSE_STORE_PAGE),
        ("SOFTWARE_CHECK", 18, CAUSE_SOFTWARE_CHECK),
        ("INST_GUEST_PAGE_FAULT", 20, CAUSE_INSTRUCTION_GUEST_PAGE),
        ("LOAD_GUEST_PAGE_FAULT", 21, CAUSE_LOAD_GUEST_PAGE),
        ("VIRTUAL_INST_FAULT", 22, CAUSE_VIRTUAL_INSTRUCTION),
        ("STORE_GUEST_PAGE_FAULT", 23, CAUSE_STORE_GUEST_PAGE),
    ];
    for (name, value, ours) in linux {
        assert_eq!(value, ours, "EXC_{name}");
    }
    let mut all: Vec<u64> = linux
        .iter()
        .map(|l| l.1)
        .chain([CAUSE_HARDWARE_ERROR])
        .collect();
    all.sort();
    let mut ours = CAUSES.to_vec();
    ours.sort();
    assert_eq!(all, ours);
    assert_eq!(CAUSE_HARDWARE_ERROR, 19);
    // CAUSE_IRQ_FLAG of Linux on RV64.
    assert_eq!(SCAUSE_INTERRUPT, 1 << 63);
}

/// The classification of Faults on ARM64 and RISC-V, for what a user
/// thread can raise (the two architectures' rows of the table of kinds).
#[test]
fn arm64_and_riscv_exceptions_map_to_the_kinds_of_faults() {
    use crate::ExceptionUse::{Call, Fault, Feature, NotThread, Own};
    use crate::FaultKind::*;
    use crate::{aarch64 as a, riscv64 as r};
    let esr = |ec: u8, iss: u64| (ec as u64) << a::ESR_EC_SHIFT | iss;
    let arm = [
        (esr(a::EC_DABT_LOWER, 0x07), Fault(Page)),
        (esr(a::EC_IABT_LOWER, 0x2B), Fault(Page)),
        (esr(a::EC_DABT_LOWER, 0x0F), Fault(Page)),
        (esr(a::EC_DABT_LOWER, 0x21), Fault(Protection)),
        (esr(a::EC_DABT_LOWER, 0x10), Fault(Protection)),
        (esr(a::EC_DABT_LOWER, 0x09), NotThread),
        (esr(a::EC_DABT_LOWER, 0x18), NotThread),
        (esr(a::EC_PC_ALIGN, 0), Fault(Protection)),
        (esr(a::EC_SP_ALIGN, 0), Fault(Protection)),
        (esr(a::EC_BTI, 0), Fault(Protection)),
        (esr(a::EC_UNKNOWN, 0), Fault(Undefined)),
        (esr(a::EC_SYS64, 0), Fault(Undefined)),
        (esr(a::EC_SVE, 0), Feature(a::SVE)),
        (esr(a::EC_SME, 0), Feature(a::SME)),
        // The SME trap code (D-141): ZT0 is its own feature; an instruction
        // refused by the streaming or ZA state is undefined.
        (esr(a::EC_SME, 0b100), Feature(a::SME2_ZT0)),
        (esr(a::EC_SME, 0b001), Fault(Undefined)),
        (esr(a::EC_SME, 0b010), Fault(Undefined)),
        (esr(a::EC_SME, 0b011), Fault(Undefined)),
        (esr(a::EC_FP_SIMD, 0), Own),
        (esr(a::EC_FP_TRAP64, 0), Fault(Arithmetic)),
        (esr(a::EC_SVC64, 0), Call),
        (esr(a::EC_BRK64, 0), Fault(Debug)),
        (esr(a::EC_STEP_LOWER, 0), Fault(Debug)),
        (esr(a::EC_BREAKPOINT_LOWER, 0), Fault(HardwareBreakpoint)),
        (esr(a::EC_WATCHPOINT_LOWER, 0), Fault(HardwareBreakpoint)),
        (esr(a::EC_DABT_SAME, 0x07), NotThread),
        (esr(a::EC_SERROR, 0), NotThread),
        (esr(a::EC_SVC32, 0), NotThread),
    ];
    for (syndrome, wanted) in arm {
        assert_eq!(a::exception_use(syndrome), Some(wanted), "{syndrome:#x}");
    }
    assert_eq!(a::exception_use(esr(0x02, 0)), None);
    assert_eq!(a::exception_use(esr(a::EC_DABT_LOWER, 0x3F)), None);
    let riscv = [
        (r::CAUSE_LOAD_PAGE, Fault(Page)),
        (r::CAUSE_STORE_PAGE, Fault(Page)),
        (r::CAUSE_INSTRUCTION_PAGE, Fault(Page)),
        (r::CAUSE_LOAD_ACCESS, Fault(Protection)),
        (r::CAUSE_STORE_ACCESS, Fault(Protection)),
        (r::CAUSE_INSTRUCTION_ACCESS, Fault(Protection)),
        (r::CAUSE_LOAD_MISALIGNED, Fault(Protection)),
        (r::CAUSE_SOFTWARE_CHECK, Fault(Protection)),
        (r::CAUSE_ILLEGAL_INSTRUCTION, Fault(Undefined)),
        (r::CAUSE_BREAKPOINT, Fault(Debug)),
        (r::CAUSE_ECALL_U, Call),
        (r::CAUSE_ECALL_S, NotThread),
        (r::CAUSE_HARDWARE_ERROR, NotThread),
        (r::CAUSE_LOAD_GUEST_PAGE, NotThread),
    ];
    for (scause, wanted) in riscv {
        assert_eq!(r::exception_use(scause), Some(wanted), "{scause}");
    }
    assert_eq!(r::exception_use(11), None);
    assert_eq!(r::exception_use(24), None);
    assert_eq!(r::exception_use(r::SCAUSE_INTERRUPT | 5), None);
}

/// Seventh review (Mistral, as judged by Hydrogen): the GIC's ranges, with
/// the extended PPIs 1056-1119 and the extended SPIs from 4096; the IMSIC's
/// identities 1 to 2,047.
#[test]
fn interrupt_ranges_of_arm64_and_riscv() {
    use crate::{aarch64 as a, riscv64 as r};
    assert_eq!(
        (a::GIC_PRIVATE_LAST, a::GIC_SPI_FIRST, a::GIC_SPI_LAST_MAX),
        (31, 32, 1019)
    );
    assert_eq!((a::GIC_EPPI_FIRST, a::GIC_EPPI_LAST), (1056, 1119));
    assert_eq!(a::GIC_EPPI_LAST - a::GIC_EPPI_FIRST + 1, 64);
    assert_eq!((a::GIC_ESPI_FIRST, a::GIC_ESPI_LAST_MAX), (4096, 5119));
    assert_eq!(a::GIC_LPI_FIRST, 8192);
    assert_eq!((r::IMSIC_IDENTITY_FIRST, r::IMSIC_IDENTITY_MAX), (1, 2047));
    assert_eq!(
        (a::IRQ_TABLE_ENTRIES, r::IRQ_TABLE_ENTRIES),
        (crate::x86_64::IRQ_TABLE_ENTRIES, 192)
    );
}

#[test]
fn the_evidence_seal_has_the_number_of_host() {
    let evidence = [
        Evidence::None,
        Evidence::SelfReported,
        Evidence::Stale,
        Evidence::Stuck,
        Evidence::Kill,
        Evidence::Retire,
        Evidence::Seal,
    ];
    for (number, e) in evidence.into_iter().enumerate() {
        assert_eq!(e as u8, number as u8);
        let word = Member::new(MemberState::Fencing, e, 1, 1);
        assert_eq!(Member::from_raw(word.raw()).map(Member::evidence), Some(e));
    }
}

#[test]
fn core_words_have_their_bits_and_refuse_a_bad_check() {
    let states = [
        CoreState::Free as u8,
        CoreState::Owned as u8,
        CoreState::Leaving as u8,
        CoreState::Joining as u8,
    ];
    assert_eq!(states, [0, 1, 2, 3]);
    assert_eq!(
        (CORE_MARK, CORE_UNUSABLE, CORE_NO_SLOT),
        (1 << 2, 1 << 3, 0xFFFF)
    );
    let word = CoreWord::owned(5, 9);
    let fields = word.raw() & !(0xfff << 4);
    assert_eq!(fields, 1 | 5 << 16 | 9 << 32);
    // The check: the top 12 bits of the word with check 0 times CHECK_MUL.
    assert_eq!(
        word.raw() >> 4 & 0xfff,
        fields.wrapping_mul(CHECK_MUL) >> 52
    );
    assert_eq!(CoreWord::from_raw(word.raw()), Some(word));
    assert_eq!(CoreWord::from_raw(word.raw() ^ 1 << 4), None);
    assert_eq!(CoreWord::from_raw(word.raw() ^ 1 << 16), None);
    let marked = CoreWord::joining(5, 9).marked();
    assert!(marked.is_marked() && marked.names(5, 9) && !marked.names(5, 8));
    assert_eq!(marked.state(), CoreState::Joining);
    let free = CoreWord::free(true);
    assert!(free.is_unusable() && free.state() == CoreState::Free);
    assert_eq!((free.slot(), free.generation()), (CORE_NO_SLOT, 0));
    assert!(!free.names(CORE_NO_SLOT, 0));
    assert!(word.unusable().is_unusable() && word.unusable().names(5, 9));
}

#[test]
fn the_eleven_kinds_of_message_have_their_numbers_and_rings() {
    for (index, kind) in NodeMessage::ALL.into_iter().enumerate() {
        // 9 is reserved (the `Ping` removed by D-135).
        let number = index as u8 + if index < 8 { 1 } else { 2 };
        assert_eq!(kind as u8, number);
        assert_eq!(NodeMessage::from_raw(kind as u8), Some(kind));
        let user = matches!(kind, NodeMessage::Send | NodeMessage::Signal);
        assert_eq!(kind.ring() == Ring::User, user, "{kind:?}");
    }
    assert_eq!(NodeMessage::from_raw(0), None);
    assert_eq!(NodeMessage::from_raw(9), None);
    assert_eq!(NodeMessage::from_raw(13), None);
    use NodeMessage::*;
    let requests: [_; 3] = [Revoke, UnitRequest, ReportAsk];
    let answers = [RevokeStep, UnitAnswer, Report, SendAnswer];
    for kind in NodeMessage::ALL {
        assert_eq!(kind.is_request(), requests.contains(&kind));
        assert_eq!(kind.is_answer(), answers.contains(&kind));
    }
    assert_eq!(
        answers.map(NodeMessage::answers),
        [Some(Revoke), Some(UnitRequest), Some(ReportAsk), Some(Send)]
    );
}

#[test]
fn values_of_the_rings_have_their_numbers() {
    assert_eq!(RING_ENTRY_BYTES, 64);
    assert_eq!((RING_KERNEL_ENTRIES, RING_USER_ENTRIES), (64, 64));
    assert_eq!(RING_MESSAGE_ENTRIES_MAX, 11);
    assert_eq!((REQUESTS_PER_PEER, SENDS_PER_PEER), (16, 8));
    assert_eq!((SENDS_RESERVED_FOR_MANAGE, SEND_GRANTS_MAX), (2, 4));
    assert_eq!((RING_USER_STEP, PENDING_STEP), (16, 32));
    assert_eq!((UNIT_REQUESTS, RING_ANSWER_RESERVE), (64, 34));
    // The reserve: one entry per credit and the rest of one Report.
    assert_eq!(
        RING_ANSWER_RESERVE as usize,
        REQUESTS_PER_PEER + SENDS_PER_PEER + RING_MESSAGE_ENTRIES_MAX - 1
    );
    // Eight KB per ordered pair.
    let pair = (RING_KERNEL_ENTRIES + RING_USER_ENTRIES) as usize * RING_ENTRY_BYTES;
    assert_eq!(pair, 8192);
    let results = [
        UnitResult::Done as u8,
        UnitResult::Elsewhere as u8,
        UnitResult::Failed as u8,
    ];
    assert_eq!(results, [0, 1, 2]);
    assert_eq!(REPORT_END, u64::MAX);
    assert_eq!((CREDIT_NONE, CREDIT_SERIAL_MASK), (31, (1 << 27) - 1));
}

#[test]
fn a_ring_header_holds_its_credit_and_judges_its_length() {
    let header = RingHeader::new(NodeMessage::Send, 10, (3, 7), 2, Some((17, 99)));
    assert_eq!(
        (
            header.kind,
            header.length,
            header.sender,
            header.sender_generation
        ),
        (11, 10, 3, 7)
    );
    assert_eq!(
        (header.receiver_generation, header.credit),
        (2, 17 | 99 << 5)
    );
    assert_eq!(header.credit(), Some((17, 99)));
    let ping = RingHeader::new(NodeMessage::Release, 1, (3, 7), 2, None);
    assert_eq!((ping.credit, ping.credit()), (31, None));
    assert!(header.readable(10) && !header.readable(9));
    let zero = RingHeader {
        length: 0,
        ..header
    };
    let long = RingHeader {
        length: 12,
        ..header
    };
    let unknown = RingHeader { kind: 13, ..header };
    assert!(!zero.readable(64) && !long.readable(64) && !unknown.readable(64));
    // The words of the largest messages fit eleven entries.
    assert_eq!(message_entries(0), 1);
    assert_eq!(message_entries(3 + SEND_GRANTS_MAX + MAX_WORDS), 10);
    assert_eq!(message_entries(6), 1);
    assert_eq!(message_entries(7), 2);
}

#[test]
fn ring_indices_count_free_entries_and_resynchronise() {
    let n = RING_KERNEL_ENTRIES;
    assert_eq!(ring_free(10, 10, n), 64);
    assert_eq!(ring_free(74, 10, n), 0);
    assert_eq!(ring_free(80, 10, n), 0);
    // Counts that never go back: the difference wraps.
    assert_eq!(ring_free(3, u64::MAX - 1, n), 59);
    assert!(ring_fits(10, 10, n, 30, RING_ANSWER_RESERVE));
    assert!(!ring_fits(10, 10, n, 31, RING_ANSWER_RESERVE));
    assert!(ring_fits(10, 10, n, 64, 0));
    assert!(!ring_fits(0, 0, n, u64::MAX, 1));
    assert_eq!(ring_resync(100, 10, n), 36);
    assert_eq!(ring_resync(74, 10, n), 10);
}

/// The fence record holds a core's last stop at its core number modulo 64,
/// which never moves while R6 frees the other words of the node (D-134).
#[test]
fn a_fence_record_indexes_its_last_stops_by_core_number_modulo_64() {
    assert_eq!(FenceRecord::stop_index(5), 5);
    assert_eq!(FenceRecord::stop_index(64 + 5), 5);
    let window: Vec<usize> = (128..192).map(FenceRecord::stop_index).collect();
    assert_eq!(window, (0..64).collect::<Vec<_>>());
}

/// The pass-22 corrections of `abi/` (D-141), one behaviour each.
#[test]
fn a_geometry_with_a_shift_near_255_is_refused_not_an_overflow() {
    let g = Geometry {
        page_shift: [12, 255, 1, 0],
        entries: 512,
        levels: 4,
        user_end: 1 << 30,
    };
    assert!(!g.valid());
    let g = Geometry {
        page_shift: [12, 21, 250, 0],
        ..g
    };
    assert!(!g.valid());
}

#[test]
fn dr7_refuses_an_io_breakpoint_a_length_of_ten_and_a_length_on_an_execution_breakpoint() {
    use x86_64::dr7_valid;
    assert!(dr7_valid(1));
    assert!(dr7_valid(1 | 0b01 << 16 | 0b11 << 18));
    assert!(!dr7_valid(1 | 0b10 << 16));
    assert!(!dr7_valid(1 | 0b01 << 16 | 0b10 << 18));
    assert!(!dr7_valid(1 | 0b11 << 18));
}

#[test]
fn a_credit_index_past_the_credits_is_none() {
    for index in 0..32u8 {
        let header = RingHeader::new(NodeMessage::Revoke, 1, (0, 1), 1, Some((index, 1)));
        let credit = header.credit();
        assert_eq!(
            credit.is_some(),
            (index as usize) < REQUESTS_PER_PEER + SENDS_PER_PEER
        );
    }
}

#[test]
fn a_clock_page_with_a_shift_of_128_reads_0() {
    let page = ClockPage {
        version: 1,
        seq: 0,
        shift: 128,
        flags: 0,
        mult: 1,
        counter_base: 0,
        ns_base: 5,
    };
    assert_eq!(page.nanoseconds(100), 0);
}

#[test]
#[should_panic]
fn a_deadline_at_the_time_limit_is_outside_firing_instant() {
    firing_instant(TIME_LIMIT, 0);
}

#[test]
#[should_panic]
fn a_device_entry_below_a_page_is_refused() {
    DeviceEntry::used(64, 6);
}

#[test]
fn a_word_whose_owner_names_nothing_is_no_ones_to_take() {
    let word = Ownership::make_raw_for_tests(OwnState::Taken, 0x2000, 1);
    let live = Member::new(MemberState::Live, Evidence::None, 0, 1);
    assert!(!word.may_take(0, 1, live, true));
}

#[test]
fn module_frames_of_a_length_beyond_the_addresses_does_not_overflow() {
    let frames = module_frames(0, None, u64::MAX, x86_64::GEOMETRY);
    assert!(frames.take(3).count() == 3);
    let frames = module_frames(u64::MAX - 4095, None, 4096, x86_64::GEOMETRY);
    assert_eq!(frames.count(), 1);
}

#[test]
fn the_crash_area_magic_is_fixed() {
    assert_eq!(CRASH_AREA_MAGIC.to_le_bytes(), *b"DodOScr\0");
}

/// The offset of every field of the `repr(C)` records that had only a size
/// check (the final review's verification of lot A, H6: two fields of
/// `DeviceEntry` swapped kept every test green), as the contract lays them
/// out: the table of Start-up state, the device table, the fence record,
/// `BootNodes`, a ring entry of a debug area and the stop record.
#[test]
fn fields_of_every_record_lie_where_the_contract_puts_them() {
    use core::mem::offset_of;
    // The device table: a word with the base, then an ownership word.
    assert_eq!(
        [
            offset_of!(DeviceEntry, range),
            offset_of!(DeviceEntry, owner)
        ],
        [0, 8]
    );
    // The fence record: the generation (bits 0-31 of word 0), word 1 the
    // first action, word 2 the confirmation, word 3 reserved, words 4 to 67
    // the last stops.
    let fence = [
        offset_of!(FenceRecord, generation),
        offset_of!(FenceRecord, reserved),
        offset_of!(FenceRecord, first_action),
        offset_of!(FenceRecord, confirmed),
        offset_of!(FenceRecord, reserved2),
        offset_of!(FenceRecord, last_stop),
    ];
    assert_eq!(fence, [0, 4, 8, 16, 24, 32]);
    // `BootNodes`: the node slots, then each node as a window and a mask.
    assert_eq!(
        [offset_of!(BootNodes, slots), offset_of!(BootNodes, nodes)],
        [0, 4]
    );
    assert_eq!(
        [offset_of!(BootNode, window), offset_of!(BootNode, mask)],
        [0, 8]
    );
    // A ring entry: sequence, instant, word 2, task, marks, payload.
    let entry = [
        offset_of!(TraceEntry, seq),
        offset_of!(TraceEntry, instant),
        offset_of!(TraceEntry, word2),
        offset_of!(TraceEntry, task),
        offset_of!(TraceEntry, marks),
        offset_of!(TraceEntry, payload),
    ];
    assert_eq!(entry, [0, 8, 16, 24, 32, 40]);
    // The stop record: commit, core, reason, instant, 128 bytes of text.
    let stop = [
        offset_of!(StopRecord, commit),
        offset_of!(StopRecord, core),
        offset_of!(StopRecord, reason),
        offset_of!(StopRecord, instant),
        offset_of!(StopRecord, text),
    ];
    assert_eq!(stop, [0, 8, 12, 16, 24]);
    // The records of Start-up state, row by row.
    let firmware = [
        offset_of!(StartFirmwareTable, kind),
        offset_of!(StartFirmwareTable, reserved),
        offset_of!(StartFirmwareTable, address),
        offset_of!(StartFirmwareTable, len),
    ];
    assert_eq!(firmware, [0, 4, 8, 16]);
    let range = [
        offset_of!(StartMemoryRange, base),
        offset_of!(StartMemoryRange, len),
        offset_of!(StartMemoryRange, kind),
        offset_of!(StartMemoryRange, reserved),
    ];
    assert_eq!(range, [0, 8, 16, 20]);
    let framebuffer = [
        offset_of!(StartFramebuffer, base),
        offset_of!(StartFramebuffer, size),
        offset_of!(StartFramebuffer, width),
        offset_of!(StartFramebuffer, height),
        offset_of!(StartFramebuffer, pitch),
        offset_of!(StartFramebuffer, format),
    ];
    assert_eq!(framebuffer, [0, 8, 16, 20, 24, 28]);
    let times = [
        offset_of!(StartBootTimes, frequency),
        offset_of!(StartBootTimes, stage_entry),
        offset_of!(StartBootTimes, firmware_end),
        offset_of!(StartBootTimes, kernel),
        offset_of!(StartBootTimes, first_thread),
        offset_of!(StartBootTimes, stage),
        offset_of!(StartBootTimes, stage_version),
    ];
    assert_eq!(times, [0, 8, 16, 24, 32, 40, 44]);
    let crash = [
        offset_of!(StartCrash, base),
        offset_of!(StartCrash, len),
        offset_of!(StartCrash, sequence),
        offset_of!(StartCrash, slot),
        offset_of!(StartCrash, flags),
    ];
    assert_eq!(crash, [0, 8, 16, 24, 28]);
    let interrupts = [
        offset_of!(StartInterrupts, first_vector),
        offset_of!(StartInterrupts, last_vector),
        offset_of!(StartInterrupts, flags),
        offset_of!(StartInterrupts, ioapics),
    ];
    assert_eq!(interrupts, [0, 4, 8, 12]);
    assert_eq!(
        [
            offset_of!(StartIoApic, first_gsi),
            offset_of!(StartIoApic, inputs)
        ],
        [0, 4]
    );
    assert_eq!(
        [offset_of!(StartKept, base), offset_of!(StartKept, len)],
        [0, 8]
    );
    let iommu = [
        offset_of!(StartIommu, slot),
        offset_of!(StartIommu, segment),
        offset_of!(StartIommu, domains),
        offset_of!(StartIommu, role),
    ];
    assert_eq!(iommu, [0, 4, 8, 12]);
    let role = [
        offset_of!(StartRole, kind),
        offset_of!(StartRole, governs),
        offset_of!(StartRole, object),
        offset_of!(StartRole, slot),
    ];
    assert_eq!(role, [0, 1, 2, 4]);
    let image = [
        offset_of!(StartImage, base),
        offset_of!(StartImage, len),
        offset_of!(StartImage, sha256),
    ];
    assert_eq!(image, [0, 8, 16]);
    let feedback = [
        offset_of!(StartFeedback, package),
        offset_of!(StartFeedback, slot),
        offset_of!(StartFeedback, bytes),
        offset_of!(StartFeedback, reserved),
    ];
    assert_eq!(feedback, [0, 4, 8, 12]);
}

/// Every fault status code of an ARM64 abort, 0x00 to 0x3F, as the table
/// of kinds of Faults reads it (Arm DDI 0487 M.d, D24.2): a translation or
/// permission fault is a page fault; alignment, an external abort not on a
/// walk, a tag check and an unsupported exclusive or atomic access are
/// protection; an address size or access flag fault, an abort on a walk,
/// a parity or ECC error, a granule protection fault, a TLB conflict and an
/// unsupported atomic update are not the thread's; the rest is reserved
/// (the final review's verification of lot A, H6: 8 of the 64 codes were
/// pinned, and 0x35 moved to "not the thread's" left every test green).
#[test]
fn every_abort_status_code_of_arm64_has_its_kind() {
    use crate::ExceptionUse::{Fault, NotThread};
    use crate::FaultKind::{Page, Protection};
    let wanted = |fsc: u8| match fsc {
        0x04..=0x07 | 0x2A | 0x2B | 0x0C..=0x0F => Some(Fault(Page)),
        0x10 | 0x11 | 0x21 | 0x35 => Some(Fault(Protection)),
        0x00..=0x03 | 0x29 | 0x2C | 0x08..=0x0B => Some(NotThread),
        0x12..=0x18 | 0x1B..=0x1F | 0x22..=0x28 | 0x30 | 0x31 | 0x34 => Some(NotThread),
        0x19 | 0x1A | 0x20 | 0x2D..=0x2F | 0x32 | 0x33 | 0x36..=0x3F => None,
        _ => unreachable!(),
    };
    for fsc in 0..=0x3F {
        assert_eq!(crate::aarch64::abort_use(fsc), wanted(fsc), "{fsc:#x}");
    }
}

/// D-154: bit 58 of the gate word is the table bit, kept by every write of
/// a count; a table chunk counts no leaf and no grant, an ordinary chunk no
/// page that is not data.
#[test]
fn the_gate_word_carries_the_table_bit_through_its_counts() {
    use crate::{GATE_WORD_TABLE, GRANT_COUNT_MAX, GateWord};
    assert_eq!(GATE_WORD_TABLE, 1 << 58);
    assert_eq!(
        (GateWord::TABLE.raw(), GateWord::ORDINARY.raw()),
        (1 << 58, 0)
    );
    assert!(GateWord::TABLE.is_table() && !GateWord::ORDINARY.is_table());
    let table = GateWord::TABLE.with_not_data(3).unwrap();
    assert!(table.is_table() && table.consistent());
    assert_eq!(
        (table.map_count(), table.not_data(), table.grants()),
        (0, 3, 0)
    );
    assert_eq!(GateWord::TABLE.with_not_data(4097), None);
    // A table chunk that counts a leaf or a grant, and an ordinary chunk
    // that holds a table, are words no gate writes.
    assert!(!table.with_map_count(1).consistent());
    assert!(!table.with_grants(1).unwrap().consistent());
    assert!(table.with_map_count(1).is_table());
    assert!(table.with_grants(GRANT_COUNT_MAX).unwrap().is_table());
    let ordinary = GateWord::new(7, 0).unwrap().with_grants(2).unwrap();
    assert!(ordinary.consistent() && !ordinary.is_table());
    assert!(!ordinary.with_not_data(1).unwrap().consistent());
    assert_eq!(ordinary.with_map_count(9).grants(), 2);
    // Bits 59-63 are zero.
    for bit in 59..64 {
        assert_eq!(GateWord::from_raw(1 << bit), None, "bit {bit}");
    }
}

/// D-154: `memory_take` kind 2, bit 51 of an untyped's slot word 0, bit 1
/// of `StartUntyped.flags`, and the 22 entries of `ObjectSizes` (F12).
#[test]
fn a_table_untyped_has_its_kind_its_slot_bit_and_its_start_flag() {
    use crate::{
        MEMORY_TAKE_ENTRY, MEMORY_TAKE_RUN, MEMORY_TAKE_TABLES, OBJECT_SIZES_ENTRIES, ObjectKind,
        SLOT_UNTYPED_BLOCK, SLOT_UNTYPED_CLEAN, SLOT_UNTYPED_DEVICE, SLOT_UNTYPED_ENTRY,
        SLOT_UNTYPED_TABLE, SPACE_HEADER_PAGES, SPACE_ROOT_PAGES, START_UNTYPED_DEVICE,
        START_UNTYPED_TABLE,
    };
    assert_eq!(
        [MEMORY_TAKE_RUN, MEMORY_TAKE_ENTRY, MEMORY_TAKE_TABLES],
        [0, 1, 2]
    );
    assert_eq!(SLOT_UNTYPED_TABLE, 1 << 51);
    let others = SLOT_UNTYPED_DEVICE | SLOT_UNTYPED_CLEAN | SLOT_UNTYPED_BLOCK | SLOT_UNTYPED_ENTRY;
    // Bits 35-50 hold a device entry's index: bit 51 is the next free one.
    assert_eq!(SLOT_UNTYPED_TABLE & (others | 0xFFFF << 35), 0);
    assert_eq!((START_UNTYPED_DEVICE, START_UNTYPED_TABLE), (1, 2));
    assert_eq!(OBJECT_SIZES_ENTRIES, 22);
    assert_eq!(ObjectKind::ALL[21], ObjectKind::ItsTable);
    assert_eq!(ObjectKind::ItsTable as u32, 21);
    assert_eq!(SPACE_HEADER_PAGES + SPACE_ROOT_PAGES, crate::ASPACE_PAGES);
    assert_eq!(SPACE_HEADER_PAGES + SPACE_ROOT_PAGES, crate::IOSPACE_PAGES);
}

/// D-154: a boot image whose `floor_tables` is 0 gives fewer than one
/// table chunk per node slot, and has no floors.
#[test]
fn floors_need_a_table_floor_of_at_least_one_chunk() {
    let boot = BootFloors {
        floor_node: 2 << 20,
        floor_tables: 0,
        reserve_bytes: 2 << 20,
        reserve_per_gib: 0,
        reserves: 2,
        plain_roles: 0,
    };
    let chunk = 2 << 20;
    assert_eq!(floors(1 << 30, 2, boot, chunk), None);
    let one = BootFloors {
        floor_tables: 1,
        ..boot
    };
    let f = floors(1 << 30, 2, one, chunk).unwrap();
    assert_eq!((f.floor, f.tables), (chunk, chunk));
    assert_eq!(f.total, 2 * 2 * chunk + 2 * chunk);
    // Three chunks and a byte round up to four.
    let more = BootFloors {
        floor_tables: 3 * chunk + 1,
        ..boot
    };
    assert_eq!(floors(1 << 30, 2, more, chunk).unwrap().tables, 4 * chunk);
    let huge = BootFloors {
        floor_tables: u64::MAX,
        ..boot
    };
    assert_eq!(floors(u64::MAX, 1, huge, chunk), None);
}

/// D-155, G7: one tag is the gate's on each architecture, and the tag
/// table holds the others, at most 4,095.
#[test]
fn the_tag_table_leaves_the_gate_its_tag() {
    use crate::{TAGS_USABLE_MAX, gate_tag, usable_tags};
    assert_eq!(TAGS_USABLE_MAX, 4095);
    // x86-64: 12-bit PCIDs less `GATE_PCID`.
    assert_eq!(usable_tags(x86_64::PCID_BITS), 4095);
    assert_eq!(
        gate_tag(x86_64::PCID_BITS),
        Some(u32::from(x86_64::GATE_PCID))
    );
    // ARM64: 8 or 16-bit ASIDs less the gate's.
    assert_eq!((usable_tags(8), gate_tag(8)), (255, Some(255)));
    assert_eq!((usable_tags(16), gate_tag(16)), (4095, Some(4095)));
    // RISC-V: the ASID bits `satp` accepts; none, and no tag is reserved.
    assert_eq!((usable_tags(4), gate_tag(4)), (15, Some(15)));
    assert_eq!((usable_tags(1), gate_tag(1)), (1, Some(1)));
    assert_eq!((usable_tags(0), gate_tag(0)), (1, None));
    for width in 0..=64 {
        let usable = usable_tags(width);
        assert!((1..=TAGS_USABLE_MAX).contains(&usable), "width {width}");
        assert!(gate_tag(width).is_none_or(|tag| tag >= usable));
    }
}

/// D-155, G10: the ordinary inter-kernel vectors lie inside 240 to 255,
/// and a line without remapping takes a vector of 48 to 239.
#[test]
fn inter_kernel_vectors_lie_above_the_claimable_ones() {
    use x86_64::{IKV_FIRST, IKV_LAST, IRQ_VECTOR_LAST, ikv_valid, line_vector_valid};
    const { assert!(IRQ_VECTOR_LAST < IKV_FIRST && IKV_FIRST <= IKV_LAST) };
    const { assert!(IKV_FIRST >= 240) };
    assert_eq!((IKV_FIRST, IKV_LAST), (240, 243));
    for vector in 0..=u8::MAX {
        assert_eq!(ikv_valid(vector), (IKV_FIRST..=IKV_LAST).contains(&vector));
        assert_eq!(line_vector_valid(vector), (48..=239).contains(&vector));
        assert!(!(ikv_valid(vector) && line_vector_valid(vector)));
    }
    // An exception's vector, the NMI's and a claimable one are refused.
    assert!(!ikv_valid(2) && !ikv_valid(31) && !ikv_valid(48) && !ikv_valid(239));
    assert!(!line_vector_valid(47) && !line_vector_valid(240));
}

/// D-155: `DR7.GD = 1` (bit 13, general detect) is refused, alone or with
/// an otherwise valid word, as an I/O condition is.
#[test]
fn dr7_refuses_general_detect() {
    use x86_64::{DR7_GD, DR7_USER_MASK, dr7_valid};
    assert_eq!(DR7_GD, 1 << 13);
    assert_eq!(DR7_USER_MASK & DR7_GD, 0);
    let good = 1 | 0b01 << 16 | 0b11 << 18;
    assert!(dr7_valid(good));
    assert!(!dr7_valid(DR7_GD) && !dr7_valid(good | DR7_GD));
    assert!(!dr7_valid(good | 0b10 << 20));
}

/// D-159, gap 50 (Exhaustion and replenishment): the budgets of an
/// exhaustion are min(1 + ⌊−r / B⌋, `SCHED_MAX_DEBT`); the rest of a debt
/// beyond the bound is forgiven, and the products stay below 2^62.
#[test]
fn the_budgets_of_an_exhaustion_are_bounded_by_sched_max_debt() {
    use crate::{SCHED_MAX_DEBT, SchedDebt, sched_debt};
    assert_eq!(SCHED_MAX_DEBT, 1 << 26);
    let debt = |budgets, forgiven| SchedDebt { budgets, forgiven };
    let b = SCHED_MIN_BUDGET_NS;
    assert_eq!(sched_debt(0, b), debt(1, false));
    assert_eq!(sched_debt(-(b as i64) + 1, b), debt(1, false));
    assert_eq!(sched_debt(-(b as i64), b), debt(2, false));
    // The bound: a debt of exactly `SCHED_MAX_DEBT` budgets is whole, one
    // nanosecond more is bounded.
    let last = (SCHED_MAX_DEBT * b - 1) as i64;
    assert_eq!(sched_debt(-last, b), debt(SCHED_MAX_DEBT, false));
    assert_eq!(sched_debt(-last - 1, b), debt(SCHED_MAX_DEBT, true));
    assert_eq!(sched_debt(i64::MIN, b), debt(SCHED_MAX_DEBT, true));
    assert_eq!(sched_debt(i64::MIN, 1), debt(SCHED_MAX_DEBT, true));
    const { assert!(SCHED_MAX_DEBT * SCHED_MAX_PERIOD_NS <= 1 << 62) };
}

/// D-159, gap 50: "a sum of instants saturates at `TIME_LIMIT`".
#[test]
fn a_sum_of_instants_saturates_at_the_time_limit() {
    use crate::{SCHED_MAX_DEBT, instant_after};
    assert_eq!(instant_after(5, 3, 10), 35);
    assert_eq!(instant_after(5, 0, 10), 5);
    let far = SCHED_MAX_DEBT * SCHED_MAX_PERIOD_NS;
    assert_eq!(
        instant_after(far, SCHED_MAX_DEBT, SCHED_MAX_PERIOD_NS),
        TIME_LIMIT
    );
    assert_eq!(instant_after(TIME_LIMIT - 1, 1, 1), TIME_LIMIT);
    assert_eq!(instant_after(TIME_LIMIT - 2, 1, 1), TIME_LIMIT - 1);
    assert_eq!(instant_after(u64::MAX, u64::MAX, u64::MAX), TIME_LIMIT);
}
