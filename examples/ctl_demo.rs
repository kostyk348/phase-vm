//! L3 демо: data-dependent цикл с decision-журналом.
//!
//! Умножение через `wz` (число итераций = данные, rep не смог бы):
//! журнал = ОДНА запись на весь цикл (O(решений), не O(шагов), не O(байт)).
//! reverse доверяет журналу и откатывает без пересчёта условий.

use std::time::Instant;

use phase_vm::ctl;
use phase_vm::state::State;

fn main() {
    // граница: r0=множимое, r1=множитель(данные!), r2=0 (результат), r3=0 (счётчик)
    let prog = ctl::parse(
        "add r3 r1\n\
         wz r3\n\
           add r2 r0\n\
           dec r3\n\
         end\n",
        8,
    )
    .unwrap();

    let mul = 500_000u64;
    let mut s = State::new(8, 0);
    s.regs[0] = 7;
    s.regs[1] = mul;
    let boundary = s.clone();

    let mut journal = Vec::new();
    let mut steps = 0u64;
    let t0 = Instant::now();
    ctl::forward(&mut s, &prog, &mut journal, &mut steps).unwrap();
    let fwd = t0.elapsed();
    assert_eq!(s.regs[2], 7 * mul, "r2 != 7*mul");
    assert_eq!(s.regs[3], 0, "счётчик обнулён");

    let jbytes = journal.len() * std::mem::size_of::<u64>();
    let naive_trace_bytes = steps * std::mem::size_of::<u64>() as u64;
    println!("L3 decision-журнал (умножение через wz, множитель={mul}):");
    println!("итераций цикла: {mul} (данные! rep статически не смог бы)");
    println!(
        "журнал: {} записей = {jbytes} B   (наивный трейс шагов: {naive_trace_bytes} B)",
        journal.len()
    );
    println!(
        "forward: {steps} листьев за {:?} ({:.2} ns/leaf)",
        fwd,
        fwd.as_nanos() as f64 / steps as f64
    );

    // reverse с доверием журналу
    let mut s2 = s.clone();
    let mut steps2 = 0u64;
    let t1 = Instant::now();
    ctl::reverse(&mut s2, &prog, &mut journal, &mut steps2).unwrap();
    let rev = t1.elapsed();
    assert_eq!(s2, boundary, "F⁻¹(F(S)) != граница");
    assert!(journal.is_empty(), "журнал исчерпан — ноль следов");
    println!(
        "reverse: {steps2} листьев за {:?} — состояние == граница, журнал пуст",
        rev
    );

    // ifnz внутри цикла: честная цена — решение на каждую итерацию
    let prog2 = ctl::parse(
        "add r3 r1\n\
         wz r3\n\
           ifnz r0\n\
             add r2 r4\n\
           end\n\
           dec r3\n\
         end\n",
        8,
    )
    .unwrap();
    let mut s3 = State::new(8, 0);
    s3.regs[0] = 1;
    s3.regs[1] = 1_000;
    s3.regs[4] = 3;
    let b3 = s3.clone();
    let mut j3 = Vec::new();
    let mut st3 = 0;
    ctl::forward(&mut s3, &prog2, &mut j3, &mut st3).unwrap();
    let n3 = j3.len();
    ctl::reverse(&mut s3, &prog2, &mut j3, &mut st3).unwrap();
    assert_eq!(s3, b3);
    println!(
        "ifnz внутри wz: журнал = {} записей на 1000 итераций (по одной на ветку) — O(решений)",
        n3
    );
}
