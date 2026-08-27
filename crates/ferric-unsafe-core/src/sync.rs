//! Blocking primitives for shared boot state: a spin lock and a set-once
//! cell, built directly on `UnsafeCell` + atomics (no external crates).

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// Mutual-exclusion spin lock; [`Spinlock::lock`] spins until it wins.
///
/// Locking twice from the same thread deadlocks by design.
pub struct Spinlock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: `value` is reachable only through guards handed out while
// `locked` is held, so sharing `&Spinlock<T>` across threads is sound when
// the payload may be sent between them.
unsafe impl<T: Send> Sync for Spinlock<T> {}

impl<T> Spinlock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinlockGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinlockGuard { lock: self }
    }

    /// Takes the lock without waiting; `None` while someone else holds it.
    pub fn try_lock(&self) -> Option<SpinlockGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
            .then(|| SpinlockGuard { lock: self })
    }
}

/// Exclusive borrow of a [`Spinlock`] payload; releasing on drop.
pub struct SpinlockGuard<'a, T> {
    lock: &'a Spinlock<T>,
}

impl<T> Deref for SpinlockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: the guard exists only while `locked` is set, which grants
        // exclusive access to `value`.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinlockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: same exclusivity as `deref`.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinlockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

const EMPTY: u8 = 0;
const BUSY: u8 = 1;
const READY: u8 = 2;

/// Set-once cell: [`OnceLock::set`] succeeds exactly once; later readers
/// poll via [`OnceLock::get`] or block in [`OnceLock::wait`].
pub struct OnceLock<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
}

// SAFETY: writes happen only under BUSY (single writer), reads only after
// READY was published with Release, so cross-thread sharing needs the payload
// to be both Send and Sync.
unsafe impl<T: Send + Sync> Sync for OnceLock<T> {}

impl<T> OnceLock<T> {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(EMPTY),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Stores `value`, or returns it unchanged if already initialized.
    pub fn set(&self, value: T) -> Result<(), T> {
        loop {
            match self
                .state
                .compare_exchange(EMPTY, BUSY, Ordering::Acquire, Ordering::Acquire)
            {
                Ok(_) => {
                    // SAFETY: BUSY grants exclusive write access, and no
                    // reader observes the payload before READY is stored.
                    unsafe { (*self.value.get()).write(value) };
                    self.state.store(READY, Ordering::Release);
                    return Ok(());
                }
                Err(BUSY) => {
                    while self.state.load(Ordering::Acquire) == BUSY {
                        core::hint::spin_loop();
                    }
                }
                Err(_) => return Err(value),
            }
        }
    }

    /// The stored value, or `None` before a successful `set`.
    pub fn get(&self) -> Option<&T> {
        match self.state.load(Ordering::Acquire) {
            READY => Some(
                // SAFETY: READY is stored with Release only after the write
                // completes, so Acquire-observing readers see initialized
                // memory.
                unsafe { (*self.value.get()).assume_init_ref() },
            ),
            _ => None,
        }
    }

    /// Spins until a `set` has succeeded anywhere, then yields the value.
    pub fn wait(&self) -> &T {
        loop {
            if let Some(value) = self.get() {
                return value;
            }
            core::hint::spin_loop();
        }
    }
}

impl<T> Default for OnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for OnceLock<T> {
    fn drop(&mut self) {
        if self.state.load(Ordering::Acquire) == READY {
            // SAFETY: READY implies one value was written here and never
            // moved out.
            unsafe { (*self.value.get()).assume_init_drop() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use std::vec::Vec;

    #[test]
    fn primitives_are_const_constructible_in_statics() {
        static SPIN: Spinlock<usize> = Spinlock::new(3);
        static ONCE: OnceLock<usize> = OnceLock::new();

        assert_eq!(*SPIN.lock(), 3);
        assert!(ONCE.get().is_none());
    }

    #[test]
    fn try_lock_refuses_while_held_and_resumes_after_drop() {
        let lock = Spinlock::new(0u32);

        let mut guard = lock.try_lock().expect("free lock must be takeable");
        assert!(lock.try_lock().is_none());
        *guard += 1;
        drop(guard);

        let guard = lock.try_lock().expect("released lock must be retakeable");
        assert_eq!(*guard, 1);
    }

    #[test]
    fn contended_threads_serialize_updates() {
        const THREADS: usize = 4;
        const ITERATIONS: usize = 2_000;

        let counter = Arc::new(Spinlock::new(0usize));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let counter = Arc::clone(&counter);
                thread::spawn(move || {
                    for _ in 0..ITERATIONS {
                        *counter.lock() += 1;
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(*counter.lock(), THREADS * ITERATIONS);
    }

    #[test]
    fn critical_sections_never_interleave() {
        // Two threads toggle an inside-lock flag; interleaving would make a
        // thread observe the flag already set.
        let flag = Arc::new(Spinlock::new(false));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let flag = Arc::clone(&flag);
                thread::spawn(move || {
                    for _ in 0..10_000 {
                        let mut guard = flag.lock();
                        assert!(!*guard);
                        *guard = true;
                        assert!(*guard);
                        *guard = false;
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn once_set_wins_once_and_losers_get_their_value_back() {
        let cell = OnceLock::new();

        assert!(cell.get().is_none());
        assert_eq!(cell.set(5), Ok(()));
        assert_eq!(cell.set(6), Err(6));

        assert_eq!(cell.get(), Some(&5));
        assert_eq!(cell.wait(), &5);
    }

    #[test]
    fn once_wait_observes_a_set_from_another_thread() {
        let cell = Arc::new(OnceLock::<u64>::new());
        let writer = Arc::clone(&cell);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            writer.set(42).unwrap();
        });

        assert_eq!(cell.wait(), &42);
    }

    #[test]
    fn concurrent_setters_produce_exactly_one_winner() {
        const SETTERS: usize = 8;

        let cell = Arc::new(OnceLock::new());
        let handles: Vec<_> = (0..SETTERS)
            .map(|i| {
                let cell = Arc::clone(&cell);
                thread::spawn(move || cell.set(i))
            })
            .collect();

        let winner_count = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().is_ok())
            .filter(|won| *won)
            .count();

        assert_eq!(winner_count, 1);
        assert!(*cell.wait() < SETTERS);
    }

    #[test]
    fn dropping_an_initialized_cell_drops_the_payload_once() {
        struct DropCounter<'a>(&'a std::cell::Cell<usize>);
        impl Drop for DropCounter<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let drops = std::cell::Cell::new(0);
        {
            let cell = OnceLock::new();
            assert!(cell.set(DropCounter(&drops)).is_ok());
        }
        assert_eq!(drops.get(), 1);

        let untouched = std::cell::Cell::new(0);
        {
            let _cell = OnceLock::<DropCounter>::new();
        }
        assert_eq!(untouched.get(), 0);
    }
}
