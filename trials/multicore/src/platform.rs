//! What the host lets us control. Linux: one worker pinned per physical core
//! with `sched_setaffinity`, the second hardware thread left idle (as the
//! kernel does, D-028). macOS: no pinning exists; workers ask for the
//! user-interactive QoS class, which prefers performance cores, and every run
//! records where its threads actually ran.

use std::process::Command;

#[cfg(target_os = "linux")]
mod os {
    unsafe extern "C" {
        fn sched_setaffinity(pid: i32, size: usize, mask: *const u8) -> i32;
        fn sched_getcpu() -> i32;
    }

    pub fn pin(cpu: usize) -> bool {
        let mut mask = [0u8; 128]; // cpu_set_t: 1024 bits
        if cpu >= 1024 {
            return false;
        }
        mask[cpu / 8] |= 1 << (cpu % 8);
        // SAFETY: `mask` is a valid cpu_set_t of the size passed; pid 0 is the
        // calling thread.
        unsafe { sched_setaffinity(0, mask.len(), mask.as_ptr()) == 0 }
    }

    pub fn qos() {}

    pub fn cpu() -> Option<usize> {
        // SAFETY: no arguments; returns -1 on error.
        let c = unsafe { sched_getcpu() };
        usize::try_from(c).ok()
    }
}

#[cfg(target_os = "macos")]
mod os {
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(class: u32, priority: i32) -> i32;
        fn pthread_cpu_number_np(cpu: *mut usize) -> i32;
    }
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

    pub fn pin(_cpu: usize) -> bool {
        false
    }

    pub fn qos() {
        // SAFETY: plain values; affects the calling thread only.
        unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
    }

    pub fn cpu() -> Option<usize> {
        let mut n = 0usize;
        // SAFETY: `n` is a valid out pointer (macOS 11 or later).
        (unsafe { pthread_cpu_number_np(&mut n) } == 0).then_some(n)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod os {
    pub fn pin(_cpu: usize) -> bool {
        false
    }
    pub fn qos() {}
    pub fn cpu() -> Option<usize> {
        None
    }
}

/// Called first by every worker thread.
pub fn prepare_worker(cpu: Option<usize>) {
    if cfg!(miri) {
        return;
    }
    os::qos();
    if let Some(c) = cpu {
        assert!(os::pin(c), "sched_setaffinity to cpu {c} failed");
    }
}

pub fn current_cpu() -> Option<usize> {
    if cfg!(miri) { None } else { os::cpu() }
}

fn sysctl(name: &str) -> Option<String> {
    let out = Command::new("sysctl").args(["-n", name]).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The CPUs to pin workers to, one per physical core, or `None` per worker
/// when the host cannot pin.
pub fn worker_cpus() -> Vec<Option<usize>> {
    if cfg!(target_os = "linux") && !cfg!(miri) {
        let mut cpus = Vec::new();
        for c in 0..1024 {
            let base = format!("/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list");
            let Ok(s) = std::fs::read_to_string(base) else {
                continue;
            };
            let first = s
                .split([',', '-'])
                .next()
                .and_then(|x| x.trim().parse().ok());
            if first == Some(c) {
                cpus.push(Some(c));
            }
        }
        if !cpus.is_empty() {
            return cpus;
        }
    }
    let n = std::thread::available_parallelism().map_or(1, |n| n.get());
    vec![None; n]
}

/// On Apple chips the efficiency cores have the low CPU numbers (checked on
/// the M5: background-QoS threads run on CPUs 0 to 5 only). Elsewhere every
/// CPU counts as preferred.
pub fn efficiency_cores() -> usize {
    if cfg!(target_os = "macos") && !cfg!(miri) {
        let p0 = sysctl("hw.perflevel0.name").unwrap_or_default();
        if !p0.is_empty() {
            return sysctl("hw.perflevel1.logicalcpu")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        }
    }
    0
}

pub fn load_average() -> String {
    if cfg!(target_os = "macos") {
        return sysctl("vm.loadavg").unwrap_or_default();
    }
    std::fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .trim()
        .to_owned()
}

pub fn describe() -> String {
    let model = if cfg!(target_os = "macos") {
        let brand = sysctl("machdep.cpu.brand_string").unwrap_or_default();
        let p = sysctl("hw.perflevel0.logicalcpu").unwrap_or_default();
        let e = sysctl("hw.perflevel1.logicalcpu").unwrap_or_default();
        format!("{brand}, {p} performance + {e} efficiency cores")
    } else {
        std::fs::read_to_string("/proc/cpuinfo")
            .unwrap_or_default()
            .lines()
            .find(|l| l.starts_with("model name"))
            .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_owned())
            .unwrap_or_default()
    };
    format!(
        "{} {}, {model}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}
