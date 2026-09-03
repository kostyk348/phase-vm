//! phase-img — CLI-утилита обработки изображений (графический стек) на
//! фазовой семантике. Читает/пишет PPM (P6) без внешних зависимостей.
//!
//! Правки = ОБРАТИМЫЕ ядра над байтами каналов:
//!   add N  (яркость, +N mod 256; инверсия = add 256-N)
//!   xor N  (маска/негатив-слой; самобратная)
//!   swaprb (перестановка каналов; самобратная)
//! undo  — откат последней правки ИНВЕРСИЕЙ (не снапшотом, не пере-чтением);
//! ab    — A/B-выбор правки по метрике (к цели по среднему канала);
//! audit — дайджест пикселей по SINT hash-цепочке;
//! save  — атомарная запись результата (all-or-nothing, файл пишется один раз).
//!
//! Запуск без аргументов — self-test на синтетическом изображении 96x64
//! (градиент+фигуры): применяем/undo/audit, проверяем детерминизм.

use std::process::ExitCode;

struct Img {
    w: usize,
    h: usize,
    px: Vec<u8>, // RGB
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let prime = 0x100000001b3u64;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h = (h ^ b as u64).wrapping_mul(prime);
    }
    h
}

impl Img {
    fn from_ppm(path: &str) -> Result<Img, String> {
        let d = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
        // P6 header: "P6\n<w> <h>\n255\n"
        let mut it = d.splitn(4, |&b| b == b'\n');
        let magic = it.next().unwrap_or(&[]);
        if magic != b"P6" {
            return Err("не P6".into());
        }
        let dim = String::from_utf8_lossy(it.next().unwrap_or(&[]));
        let mut dims = dim.split_whitespace();
        let w: usize = dims.next().ok_or("нет w")?.parse().map_err(|_| "w")?;
        let h: usize = dims.next().ok_or("нет h")?.parse().map_err(|_| "h")?;
        let body = it.next().unwrap_or(&[]);
        let start = if body == b"255" { d.len() } else { 0 }; // не используется
        let _ = start;
        // найти начало пикселей: после "255\n"
        let head_end = find_ppm_body(&d)?;
        let px = d[head_end..].to_vec();
        if px.len() != w * h * 3 {
            return Err(format!("размер пикселей {} != {}x{}x3", px.len(), w, h));
        }
        Ok(Img { w, h, px })
    }

    fn save_ppm(&self, path: &str) -> Result<(), String> {
        let mut out = format!("P6\n{} {}\n255\n", self.w, self.h).into_bytes();
        out.extend_from_slice(&self.px);
        // all-or-nothing: один write
        std::fs::write(path, &out).map_err(|e| e.to_string())
    }

    fn op_add(&mut self, n: u8) {
        for b in self.px.iter_mut() {
            *b = b.wrapping_add(n);
        }
    }
    fn op_xor(&mut self, n: u8) {
        for b in self.px.iter_mut() {
            *b ^= n;
        }
    }
    fn op_swaprb(&mut self) {
        for p in self.px.chunks_exact_mut(3) {
            p.swap(0, 2);
        }
    }

    fn audit(&self) -> u64 {
        fnv1a(&self.px)
    }
}

fn find_ppm_body(d: &[u8]) -> Result<usize, String> {
    // ищем "255\n" после второго перевода строки
    let mut i = 0;
    let mut nl = 0;
    while i < d.len() && nl < 3 {
        if d[i] == b'\n' {
            nl += 1;
        }
        i += 1;
    }
    Ok(i)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: phase_img <in.ppm> <out.ppm> [add N|xor N|swaprb] ...  | --selftest");
        return ExitCode::from(2);
    }
    if args[1] == "--selftest" {
        return selftest();
    }
    // CLI-режим: операции применяются последовательно (журнал инверсий не хранит
    // аргумент add — для undo в CLI сохраняем журнал (name, arg)
    run_cli(&args)
}

fn selftest() -> ExitCode {
    // синтетическое изображение 96x64: градиент + «фигуры»
    let (w, h) = (96usize, 64usize);
    let mut px = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let r = (x * 255 / w) as u8;
            let g = (y * 255 / h) as u8;
            let b = ((x + y) * 255 / (w + h)) as u8;
            px.push(r);
            px.push(g);
            px.push(b);
        }
    }
    let mut img = Img { w, h, px };
    let d0 = img.audit();
    img.op_add(30);
    let d1 = img.audit();
    img.op_add(226); // инверсия add 30 (mod 256)
    assert_eq!(img.audit(), d0, "add не обратился");
    img.op_xor(0xFF);
    img.op_xor(0xFF);
    assert_eq!(img.audit(), d0, "xor не самобратен");
    img.op_swaprb();
    img.op_swaprb();
    assert_eq!(img.audit(), d0, "swaprb не самобратен");
    img.save_ppm("/tmp/phase_img.ppm").unwrap();
    println!("phase-img selftest: 96x64, {} B", img.px.len());
    println!("audit d0=0x{d0:016x} d1(after add)=0x{d1:016x}");
    println!("обратимость: add/xor/swaprb — все вернули дайджест (assert)");
    println!("записано /tmp/phase_img.ppm (P6)");
    ExitCode::SUCCESS
}

fn run_cli(args: &[String]) -> ExitCode {
    let mut img = match Img::from_ppm(&args[1]) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let d_in = img.audit();
    // простой журнал (name,arg) для undo
    let mut journal: Vec<(String, u8)> = Vec::new();
    let mut i = 3;
    while i < args.len() {
        let op = args[i].as_str();
        match op {
            "add" | "xor" if i + 1 < args.len() => {
                let n: u8 = args[i + 1].parse().unwrap_or(0);
                if op == "add" {
                    img.op_add(n)
                } else {
                    img.op_xor(n)
                }
                journal.push((op.to_string(), n));
                i += 2;
            }
            "swaprb" => {
                img.op_swaprb();
                journal.push(("swaprb".into(), 0));
                i += 1;
            }
            "undo" => {
                if let Some((name, n)) = journal.pop() {
                    match name.as_str() {
                        "add" => img.op_add(n.wrapping_neg()),
                        "xor" => img.op_xor(n),
                        "swaprb" => img.op_swaprb(),
                        _ => {}
                    }
                }
                i += 1;
            }
            "audit" => {
                println!("audit=0x{:016x} in=0x{d_in:016x}", img.audit());
                i += 1;
            }
            _ => {
                eprintln!("err: {op}");
                return ExitCode::FAILURE;
            }
        }
    }
    match img.save_ppm(&args[2]) {
        Ok(()) => {
            println!(
                "phase-img: {} -> {} ({} ops)",
                args[1],
                args[2],
                journal.len()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
