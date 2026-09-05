//! Минимальный io_uring рантайм (сырые syscalls, 0 зависимостей).
//! Чтения через IORING_OP_READV (opcode 1) с одним iovec на операцию.

use std::io;

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

#[repr(C)]
struct Iovec {
    base: *mut u8,
    len: usize,
}

const OFF_SQ: u64 = 0;
const OFF_CQ: u64 = 0x8000000;
const OFF_SQES: u64 = 0x10000000;
const OP_READV: u8 = 1;

fn sc3(n: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let r: i64;
    unsafe {
        std::arch::asm!("syscall", inlateout("rax") n as i64 => r, in("rdi") a1, in("rsi") a2, in("rdx") a3, lateout("rcx") _, lateout("r11") _);
    }
    r
}
fn sc6(n: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
    let r: i64;
    unsafe {
        std::arch::asm!("syscall", inlateout("rax") n as i64 => r, in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4, in("r8") a5, in("r9") a6, lateout("rcx") _, lateout("r11") _);
    }
    r
}

fn mmap_(len: usize, fd: i32, off: u64) -> io::Result<*mut u8> {
    let r = sc6(9, 0, len as u64, 3, 1, fd as u64, off);
    if r <= 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r as *mut u8)
    }
}

pub struct Uring {
    fd: i32,
    tail: *mut u32,
    mask: *mut u32,
    array: *mut u32,
    sqes: *mut u8,
    cq_head: *mut u32,
    cq_tail: *mut u32,
    cq_mask: *mut u32,
    cqes: *mut u8,
    pending: usize,
    entries: usize,
    /// iovec-буферы живут до завершения батча (READV читает их при обработке)
    iovs: Vec<Iovec>,
}

impl Uring {
    pub fn new(entries: usize) -> io::Result<Uring> {
        let mut p = Params {
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
        let fd = sc3(425, entries as u64, &mut p as *mut Params as u64, 0);
        if fd < 0 {
            return Err(io::Error::from_raw_os_error(-fd as i32));
        }
        let pr = |n: usize| (n + 4095) & !4095;
        let sq_ring = mmap_(pr(p.sq_off.array as usize + entries * 4), fd as i32, OFF_SQ)?;
        let cq_ring = mmap_(pr(p.cq_off.cqes as usize + entries * 16), fd as i32, OFF_CQ)?;
        let sqes = mmap_(entries * 64, fd as i32, OFF_SQES)?;
        unsafe {
            Ok(Uring {
                fd: fd as i32,
                tail: sq_ring.add(p.sq_off.tail as usize) as *mut u32,
                mask: sq_ring.add(p.sq_off.ring_mask as usize) as *mut u32,
                array: sq_ring.add(p.sq_off.array as usize) as *mut u32,
                sqes,
                cq_head: cq_ring.add(p.cq_off.head as usize) as *mut u32,
                cq_tail: cq_ring.add(p.cq_off.tail as usize) as *mut u32,
                cq_mask: cq_ring.add(p.cq_off.ring_mask as usize) as *mut u32,
                cqes: cq_ring.add(p.cq_off.cqes as usize),
                pending: 0,
                entries,
                iovs: Vec::with_capacity(entries),
            })
        }
    }

    /// Поставить в очередь чтение файла в буфер (один iovec).
    pub fn enqueue_read(&mut self, fd: i32, buf: *mut u8, len: usize, off: u64, tag: u64) {
        assert!(self.pending < self.entries);
        let tail = unsafe { *self.tail };
        let idx = (tail as usize) & (unsafe { *self.mask } as usize);
        let sqe = unsafe { self.sqes.add(idx * 64) };
        // iovec живёт в self.iovs (стабильный адрес зарезервирован)
        let iov = Iovec { base: buf, len };
        self.iovs.push(iov);
        let iov_ptr = self.iovs.as_ptr();
        unsafe {
            std::ptr::write_bytes(sqe, 0, 64);
            *sqe.add(0) = OP_READV;
            *(sqe.add(4) as *mut i32) = fd;
            *(sqe.add(8) as *mut u64) = off;
            *(sqe.add(16) as *mut u64) = iov_ptr.add(self.iovs.len() - 1) as u64;
            *(sqe.add(24) as *mut u32) = 1;
            *(sqe.add(32) as *mut u64) = tag;
            *self.array.add(idx) = idx as u32;
            *self.tail = tail.wrapping_add(1);
        }
        self.pending += 1;
    }

    /// Отправить все поставленные операции одним io_uring_enter.
    pub fn submit(&mut self) -> io::Result<()> {
        let n = self.pending;
        if n == 0 {
            return Ok(());
        }
        let r = sc6(426, self.fd as u64, n as u64, 0, 0, 0, 0);
        if r < 0 {
            return Err(io::Error::from_raw_os_error(-r as i32));
        }
        self.pending = 0;
        Ok(())
    }

    /// Дождаться n завершений. Возвращает (tag, результат).
    pub fn wait(&mut self, n: usize) -> Vec<(u64, i32)> {
        let mut out = Vec::with_capacity(n);
        let mut spins = 0u64;
        while out.len() < n {
            let mask = unsafe { *self.cq_mask } as usize;
            let tail = unsafe { *self.cq_tail };
            let mut head = unsafe { *self.cq_head };
            while head != tail && out.len() < n {
                let idx = (head as usize) & mask;
                let cqe = unsafe { self.cqes.add(idx * 16) };
                let tag = unsafe { *(cqe as *const u64) };
                let res = unsafe { *(cqe.add(8) as *const i32) };
                out.push((tag, res));
                head = head.wrapping_add(1);
            }
            unsafe { *self.cq_head = head };
            spins += 1;
            if spins > 2_000_000_000 {
                panic!("uring wait завис: got={} need={n}", out.len());
            }
            std::hint::spin_loop();
        }
        // iovec-буферы этого батча больше не нужны
        self.iovs.clear();
        out
    }
}
