//! The few UEFI and PI interfaces the image uses, written by hand. Layouts
//! and GUIDs follow the EDK2 reference headers (MdePkg/Include at commit
//! 8628578, 13 November 2026): `Uefi/UefiSpec.h` (system table, boot and
//! runtime services, UEFI 2.10 chapter 4), `Protocol/SimpleFileSystem.h`,
//! `Protocol/LoadedImage.h`, `Protocol/SimpleTextIn.h`, `Protocol/SimpleTextOut.h`,
//! `Guid/Acpi.h`, and `Protocol/MpService.h` (PI 1.8 volume 2, MP services).
//! The UEFI specification's own text could not be fetched (uefi.org answers
//! automated clients with a challenge page): not verified against it.

use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type Event = *mut c_void;
pub type Status = usize;

pub const SUCCESS: Status = 0;
const ERROR_BIT: usize = 1 << 63;
pub const NOT_FOUND: Status = ERROR_BIT | 14;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);

pub const LOADED_IMAGE_GUID: Guid = Guid(
    0x5B1B_31A1,
    0x9562,
    0x11d2,
    [0x8E, 0x3F, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const SIMPLE_FILE_SYSTEM_GUID: Guid = Guid(
    0x964e_5b22,
    0x6459,
    0x11d2,
    [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
);
pub const MP_SERVICES_GUID: Guid = Guid(
    0x3fdd_a605,
    0xa76e,
    0x4f46,
    [0xad, 0x29, 0x12, 0xf4, 0x53, 0x1b, 0x3d, 0x08],
);
/// ACPI 2.0 and later RSDP (`Guid/Acpi.h`, `EFI_ACPI_20_TABLE_GUID`).
pub const ACPI_20_TABLE_GUID: Guid = Guid(
    0x8868_e871,
    0xe4f1,
    0x11d3,
    [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81],
);

#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct SystemTable {
    pub hdr: TableHeader,
    pub firmware_vendor: *const u16,
    pub firmware_revision: u32,
    pub console_in_handle: Handle,
    pub con_in: *mut SimpleTextInput,
    pub console_out_handle: Handle,
    pub con_out: *mut SimpleTextOutput,
    pub standard_error_handle: Handle,
    pub std_err: *mut SimpleTextOutput,
    pub runtime_services: *mut RuntimeServices,
    pub boot_services: *mut BootServices,
    pub number_of_table_entries: usize,
    pub configuration_table: *const ConfigurationTable,
}

#[repr(C)]
pub struct ConfigurationTable {
    pub vendor_guid: Guid,
    pub vendor_table: *const c_void,
}

type Unused = usize;

#[repr(C)]
pub struct BootServices {
    pub hdr: TableHeader,
    pub raise_tpl: Unused,
    pub restore_tpl: Unused,
    pub allocate_pages: unsafe extern "efiapi" fn(u32, u32, usize, *mut u64) -> Status,
    pub free_pages: unsafe extern "efiapi" fn(u64, usize) -> Status,
    pub get_memory_map: unsafe extern "efiapi" fn(
        *mut usize,
        *mut MemoryDescriptor,
        *mut usize,
        *mut usize,
        *mut u32,
    ) -> Status,
    pub allocate_pool: unsafe extern "efiapi" fn(u32, usize, *mut *mut c_void) -> Status,
    pub free_pool: unsafe extern "efiapi" fn(*mut c_void) -> Status,
    pub create_event: unsafe extern "efiapi" fn(u32, usize, usize, usize, *mut Event) -> Status,
    pub set_timer: unsafe extern "efiapi" fn(Event, u32, u64) -> Status,
    pub wait_for_event: unsafe extern "efiapi" fn(usize, *const Event, *mut usize) -> Status,
    pub signal_event: Unused,
    pub close_event: unsafe extern "efiapi" fn(Event) -> Status,
    pub check_event: unsafe extern "efiapi" fn(Event) -> Status,
    pub install_protocol_interface: Unused,
    pub reinstall_protocol_interface: Unused,
    pub uninstall_protocol_interface: Unused,
    pub handle_protocol: unsafe extern "efiapi" fn(Handle, *const Guid, *mut *mut c_void) -> Status,
    pub reserved: Unused,
    pub register_protocol_notify: Unused,
    pub locate_handle: Unused,
    pub locate_device_path: Unused,
    pub install_configuration_table: Unused,
    pub load_image: Unused,
    pub start_image: Unused,
    pub exit: Unused,
    pub unload_image: Unused,
    pub exit_boot_services: Unused,
    pub get_next_monotonic_count: Unused,
    pub stall: unsafe extern "efiapi" fn(usize) -> Status,
    pub set_watchdog_timer: unsafe extern "efiapi" fn(usize, u64, usize, *const u16) -> Status,
    pub connect_controller: Unused,
    pub disconnect_controller: Unused,
    pub open_protocol: Unused,
    pub close_protocol: Unused,
    pub open_protocol_information: Unused,
    pub protocols_per_handle: Unused,
    pub locate_handle_buffer: Unused,
    pub locate_protocol:
        unsafe extern "efiapi" fn(*const Guid, *const c_void, *mut *mut c_void) -> Status,
}

/// `EFI_ALLOCATE_TYPE`.
pub const ALLOCATE_ANY_PAGES: u32 = 0;
pub const ALLOCATE_MAX_ADDRESS: u32 = 1;
pub const ALLOCATE_ADDRESS: u32 = 2;
/// `EFI_MEMORY_TYPE`: EfiLoaderData, EfiConventionalMemory.
pub const LOADER_DATA: u32 = 2;
pub const CONVENTIONAL_MEMORY: u32 = 7;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MemoryDescriptor {
    pub kind: u32,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub number_of_pages: u64,
    pub attribute: u64,
}

#[repr(C)]
pub struct RuntimeServices {
    pub hdr: TableHeader,
    pub unused: [Unused; 10],
    pub reset_system: unsafe extern "efiapi" fn(u32, Status, usize, *const c_void) -> !,
}

/// `EFI_RESET_TYPE`.
pub const RESET_COLD: u32 = 0;
pub const RESET_WARM: u32 = 1;
pub const RESET_SHUTDOWN: u32 = 2;

#[repr(C)]
pub struct InputKey {
    pub scan_code: u16,
    pub unicode_char: u16,
}

#[repr(C)]
pub struct SimpleTextInput {
    pub reset: Unused,
    pub read_key_stroke: unsafe extern "efiapi" fn(*mut SimpleTextInput, *mut InputKey) -> Status,
    pub wait_for_key: Event,
}

#[repr(C)]
pub struct SimpleTextOutput {
    pub reset: Unused,
    pub output_string: unsafe extern "efiapi" fn(*mut SimpleTextOutput, *const u16) -> Status,
}

#[repr(C)]
pub struct LoadedImage {
    pub revision: u32,
    pub parent_handle: Handle,
    pub system_table: *mut SystemTable,
    pub device_handle: Handle,
}

#[repr(C)]
pub struct SimpleFileSystem {
    pub revision: u64,
    pub open_volume: unsafe extern "efiapi" fn(*mut SimpleFileSystem, *mut *mut File) -> Status,
}

#[repr(C)]
pub struct File {
    pub revision: u64,
    pub open: unsafe extern "efiapi" fn(*mut File, *mut *mut File, *const u16, u64, u64) -> Status,
    pub close: unsafe extern "efiapi" fn(*mut File) -> Status,
    pub delete: unsafe extern "efiapi" fn(*mut File) -> Status,
    pub read: unsafe extern "efiapi" fn(*mut File, *mut usize, *mut c_void) -> Status,
    pub write: unsafe extern "efiapi" fn(*mut File, *mut usize, *const c_void) -> Status,
    pub get_position: unsafe extern "efiapi" fn(*mut File, *mut u64) -> Status,
    pub set_position: unsafe extern "efiapi" fn(*mut File, u64) -> Status,
    pub get_info: Unused,
    pub set_info: Unused,
    pub flush: unsafe extern "efiapi" fn(*mut File) -> Status,
}

pub const FILE_MODE_READ: u64 = 0x1;
pub const FILE_MODE_WRITE: u64 = 0x2;
pub const FILE_MODE_CREATE: u64 = 0x8000_0000_0000_0000;
pub const FILE_DIRECTORY: u64 = 0x10;

#[repr(C)]
pub struct MpServices {
    pub get_number_of_processors:
        unsafe extern "efiapi" fn(*mut MpServices, *mut usize, *mut usize) -> Status,
    pub get_processor_info:
        unsafe extern "efiapi" fn(*mut MpServices, usize, *mut ProcessorInformation) -> Status,
    pub startup_all_aps: Unused,
    pub startup_this_ap: unsafe extern "efiapi" fn(
        *mut MpServices,
        unsafe extern "efiapi" fn(*mut c_void),
        usize,
        Event,
        usize,
        *mut c_void,
        *mut u8,
    ) -> Status,
    pub switch_bsp: Unused,
    pub enable_disable_ap: Unused,
    pub who_am_i: unsafe extern "efiapi" fn(*mut MpServices, *mut usize) -> Status,
}

/// `EFI_PROCESSOR_INFORMATION` without the extended part, which is filled
/// only when `CPU_V2_EXTENDED_TOPOLOGY` is asked for (it is not); the buffer
/// passed is large enough for it anyway.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct ProcessorInformation {
    pub processor_id: u64,
    pub status_flag: u32,
    pub package: u32,
    pub core: u32,
    pub thread: u32,
    pub extended: [u32; 6],
}

pub const PROCESSOR_AS_BSP_BIT: u32 = 0x1;
pub const PROCESSOR_ENABLED_BIT: u32 = 0x2;
