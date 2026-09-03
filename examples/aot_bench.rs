//! AOT (P2): компиляция обратимого ядра в нативный код и замер против
//! интерпретатора. Требует `cc` в PATH.
//!
//! Генерируем C (src/aot.rs), компилируем `cc -O2 -shared`, грузим dlopen-ом
//! (голый FFI, без внешних крейтов), сравниваем ns/leaf и сверяем результат.

use std::ffi::CString;
use std::time::Instant;

use phase_vm::aot;
use phase_vm::machine::{count_leaves, reverse_all, run_forward};
use phase_vm::program::parse;
use phase_vm::state::State;

const SRC: &str = "\
rep 200000
  add r0 r1
  rotl r1 7
  xor r1 r0
  swp r0 r1
  add r2 r3
  rotl r3 11
  xor r3 r2
  swp r2 r3
  add r0 r4
  xor r4 r2
end
";

#[link(name = "dl")]
extern "C" {
    fn dlopen(
        name: *const std::os::raw::c_char,
        flags: std::os::raw::c_int,
    ) -> *mut std::os::raw::c_void;
    fn dlsym(
        h: *mut std::os::raw::c_void,
        s: *const std::os::raw::c_char,
    ) -> *mut std::os::raw::c_void;
}

type Fn2 = unsafe extern "C" fn(*mut u64, *mut u64, u64);

fn load(path: &str, sym: &str) -> Fn2 {
    unsafe {
        let cpath = CString::new(path).unwrap();
        let h = dlopen(cpath.as_ptr(), 2); // RTLD_NOW
        assert!(!h.is_null(), "dlopen failed");
        let csym = CString::new(sym).unwrap();
        let f = dlsym(h, csym.as_ptr());
        assert!(!f.is_null(), "dlsym {sym} failed");
        std::mem::transmute::<*mut std::os::raw::c_void, Fn2>(f)
    }
}

fn main() {
    let which = std::process::Command::new("cc")
        .arg("--version")
        .output()
        .is_ok();
    if !which {
        eprintln!("нет cc в PATH — AOT недоступен");
        std::process::exit(2);
    }

    let nodes = parse(SRC, 8).unwrap().nodes;
    assert!(
        phase_vm::check::check(&nodes).reversible(),
        "ядро должно быть чистым"
    );
    let leaves = count_leaves(&nodes);

    let c_src = aot::codegen(&nodes);
    let c_path = "/tmp/phase_aot.c";
    let so_path = "/tmp/libphase_aot.so";
    std::fs::write(c_path, &c_src).unwrap();
    let st = std::process::Command::new("cc")
        .args(["-O2", "-shared", "-fPIC", "-o", so_path, c_path])
        .status()
        .unwrap();
    assert!(st.success(), "cc failed");

    let fwd_native = load(so_path, "phase_fwd");
    let rev_native = load(so_path, "phase_rev");

    let mut s = State::random(8, 0, 42);
    let orig = s.clone();

    // корректность: native fwd == interpreter fwd; native rev возвращает к orig
    let mut a = s.clone();
    run_forward(&mut a, &nodes).unwrap();
    let mut b = s.clone();
    unsafe { fwd_native(b.regs.as_mut_ptr(), b.mem.as_mut_ptr(), b.mem.len() as u64) }
    assert_eq!(a.regs, b.regs, "native fwd != interpreter fwd");
    let mut c = b.clone();
    unsafe { rev_native(c.regs.as_mut_ptr(), c.mem.as_mut_ptr(), c.mem.len() as u64) }
    assert_eq!(c.regs, orig.regs, "native rev не вернул к началу");

    // скорость
    let rounds = 300u64;
    let t0 = Instant::now();
    for _ in 0..rounds {
        unsafe { fwd_native(s.regs.as_mut_ptr(), s.mem.as_mut_ptr(), s.mem.len() as u64) }
    }
    let nat_fwd = t0.elapsed().as_nanos() as f64 / (rounds as f64 * leaves as f64);

    let t1 = Instant::now();
    for _ in 0..rounds {
        run_forward(&mut s, &nodes).unwrap();
    }
    let int_fwd = t1.elapsed().as_nanos() as f64 / (rounds as f64 * leaves as f64);

    let t2 = Instant::now();
    for _ in 0..rounds {
        unsafe { rev_native(s.regs.as_mut_ptr(), s.mem.as_mut_ptr(), s.mem.len() as u64) }
    }
    let nat_rev = t2.elapsed().as_nanos() as f64 / (rounds as f64 * leaves as f64);

    let t3 = Instant::now();
    for _ in 0..rounds {
        reverse_all(&mut s, &nodes).unwrap();
    }
    let int_rev = t3.elapsed().as_nanos() as f64 / (rounds as f64 * leaves as f64);

    println!("AOT (P2): leaves={leaves} (rep 200000), cc -O2, dlopen");
    println!(
        "forward : native {nat_fwd:6.2} ns/leaf | interpreter {int_fwd:6.2} | {:.1}×",
        int_fwd / nat_fwd
    );
    println!(
        "reverse : native {nat_rev:6.2} ns/leaf | interpreter {int_rev:6.2} | {:.1}×",
        int_rev / nat_rev
    );
    println!("корректность: native == interpreter (fwd), native rev == orig (assert)");
}
