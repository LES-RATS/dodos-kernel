//! What the trial needs from its host: a clock, one closure run per core, a
//! mutex and a barrier. With the `std` feature these are the standard
//! library's and threads; without it (the bare-metal image of
//! `trials/desktop`) the image provides them through [`install`], so the
//! workloads run unchanged on real cores with real interrupts.

pub use core::time::Duration;
#[cfg(feature = "std")]
pub use std::sync::{Barrier, Mutex};
#[cfg(feature = "std")]
pub use std::time::Instant;

#[cfg(not(feature = "std"))]
pub use bare::{Barrier, Host, Instant, Mutex, host, install};

/// Runs `worker(i)` for `i` in `0..n`, each on its own core (its own thread
/// with `std`), and `main` on the calling one; returns the workers' results
/// in order and `main`'s. Every worker has finished when it returns.
#[cfg(feature = "std")]
pub fn scope<T: Send, R>(
    n: usize,
    worker: impl Fn(usize) -> T + Sync,
    main: impl FnOnce() -> R,
) -> (alloc::vec::Vec<T>, R) {
    std::thread::scope(|s| {
        let worker = &worker;
        let handles: alloc::vec::Vec<_> = (0..n).map(|i| s.spawn(move || worker(i))).collect();
        let r = main();
        let outs = handles
            .into_iter()
            .map(|h| h.join().expect("worker panicked"))
            .collect();
        (outs, r)
    })
}

#[cfg(not(feature = "std"))]
pub use bare::scope;

/// Sleeps the calling thread (`std`) or spins on the clock (bare metal).
pub fn sleep(d: Duration) {
    #[cfg(feature = "std")]
    std::thread::sleep(d);
    #[cfg(not(feature = "std"))]
    {
        let end = Instant::now() + d;
        while Instant::now() < end {
            core::hint::spin_loop();
        }
    }
}

#[cfg(not(feature = "std"))]
mod bare {
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::cell::UnsafeCell;
    use core::ops::{Add, Deref, DerefMut, Sub};
    use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
    use core::time::Duration;

    /// The services of the bare-metal image. `start` must run `f(i)` on
    /// worker core `i` for every `i < n` and return at once; `wait` returns
    /// when every one of them has returned, so `f` never outlives the call
    /// of [`scope`] that passed it.
    pub struct Host {
        pub now_ns: fn() -> u64,
        pub start: fn(usize, &(dyn Fn(usize) + Sync)),
        pub wait: fn(),
        /// A real interrupt taken by the calling core: entry, handler, end
        /// of interrupt and return, paid where the model charges `ipi_ns`.
        pub interrupt: fn(),
        /// The CPU number of the calling core for the "on P" column:
        /// efficiency cores first, as on the M5.
        pub current_cpu: fn() -> Option<usize>,
        pub efficiency_cores: usize,
        pub describe: fn() -> String,
    }

    static HOST: AtomicPtr<Host> = AtomicPtr::new(core::ptr::null_mut());

    pub fn install(h: &'static Host) {
        HOST.store(core::ptr::from_ref(h).cast_mut(), Ordering::Release);
    }

    pub fn host() -> &'static Host {
        let p = HOST.load(Ordering::Acquire);
        assert!(!p.is_null(), "the image did not install its host");
        // SAFETY: only `install` stores a non-null value, from a `&'static`.
        unsafe { &*p }
    }

    /// Nanoseconds from the host's clock.
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
    pub struct Instant(u64);

    impl Instant {
        pub fn now() -> Instant {
            Instant((host().now_ns)())
        }
        pub fn elapsed(&self) -> Duration {
            Instant::now() - *self
        }
        pub fn duration_since(&self, earlier: Instant) -> Duration {
            *self - earlier
        }
        pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
            *self - earlier
        }
    }

    impl Add<Duration> for Instant {
        type Output = Instant;
        fn add(self, d: Duration) -> Instant {
            Instant(self.0 + u64::try_from(d.as_nanos()).unwrap_or(u64::MAX / 2))
        }
    }

    impl core::ops::AddAssign<Duration> for Instant {
        fn add_assign(&mut self, d: Duration) {
            *self = *self + d;
        }
    }

    impl Sub for Instant {
        type Output = Duration;
        fn sub(self, earlier: Instant) -> Duration {
            Duration::from_nanos(self.0.saturating_sub(earlier.0))
        }
    }

    /// A spin lock with the part of `std::sync::Mutex`'s interface the trial
    /// uses. It never poisons; the error types only keep the call sites.
    pub struct Mutex<T> {
        held: AtomicBool,
        v: UnsafeCell<T>,
    }

    // SAFETY: the value is reached only through a guard, and `held` lets one
    // guard exist at a time.
    unsafe impl<T: Send> Sync for Mutex<T> {}

    #[derive(Debug)]
    pub struct WouldBlock;

    pub struct MutexGuard<'a, T> {
        m: &'a Mutex<T>,
    }

    impl<T> Mutex<T> {
        pub const fn new(v: T) -> Mutex<T> {
            Mutex {
                held: AtomicBool::new(false),
                v: UnsafeCell::new(v),
            }
        }
        pub fn try_lock(&self) -> Result<MutexGuard<'_, T>, WouldBlock> {
            self.held
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .map(|_| MutexGuard { m: self })
                .map_err(|_| WouldBlock)
        }
        pub fn lock(&self) -> Result<MutexGuard<'_, T>, WouldBlock> {
            loop {
                if let Ok(g) = self.try_lock() {
                    return Ok(g);
                }
                core::hint::spin_loop();
            }
        }
    }

    impl<T> Deref for MutexGuard<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            // SAFETY: this guard is the only one (`held`).
            unsafe { &*self.m.v.get() }
        }
    }

    impl<T> DerefMut for MutexGuard<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            // SAFETY: this guard is the only one (`held`).
            unsafe { &mut *self.m.v.get() }
        }
    }

    impl<T> Drop for MutexGuard<'_, T> {
        fn drop(&mut self) {
            self.m.held.store(false, Ordering::Release);
        }
    }

    /// A sense-reversing spin barrier.
    pub struct Barrier {
        n: usize,
        count: AtomicUsize,
        sense: AtomicBool,
    }

    impl Barrier {
        pub fn new(n: usize) -> Barrier {
            Barrier {
                n,
                count: AtomicUsize::new(0),
                sense: AtomicBool::new(false),
            }
        }
        pub fn wait(&self) {
            let mine = !self.sense.load(Ordering::Acquire);
            if self.count.fetch_add(1, Ordering::AcqRel) + 1 == self.n {
                self.count.store(0, Ordering::Relaxed);
                self.sense.store(mine, Ordering::Release);
            } else {
                while self.sense.load(Ordering::Acquire) != mine {
                    core::hint::spin_loop();
                }
            }
        }
    }

    pub fn scope<T: Send, R>(
        n: usize,
        worker: impl Fn(usize) -> T + Sync,
        main: impl FnOnce() -> R,
    ) -> (Vec<T>, R) {
        let slots: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
        let body = |i: usize| {
            let t = worker(i);
            *slots[i].lock().expect("slot") = Some(t);
        };
        (host().start)(n, &body);
        let r = main();
        (host().wait)();
        let outs = slots
            .into_iter()
            .map(|s| s.v.into_inner().expect("a worker did not finish"))
            .collect();
        (outs, r)
    }
}
