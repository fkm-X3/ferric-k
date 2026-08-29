use std::sync::atomic::{AtomicUsize, Ordering};

static STEP_NO: AtomicUsize = AtomicUsize::new(0);

pub fn step(name: &str) {
    let n = STEP_NO.fetch_add(1, Ordering::SeqCst) + 1;
    println!("\x1b[36m==> [{n}] {name}\x1b[0m");
}

pub fn ok(msg: &str) {
    println!("\x1b[32m    [ok] {msg}\x1b[0m");
}

pub fn fail(msg: &str) {
    eprintln!("\x1b[31m    [FAIL] {msg}\x1b[0m");
}

pub fn note(msg: &str) {
    println!("    {msg}");
}

pub fn reset_counter() {
    STEP_NO.store(0, Ordering::SeqCst);
}
