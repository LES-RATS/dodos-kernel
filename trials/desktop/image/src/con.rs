//! Output of the bootstrap core: the screen (UEFI console), the serial port
//! when one answers at COM1, and the files of the image's own stick. Only
//! the bootstrap core calls this module: firmware services are not safe to
//! call from other cores.
//!
//! The only file system ever opened is the one on the device the image was
//! loaded from (`LoadedImage.DeviceHandle`): the image writes to no other
//! disk.

use crate::efi::{self, File, InputKey, SimpleFileSystem, SystemTable};
use crate::x86;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::ptr::null_mut;
use dodos_trial_multicore::rt::Mutex;

pub struct Con {
    pub st: *mut SystemTable,
    serial: bool,
    dir: *mut File,
    results: *mut File,
    pending: String,
}

// SAFETY: only the bootstrap core uses the console (module comment).
unsafe impl Send for Con {}

pub static CON: Mutex<Option<Con>> = Mutex::new(None);

const COM1: u16 = 0x3F8;

/// A 16550 at COM1 answers if its scratch register (offset 7) keeps a value.
/// The register layout (scratch at 7, line control at 3, line status at 5,
/// divisor latch behind LCR bit 7) is the 16550's, written from memory: not
/// verified against a data sheet. A wrong probe only loses the serial copy.
fn probe_serial() -> bool {
    // SAFETY: COM1's ports; on a machine without a UART the writes go
    // nowhere and the read returns 0xFF.
    unsafe {
        x86::outb(COM1 + 7, 0x5A);
        if x86::inb(COM1 + 7) != 0x5A {
            return false;
        }
        x86::outb(COM1 + 7, 0xA5);
        if x86::inb(COM1 + 7) != 0xA5 {
            return false;
        }
        // 115200 baud 8N1: divisor latch 1 (DLAB, bit 7 of LCR).
        x86::outb(COM1 + 1, 0);
        x86::outb(COM1 + 3, 0x80);
        x86::outb(COM1, 1);
        x86::outb(COM1 + 1, 0);
        x86::outb(COM1 + 3, 0x03);
        x86::outb(COM1 + 2, 0xC7);
    }
    true
}

/// Writes one byte to COM1, waiting for the transmit holding register
/// (LSR bit 5). Also used by the watchdog on another core.
pub fn serial_byte(b: u8) {
    // SAFETY: COM1's ports, probed before use.
    unsafe {
        for _ in 0..100_000 {
            if x86::inb(COM1 + 5) & 0x20 != 0 {
                break;
            }
        }
        x86::outb(COM1, b);
    }
}

fn utf16(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s
        .chars()
        .map(|c| if (c as u32) < 0x10000 { c as u16 } else { b'?'.into() })
        .collect();
    v.push(0);
    v
}

impl Con {
    fn screen(&mut self, s: &str) {
        let mut text = String::with_capacity(s.len() + 2);
        for c in s.chars() {
            if c == '\n' {
                text.push('\r');
            }
            text.push(c);
        }
        let w = utf16(&text);
        // SAFETY: ConOut is valid while boot services run; `w` ends in 0.
        unsafe {
            let out = (*self.st).con_out;
            ((*out).output_string)(out, w.as_ptr());
        }
        if self.serial {
            for b in text.bytes() {
                serial_byte(b);
            }
        }
    }
}

/// Starts the console and opens the image's own volume.
///
/// # Safety
/// `st` and `image` are the arguments the firmware gave the image.
pub unsafe fn init(st: *mut SystemTable, image: efi::Handle) {
    let mut c = Con {
        st,
        serial: probe_serial(),
        dir: null_mut(),
        results: null_mut(),
        pending: String::new(),
    };
    // SAFETY: boot services are valid; the protocols are the firmware's.
    unsafe {
        let bs = (*st).boot_services;
        let mut li: *mut core::ffi::c_void = null_mut();
        let mut fs: *mut core::ffi::c_void = null_mut();
        let mut root: *mut File = null_mut();
        if ((*bs).handle_protocol)(image, &efi::LOADED_IMAGE_GUID, &mut li) == efi::SUCCESS {
            let dev = (*li.cast::<efi::LoadedImage>()).device_handle;
            if ((*bs).handle_protocol)(dev, &efi::SIMPLE_FILE_SYSTEM_GUID, &mut fs)
                == efi::SUCCESS
            {
                let fs = fs.cast::<SimpleFileSystem>();
                if ((*fs).open_volume)(fs, &mut root) == efi::SUCCESS {
                    let name = utf16("DODOS");
                    let mode = efi::FILE_MODE_READ | efi::FILE_MODE_WRITE | efi::FILE_MODE_CREATE;
                    let mut dir = null_mut();
                    if ((*root).open)(root, &mut dir, name.as_ptr(), mode, efi::FILE_DIRECTORY)
                        == efi::SUCCESS
                    {
                        c.dir = dir;
                    }
                }
            }
        }
    }
    *CON.lock().expect("console") = Some(c);
    let mut con = CON.lock().expect("console");
    let c = con.as_mut().expect("console");
    if !c.dir.is_null() {
        c.results = open(c.dir, "RESULTS.TXT", true);
    }
}

fn open(dir: *mut File, name: &str, append: bool) -> *mut File {
    let n = utf16(name);
    let mut f = null_mut();
    let mode = efi::FILE_MODE_READ | efi::FILE_MODE_WRITE | efi::FILE_MODE_CREATE;
    // SAFETY: `dir` is an open directory of the image's volume.
    unsafe {
        if ((*dir).open)(dir, &mut f, n.as_ptr(), mode, 0) != efi::SUCCESS {
            return null_mut();
        }
        if append {
            ((*f).set_position)(f, u64::MAX);
        }
    }
    f
}

fn write_all(f: *mut File, bytes: &[u8]) -> bool {
    if f.is_null() {
        return false;
    }
    let mut n = bytes.len();
    // SAFETY: `f` is an open file; the buffer is valid for `n` bytes.
    unsafe { ((*f).write)(f, &mut n, bytes.as_ptr().cast()) == efi::SUCCESS && n == bytes.len() }
}

/// Prints a line on the screen and the serial port and queues it for the
/// results file.
pub fn line(s: &str) {
    assert!(crate::run::is_bsp(), "con::line called on another core: {s}");
    let mut con = CON.lock().expect("console");
    let Some(c) = con.as_mut() else { return };
    c.screen(s);
    c.screen("\n");
    c.pending.push_str(s);
    c.pending.push('\n');
}

/// Writes the queued lines to RESULTS.TXT and flushes it.
pub fn flush() {
    let mut con = CON.lock().expect("console");
    let Some(c) = con.as_mut() else { return };
    if c.pending.is_empty() || c.results.is_null() {
        return;
    }
    if write_all(c.results, c.pending.as_bytes()) {
        // SAFETY: an open file.
        unsafe { ((*c.results).flush)(c.results) };
        c.pending.clear();
    }
}

pub fn has_volume() -> bool {
    CON.lock().expect("console").as_ref().is_some_and(|c| !c.dir.is_null())
}

/// Reads a whole file of the image's directory.
pub fn read_file(name: &str) -> Option<Vec<u8>> {
    let con = CON.lock().expect("console");
    let c = con.as_ref()?;
    if c.dir.is_null() {
        return None;
    }
    let n = utf16(name);
    let mut f = null_mut();
    let mut out = Vec::new();
    // SAFETY: an open directory; reads into a buffer of the given size.
    unsafe {
        if ((*c.dir).open)(c.dir, &mut f, n.as_ptr(), efi::FILE_MODE_READ, 0) != efi::SUCCESS {
            return None;
        }
        let mut buf = [0u8; 4096];
        loop {
            let mut len = buf.len();
            if ((*f).read)(f, &mut len, buf.as_mut_ptr().cast()) != efi::SUCCESS || len == 0 {
                break;
            }
            out.extend_from_slice(&buf[..len]);
        }
        ((*f).close)(f);
    }
    Some(out)
}

/// Appends to a file of the image's directory and flushes it to the stick.
pub fn append_file(name: &str, bytes: &[u8]) -> bool {
    let con = CON.lock().expect("console");
    let Some(c) = con.as_ref() else { return false };
    if c.dir.is_null() {
        return false;
    }
    let f = open(c.dir, name, true);
    let ok = write_all(f, bytes);
    if !f.is_null() {
        // SAFETY: an open file.
        unsafe {
            ((*f).flush)(f);
            ((*f).close)(f);
        }
    }
    ok
}

/// Replaces a file of the image's directory.
pub fn replace_file(name: &str, bytes: &[u8]) -> bool {
    let con = CON.lock().expect("console");
    let Some(c) = con.as_ref() else { return false };
    if c.dir.is_null() {
        return false;
    }
    let old = open(c.dir, name, false);
    if !old.is_null() {
        // SAFETY: an open file; Delete also closes it.
        unsafe { ((*old).delete)(old) };
    }
    let f = open(c.dir, name, false);
    let ok = write_all(f, bytes);
    if !f.is_null() {
        // SAFETY: an open file.
        unsafe {
            ((*f).flush)(f);
            ((*f).close)(f);
        }
    }
    ok
}

fn st() -> *mut SystemTable {
    CON.lock().expect("console").as_ref().map_or(null_mut(), |c| c.st)
}

/// The key pressed, if any (a printable character, upper-cased).
pub fn key() -> Option<char> {
    let st = st();
    let mut k = InputKey {
        scan_code: 0,
        unicode_char: 0,
    };
    // SAFETY: ConIn is valid while boot services run.
    let r = unsafe { ((*(*st).con_in).read_key_stroke)((*st).con_in, &mut k) };
    (r == efi::SUCCESS).then(|| char::from_u32(u32::from(k.unicode_char)).unwrap_or('?').to_ascii_uppercase())
}

/// Waits up to `ms` for a key.
pub fn wait_key(ms: u64) -> Option<char> {
    while key().is_some() {}
    for _ in 0..ms / 10 {
        if let Some(k) = key() {
            return Some(k);
        }
        stall_us(10_000);
    }
    None
}

/// The firmware's busy wait (boot services Stall).
pub fn stall_us(us: usize) {
    let st = st();
    // SAFETY: boot services are valid.
    unsafe { ((*(*st).boot_services).stall)(us) };
}

/// Arms (or with 0 disarms) the firmware watchdog, which resets the machine
/// if it is not re-armed within `secs` (boot services SetWatchdogTimer).
pub fn watchdog(secs: usize) {
    let st = st();
    // SAFETY: boot services are valid; code 0x10000 is above the range
    // reserved by the specification for its own codes (EDK2 UefiSpec.h).
    unsafe { ((*(*st).boot_services).set_watchdog_timer)(secs, 0x10000, 0, core::ptr::null()) };
}

/// Formats into a line: `out!("R x n={}", n)`.
#[macro_export]
macro_rules! out {
    ($($t:tt)*) => {
        $crate::con::line(&alloc::format!($($t)*))
    };
}

/// A `fmt::Write` sink for building a line.
pub struct LineBuf(pub String);

impl Write for LineBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.push_str(s);
        Ok(())
    }
}
