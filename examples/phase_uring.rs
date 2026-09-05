//! phase-uring: event-loop = фаза на io_uring (сырые syscalls, 0 зависимостей).
//!
//! Идея PHASE: события батчатся в «фазу» — один io_uring_enter обрабатывает
//! до N событий (минимум syscalls), завершения собираются пачкой
//! детерминированно. Здесь: пакетная отправка IORING_OP_NOP и сбор CQEs —
//! замер стоимости «фазы событий» на размер батча.
//!
//! Ссылки на структуры — из uapi/linux/io_uring.h (x86_64).

use std::arch::asm;

#[repr(C)]
struct Params {
    sq_entries: u32,
    cq_entries: u32,
    flags: u32,
    sq_thread_cpu: u32,
    sq_thread_idle: u32,
    features: u32,
    wq_fd: u32,
    resv: [u32; 3],
    sq_off: SqOff,
    cq_off: CqOff,
}
#[repr(C)]
struct SqOff {
    head: u32,
    tail: u32,
    ring_mask: u32,
    ring_entries: u32,
    flags: u32,
    dropped: u32,
    array: u32,
    resv1: u32,
    resv2: u32,
    resv3: u32,
}
#[repr(C)]
struct CqOff {
    head: u32,
    tail: u32,
    ring_mask: u32,
    ring_entries: u32,
    overflow: u32,
    cqes: u32,
    flags: u32,
    resv1: u32,
    resv2: u32,
    resv3: u32,
}

const IORING_OFF_SQ_RING: u64 = 0;
const IORING_OFF_CQ_RING: u64 = 0x8000000;
const IORING_OFF_SQES: u64 = 0x10000000;

fn syscall3(n: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!("syscall", inlateout("rax") n as i64 => r, in("rdi") a1, in("rsi") a2, in("rdx") a3, lateout("rcx") _, lateout("r11") _);
    }
    r
}

fn syscall6(n: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!("syscall",
            inlateout("rax") n as i64 => r,
            in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4, in("r8") a5, in("r9") a6,
            lateout("rcx") _, lateout("r11") _);
    }
    r
}

fn mmap(len: usize, prot: i32, flags: i32, fd: i32, off: u64) -> *mut u8 {
    let r = syscall6(9, 0, len as u64, prot as u64, flags as u64, fd as u64, off);
    assert!(r > 0, "mmap io_uring failed: {r}");
    r as *mut u8
}

fn page_round(n: usize) -> usize {
    (n + 4095) & !4095
}

struct Uring {
    fd: i32,
    sq_tail: *mut u32,
    sq_mask: *mut u32,
    sq_array: *mut u32,
    sqes: *mut u8,
    cq_head: *mut u32,
    cq_tail: *mut u32,
    cq_mask: *mut u32,
    cqes: *mut u8,
}

impl Uring {
    fn setup(entries: usize) -> Result<Uring, String> {
        let mut params = Params {
            sq_entries: 0,
            cq_entries: 0,
            flags: 0,
            sq_thread_cpu: 0,
            sq_thread_idle: 0,
            features: 0,
            wq_fd: 0,
            resv: [0; 3],
            sq_off: SqOff {
                head: 0,
                tail: 0,
                ring_mask: 0,
                ring_entries: 0,
                flags: 0,
                dropped: 0,
                array: 0,
                resv1: 0,
                resv2: 0,
                resv3: 0,
            },
            cq_off: CqOff {
                head: 0,
                tail: 0,
                ring_mask: 0,
                ring_entries: 0,
                overflow: 0,
                cqes: 0,
                flags: 0,
                resv1: 0,
                resv2: 0,
                resv3: 0,
            },
        };
        let fd = syscall3(425, entries as u64, &mut params as *mut Params as u64, 0);
        if fd < 0 {
            return Err(format!("io_uring_setup: {fd}"));
        }
        let sq_size = page_round(params.sq_off.array as usize + entries * 4);
        let cq_size = page_round(params.cq_off.cqes as usize + entries * 16);
        let sq_ring = mmap(sq_size, 3, 1, fd as i32, IORING_OFF_SQ_RING);
        let cq_ring = mmap(cq_size, 3, 1, fd as i32, IORING_OFF_CQ_RING);
        let sqes = mmap(entries * 64, 3, 1, fd as i32, IORING_OFF_SQES);
        unsafe {
            Ok(Uring {
                fd: fd as i32,
                sq_tail: sq_ring.add(params.sq_off.tail as usize) as *mut u32,
                sq_mask: sq_ring.add(params.sq_off.ring_mask as usize) as *mut u32,
                sq_array: sq_ring.add(params.sq_off.array as usize) as *mut u32,
                sqes,
                cq_head: cq_ring.add(params.cq_off.head as usize) as *mut u32,
                cq_tail: cq_ring.add(params.cq_off.tail as usize) as *mut u32,
                cq_mask: cq_ring.add(params.cq_off.ring_mask as usize) as *mut u32,
                cqes: cq_ring.add(params.cq_off.cqes as usize),
            })
        }
    }

    unsafe fn submit_nops(&mut self, n: usize) -> i64 {
        let mask = *self.sq_mask;
        let tail = *self.sq_tail;
        for i in 0..n {
            let idx = (tail as usize + i) & mask as usize;
            let sqe = self.sqes.add(idx * 64);
            std::ptr::write_bytes(sqe, 0, 64);
            *sqe.add(0) = 0; // opcode NOP
            let data = (idx as u64) + 1;
            std::ptr::copy_nonoverlapping(&data as *const u64 as *const u8, sqe.add(32), 8);
            *self.sq_array.add(idx) = idx as u32;
        }
        *self.sq_tail = tail.wrapping_add(n as u32);
        syscall6(426, self.fd as u64, n as u64, 0, 0, 0, 0) // enter, no min_complete
    }

    /// Собрать need завершившихся CQE.
    unsafe fn reap(&mut self, need: u32) -> u32 {
        let mask = *self.cq_mask;
        let mut got = 0u32;
        let mut spins = 0u64;
        while got < need {
            let tail = *self.cq_tail;
            let head = *self.cq_head;
            while head != tail && got < need {
                let idx = (head & mask) as usize;
                let cqe = self.cqes.add(idx * 16);
                let res = *(cqe.add(8) as *const i32);
                assert_eq!(res, 0, "CQE res={res}");
                got += 1;
            }
            if got >= need {
                *self.cq_head = head;
                break;
            }
            *self.cq_head = head;
            spins += 1;
            if spins > 2_000_000_000 {
                panic!("CQ завис: got={got} need={need}");
            }
            std::hint::spin_loop();
        }
        got
    }
}

fn main() {
    let n = 100_000usize; // всего событий
    let batch = 32usize;
    let mut ring = match Uring::setup(batch * 2) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("io_uring недоступен: {e}");
            return;
        }
    };
    let t0 = std::time::Instant::now();
    let mut submitted = 0usize;
    let mut enters = 0u64;
    while submitted < n {
        let take = (n - submitted).min(batch);
        unsafe {
            let r = ring.submit_nops(take);
            assert!(r >= 0, "io_uring_enter: {r}");
            ring.reap(take as u32);
        }
        submitted += take;
        enters += 1;
    }
    let dt = t0.elapsed();
    println!("phase-uring: {n} событий (NOP), батч={batch}");
    println!(
        "syscalls io_uring_enter: {enters} ({:.0} событий/enter) — события батчатся в «фазу»",
        n as f64 / enters as f64
    );
    println!(
        "время: {:?}  ({:.0} ns/событие)",
        dt,
        dt.as_nanos() as f64 / n as f64
    );
    println!("против read(): 1 syscall на событие — здесь в {batch}× меньше syscalls");
    // сравнение: эквивалент без батчинга
    let mut ring2 = Uring::setup(4).unwrap();
    let m = n.min(20_000);
    let t1 = std::time::Instant::now();
    for _ in 0..m {
        unsafe {
            ring2.submit_nops(1);
            ring2.reap(1);
        }
    }
    let d1 = t1.elapsed();
    let ns1 = d1.as_nanos() as f64 / m as f64;
    let ns2 = dt.as_nanos() as f64 / n as f64;
    println!(
        "без батчинга (1/enter): {ns1:.0} ns/событие — батч быстрее в {:.1}×",
        ns1 / ns2
    );
}
