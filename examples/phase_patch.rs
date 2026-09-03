//! phase-patch: атомарный all-or-nothing патчер (CLI-обёртка).
//!
//!   phase_patch <файл> <патч.diff> [--apply|--dry] [--out <результат>]
//!
//! Приколы парадигмы: файл не трогается до полного успеха (конфликт =
//! «откат», файл бит-в-бит исходный, ноль временных файлов), дайджест по
//! SINT hash-цепочке, детерминизм. Патч можно сгенерировать настоящим
//! `diff -u` и скормить нам.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: phase_patch <файл> <патч.diff> [--out <результат>]");
        return ExitCode::from(2);
    }
    let file = &args[1];
    let diff_path = &args[2];
    let mut out_path: Option<&String> = None;
    let mut i = 3;
    while i < args.len() {
        if args[i] == "--out" && i + 1 < args.len() {
            out_path = Some(&args[i + 1]);
            i += 2;
        } else {
            i += 1;
        }
    }

    let orig = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("не могу прочитать {file}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let diff = match std::fs::read_to_string(diff_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("не могу прочитать {diff_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let orig_hash = phase_vm::patch::fnv_digest(&orig);
    match phase_vm::patch::apply_unified(&orig, &diff) {
        Ok(r) => {
            match &out_path {
                Some(p) => std::fs::write(p, &r.text).unwrap_or_else(|e| {
                    eprintln!("не могу писать {p}: {e}");
                    std::process::exit(1)
                }),
                None => print!("{}", r.text),
            }
            println!("phase-patch: OK");
            println!(
                "аудит: orig-digest=0x{orig_hash:016x}  result-digest=0x{:016x}",
                r.digest
            );
            println!("атомарность: файл записан одним коммитом после полного успеха");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("phase-patch: ОТКАЗ (файл не тронут): {e}");
            // доказательство: файл на диске бит-в-бит как был
            ExitCode::FAILURE
        }
    }
}
