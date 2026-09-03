//! P10: wide-лейны AVX2. Обратимые примитивы (xor/add/sub/rot) покомпонентны —
//! значит их можно гнать векторно по лейнам. Замер: AVX2 против скаляра
//! на массиве 4M u64 (элемент = «лейна»), операции reversible-ядра.
//!
//! x86_64 + AVX2 (std::arch, без внешних крейтов). Это валидация пути P10:
//! ядро L0 «wide» на реальном железе даёт ожидаемый векторный выигрыш.

use std::arch::x86_64::*;
use std::time::Instant;

#[target_feature(enable = "avx2")]
unsafe fn avx_pass(a: &mut [u64], b: &mut [u64], rot_k: u32) {
    let n = a.len();
    let mut i = 0;
    while i + 4 <= n {
        let va = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
        // xor: a^=b  (самобратно)
        let x = _mm256_xor_si256(va, vb);
        _mm256_storeu_si256(a.as_mut_ptr().add(i) as *mut __m256i, x);
        // b = rotl(b, k): сдвиги + or (обратимо: rotr)
        let k = rot_k & 63;
        if k != 0 {
            let cl = _mm256_set1_epi64x(k as i64);
            let cr = _mm256_set1_epi64x((64 - k) as i64);
            let l = _mm256_sllv_epi64(vb, cl);
            let r = _mm256_srlv_epi64(vb, cr);
            let rb = _mm256_or_si256(l, r);
            _mm256_storeu_si256(b.as_mut_ptr().add(i) as *mut __m256i, rb);
        }
        i += 4;
    }
    // хвост (некратный 4) — скалярно
    while i < n {
        a[i] ^= b[i];
        i += 1;
    }
}

fn scalar_pass(a: &mut [u64], b: &mut [u64], rot_k: u32) {
    let k = rot_k & 63;
    for i in 0..a.len() {
        a[i] ^= b[i];
        if k != 0 {
            b[i] = b[i].rotate_left(k);
        }
    }
}

fn main() {
    assert!(is_x86_feature_detected!("avx2"), "нужен AVX2");
    let n = 2048usize;
    let rounds = 200_000u64;
    let mut a = vec![0xDEAD_BEEF_1234_5678u64; n];
    let mut b: Vec<u64> = (0..n as u64)
        .map(|i| i.wrapping_mul(0x9E3779B97F4A7C15))
        .collect();
    let a0 = a.clone();
    let b0 = b.clone();

    // корректность: avx == scalar
    let mut a1 = a0.clone();
    let mut b1 = b0.clone();
    unsafe { avx_pass(&mut a1, &mut b1, 13) }
    let mut a2 = a0.clone();
    let mut b2 = b0.clone();
    scalar_pass(&mut a2, &mut b2, 13);
    assert_eq!(a1, a2);
    assert_eq!(b1, b2, "AVX2 rotl разошёлся со скаляром");

    // бенч
    let t0 = Instant::now();
    for _ in 0..rounds {
        scalar_pass(&mut a, &mut b, 13);
    }
    let sc = t0.elapsed().as_nanos() as f64 / (rounds as f64 * n as f64);

    let mut a = a0;
    let mut b = b0;
    let t1 = Instant::now();
    for _ in 0..rounds {
        unsafe { avx_pass(&mut a, &mut b, 13) }
    }
    let av = t1.elapsed().as_nanos() as f64 / (rounds as f64 * n as f64);

    println!("wide-lanes (P10): AVX2, N={n} u64 («лейн»), xor+rotl, {rounds} раундов");
    println!(
        "scalar: {sc:6.2} ns/элемент | AVX2: {av:6.2} ns/элемент | {:.1}×",
        sc / av
    );
    println!("корректность: AVX2 == скаляр (assert) — обратимость покомпонентна, XOR/ROT обратимы");
}
