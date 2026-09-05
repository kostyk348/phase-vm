//! Минимальный репро: один IORING_OP_READ в Vec-буфер.
use phase_vm::uring::Uring;
use std::os::unix::io::AsRawFd;

fn main() {
    std::fs::write("/tmp/rr.bin", vec![7u8; 100]).unwrap();
    let f = std::fs::File::open("/tmp/rr.bin").unwrap();
    let mut buf = vec![0u8; 100];
    let mut ur = Uring::new(8).unwrap();
    ur.enqueue_read(f.as_raw_fd(), buf.as_mut_ptr(), 100, 0, 42);
    ur.submit().unwrap();
    let c = ur.wait(1);
    println!("completion: tag={} res={}", c[0].0, c[0].1);
    println!("buf[0..5]={:?}", &buf[0..5]);
}
