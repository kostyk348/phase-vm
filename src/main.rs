//! CLI phase-vm: check / run / runrev / rev / roundtrip / bench / dbg / cipher.
//!
//! Примеры:
//!   phase-vm check  programs/mixer.phase
//!   phase-vm run    programs/clean.phase --regs 8 --set r0=5 --set r1=7
//!   phase-vm rev    programs/clean.phase --regs 8 --set r0=5 --set r1=7
//!   phase-vm run    programs/ledger.phase --mem 8
//!   phase-vm rev    programs/ledger.phase --mem 8            # откат перевода
//!   phase-vm dbg    programs/mixer.phase                      # reverse-step
//!   phase-vm roundtrip programs/mixer.phase --trials 1000
//!   phase-vm bench  programs/mixer_big.phase --rounds 200
//!   phase-vm cipher --key 0102030405060708... --pt 0123456789abcdef...

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::time::Instant;

use phase_vm::check;
use phase_vm::inst::Inst;
use phase_vm::machine::{
    count_leaves, iter_forward, iter_reverse, leaf_at, reverse_all, reverse_n,
    reversible_suffix_len, run_forward,
};
use phase_vm::program::parse;
use phase_vm::state::{fmt_mem, State};

fn usage() -> ! {
    eprintln!(
        "phase-vm — обратимая регистровая машина\n\
         \n\
         usage:\n\
         \x20 phase-vm check <file.phase> [--regs N] [--mem N]\n\
         \x20 phase-vm run  <file.phase> [--regs N] [--mem N] [--set rN=val ...]\n\
         \x20 phase-vm runrev <file.phase> [--regs N] [--mem N] [--set ...]\n\
         \x20        (обратный прогон из данного состояния — напр. decrypt)\n\
         \x20 phase-vm rev  <file.phase> [--regs N] [--mem N] [--set ...] [--steps K]\n\
         \x20 phase-vm dbg  <file.phase> [--regs N] [--mem N] [--set ...]\n\
         \x20        (интерактивный отладчик: f/b/g/c/p/pm/br/bl/q)\n\
         \x20 phase-vm roundtrip <file.phase> [--regs N] [--mem N] [--trials T] [--seed S]\n\
         \x20 phase-vm bench <file.phase> [--regs N] [--mem N] [--rounds R] [--set ...]\n\
         \x20 phase-vm cipher --key <hex64> --pt <hex64> [--rounds R]\n\
         \x20        (PC1: encrypt=forward, decrypt=backward)\n"
    );
    std::process::exit(2);
}

#[derive(Default)]
struct Opts {
    regs: usize,
    mem: usize,
    sets: Vec<(u8, u64)>,
    trials: u64,
    seed: u64,
    steps: Option<u64>,
    rounds: u64,
}

fn collect_opts(args: &[String]) -> (String, Opts) {
    let mut file: Option<String> = None;
    let mut o = Opts {
        regs: 16,
        mem: 0,
        trials: 100,
        seed: 0x1234_5678,
        rounds: 100,
        ..Opts::default()
    };
    let mut i = 0;
    let num = |s: &str| -> u64 { s.parse().unwrap_or_else(|_| usage()) };
    while i < args.len() {
        match args[i].as_str() {
            "--regs" => {
                o.regs = num(&args[i + 1]) as usize;
                i += 2;
            }
            "--mem" => {
                o.mem = num(&args[i + 1]) as usize;
                i += 2;
            }
            "--trials" => {
                o.trials = num(&args[i + 1]);
                i += 2;
            }
            "--seed" => {
                o.seed = num(&args[i + 1]);
                i += 2;
            }
            "--steps" => {
                o.steps = Some(num(&args[i + 1]));
                i += 2;
            }
            "--rounds" => {
                o.rounds = num(&args[i + 1]);
                i += 2;
            }
            "--set" => {
                let kv = &args[i + 1];
                o.sets.push(parse_set(kv));
                i += 2;
            }
            s if s.starts_with("--set=") => {
                o.sets.push(parse_set(&s[6..]));
                i += 1;
            }
            s if !s.starts_with('-') && file.is_none() => {
                file = Some(s.to_string());
                i += 1;
            }
            _ => usage(),
        }
    }
    (file.unwrap_or_else(|| usage()), o)
}

fn parse_set(kv: &str) -> (u8, u64) {
    let (reg, val) = kv.split_once('=').unwrap_or_else(|| usage());
    let r: usize = reg
        .trim_start_matches(['r', 'R'])
        .parse()
        .unwrap_or_else(|_| usage());
    (r as u8, parse_imm(val))
}

fn parse_imm(s: &str) -> u64 {
    let t = s.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).unwrap_or_else(|_| usage())
    } else if let Some(b) = t.strip_prefix('-') {
        let v: u64 = b.parse().unwrap_or_else(|_| usage());
        v.wrapping_neg()
    } else {
        t.parse().unwrap_or_else(|_| usage())
    }
}

fn read_prog(file: &str, nregs: usize) -> Vec<phase_vm::Node> {
    let text = std::fs::read_to_string(file).unwrap_or_else(|e| {
        eprintln!("не могу прочитать {file}: {e}");
        std::process::exit(1)
    });
    parse(&text, nregs)
        .unwrap_or_else(|e| {
            eprintln!("ошибка парсинга {file}: {e}");
            std::process::exit(1)
        })
        .nodes
}

fn init_state(o: &Opts) -> State {
    let mut s = State::new(o.regs, o.mem);
    for &(r, v) in &o.sets {
        if r as usize >= o.regs {
            eprintln!("--set r{r} вне диапазона --regs {}", o.regs);
            std::process::exit(1);
        }
        s.regs[r as usize] = v;
    }
    s
}

/// Компактный показ: только ненулевые регистры/слова памяти.
fn show(s: &State) {
    let nz: Vec<String> = s
        .regs
        .iter()
        .enumerate()
        .filter(|(_, &v)| v != 0)
        .map(|(i, v)| format!("r{i}=0x{v:016x}"))
        .collect();
    if nz.is_empty() {
        println!("regs: (все нули)");
    } else {
        println!("regs: {}", nz.join(" "));
    }
    if s.nmem() > 0 {
        let nzm: Vec<String> = s
            .mem
            .iter()
            .enumerate()
            .filter(|(_, &v)| v != 0)
            .map(|(i, v)| format!("m[{i}]=0x{v:016x}"))
            .collect();
        if nzm.is_empty() {
            println!("mem : (все нули)");
        } else {
            println!("mem : {}", nzm.join(" "));
        }
    }
}

fn cmd_check(file: &str, o: &Opts) -> ExitCode {
    let nodes = read_prog(file, o.regs);
    let rep = check::check(&nodes);
    println!("файл: {file}");
    println!("листьев всего:       {}", rep.total_leaves);
    println!("обратимых:           {}", rep.invertible_leaves);
    println!("необратимых(границ): {}", rep.irreversible_leaves);
    if rep.reversible() {
        println!("ВЕРДИКТ: программа ЧИСТО обратима (F⁻¹∘F = Id, reverse по всей длине)");
    } else {
        println!("ВЕРДИКТ: НЕ чисто обратима (есть границы/нарушения)");
        for v in &rep.violations {
            match v.kind {
                check::ViolationKind::Boundary => {
                    println!("  граница на листе #{}: {}", v.leaf, v.reason)
                }
                check::ViolationKind::Alias => {
                    println!("  алиасинг на листе #{}: {}", v.leaf, v.reason)
                }
            }
        }
        println!(
            "откатываемый суффикс после последней границы: {} листьев",
            reversible_suffix_len(&nodes)
        );
    }
    ExitCode::SUCCESS
}

fn cmd_run(file: &str, o: &Opts) -> ExitCode {
    let nodes = read_prog(file, o.regs);
    let mut s = init_state(o);
    let k = run_forward(&mut s, &nodes).unwrap_or_else(|e| {
        eprintln!("ошибка исполнения: {e}");
        std::process::exit(1)
    });
    println!("forward: {k} листьев");
    show(&s);
    ExitCode::SUCCESS
}

fn cmd_runrev(file: &str, o: &Opts) -> ExitCode {
    let nodes = read_prog(file, o.regs);
    let mut s = init_state(o);
    let k = reverse_all(&mut s, &nodes).unwrap_or_else(|e| {
        eprintln!("ошибка обратного прогона: {e}");
        std::process::exit(1)
    });
    println!("backward: {k} листьев");
    show(&s);
    ExitCode::SUCCESS
}

fn cmd_rev(file: &str, o: &Opts) -> ExitCode {
    let nodes = read_prog(file, o.regs);
    let mut s = init_state(o);
    run_forward(&mut s, &nodes).unwrap_or_else(|e| {
        eprintln!("ошибка исполнения: {e}");
        std::process::exit(1)
    });
    let total = count_leaves(&nodes);
    let suffix = reversible_suffix_len(&nodes);
    let steps = o.steps.unwrap_or(u64::MAX).min(suffix);
    let done = reverse_n(&mut s, &nodes, steps).unwrap_or_else(|e| {
        eprintln!("ошибка отката: {e}");
        std::process::exit(1)
    });
    println!("forward: {total} листьев; обратимый суффикс: {suffix}; откачено: {done}");
    show(&s);
    println!(
        "{}",
        if done == suffix {
            "state = состояние сразу после границ (ввод не тронут)"
        } else {
            "state = промежуточная точка внутри ядра"
        }
    );
    ExitCode::SUCCESS
}

fn cmd_roundtrip(file: &str, o: &Opts) -> ExitCode {
    let nodes = read_prog(file, o.regs);
    let rep = check::check(&nodes);
    if !rep.reversible() {
        eprintln!(
            "roundtrip требует чисто обратимую программу (без set/mset). \
             Проверка: {} листьев, {} необратимых. Смотри 'check'.",
            rep.total_leaves, rep.irreversible_leaves
        );
        return ExitCode::FAILURE;
    }
    let mut passes = 0u64;
    for t in 0..o.trials {
        let orig = State::random(o.regs, o.mem, o.seed.wrapping_add(t * 0x9E3779B97F4A7C15));
        let mut s = orig.clone();
        run_forward(&mut s, &nodes).unwrap();
        let after = s.clone();
        let done = reverse_all(&mut s, &nodes).unwrap();
        assert_eq!(done, rep.total_leaves, "не весь поток откачен");
        if s != orig {
            eprintln!("ПРОВАЛ на trial {t}");
            eprintln!("orig:  {orig}");
            eprintln!("after: {after}");
            eprintln!("back:  {s}");
            return ExitCode::FAILURE;
        }
        passes += 1;
    }
    println!(
        "roundtrip OK: {passes}/{} случайных состояний, F⁻¹(F(S)) == S бит-в-бит",
        o.trials
    );
    println!("(листьев на прогон: {})", rep.total_leaves);
    ExitCode::SUCCESS
}

fn cmd_bench(file: &str, o: &Opts) -> ExitCode {
    let nodes = read_prog(file, o.regs);
    let leaves = count_leaves(&nodes);
    let mut s = init_state(o);
    if s.regs.iter().all(|&v| v == 0) && s.mem.iter().all(|&v| v == 0) {
        s = State::random(o.regs, o.mem, 42);
    }
    let mut dummy = 0u64;

    let t0 = Instant::now();
    for _ in 0..o.rounds {
        let k = run_forward(&mut s, &nodes).unwrap();
        dummy = dummy.wrapping_add(k);
    }
    let fwd_ns = t0.elapsed().as_nanos() as f64 / (o.rounds as f64 * leaves as f64);

    let t1 = Instant::now();
    for _ in 0..o.rounds {
        let k = reverse_all(&mut s, &nodes).unwrap();
        dummy = dummy.wrapping_add(k);
    }
    let rev_ns = t1.elapsed().as_nanos() as f64 / (o.rounds as f64 * leaves as f64);

    let state_bytes = (o.regs + o.mem) * 8;
    let t2 = Instant::now();
    for _ in 0..o.rounds {
        let cp = (s.regs.clone(), s.mem.clone());
        dummy = dummy.wrapping_add(cp.0[0]);
    }
    let snap_ns = t2.elapsed().as_nanos() as f64 / o.rounds as f64;

    let _ = std::hint::black_box(dummy);
    println!(
        "leaves={leaves} regs={} mem={} rounds={}",
        o.regs, o.mem, o.rounds
    );
    println!("forward : {fwd_ns:8.2} ns/leaf");
    println!("backward: {rev_ns:8.2} ns/leaf   (доп. память: 0 B, логов: 0)");
    println!("snapshot: {snap_ns:8.2} ns на копию состояния {state_bytes} B (regs+mem)");
    ExitCode::SUCCESS
}

// ---- отладчик (a): reverse-step, брейкпоинты, просмотр ----

struct Dbg {
    nodes: Vec<phase_vm::Node>,
    total: u64,
    /// число исполненных листьев (состояние = F над leaves[0..pos])
    pos: u64,
    s: State,
    breaks: Vec<u64>,
}

impl Dbg {
    fn describe(&self) {
        let rev_suffix = reversible_suffix_len(&self.nodes);
        println!(
            "pos={}/{} листьев | откатываемый суффикс={} | брейкпоинты={:?}",
            self.pos, self.total, rev_suffix, self.breaks
        );
    }

    fn cur_inst(&self) -> Option<Inst> {
        leaf_at(&self.nodes, self.pos)
    }

    fn step_fwd(&mut self, n: u64) -> Result<(), String> {
        for _ in 0..n {
            if self.pos >= self.total {
                return Ok(());
            }
            let inst = self.cur_inst().unwrap();
            inst.exec(&mut self.s)
                .map_err(|e| format!("лист #{} ({}): {}", self.pos, inst, e))?;
            self.pos += 1;
        }
        Ok(())
    }

    fn step_back(&mut self, n: u64) -> Result<(), String> {
        for _ in 0..n {
            if self.pos == 0 {
                return Ok(());
            }
            let leaf = leaf_at(&self.nodes, self.pos - 1).unwrap();
            let inv = match leaf.inverse() {
                Some(i) if leaf.operand_alias().is_none() => i,
                _ => {
                    return Err(format!(
                        "лист #{} ({leaf}) — граница: откат через set/mset требует лога/чекпоинта",
                        self.pos - 1
                    ))
                }
            };
            inv.exec(&mut self.s)
                .map_err(|e| format!("обратный лист #{} ({}): {}", self.pos - 1, inv, e))?;
            self.pos -= 1;
        }
        Ok(())
    }

    /// continue: исполнять пока не упрёмся в брейкпоинт (до его исполнения) или конец.
    fn cont(&mut self) -> Result<(), String> {
        loop {
            if self.breaks.contains(&self.pos) {
                println!("стоп на брейкпоинте pos={}", self.pos);
                return Ok(());
            }
            if self.pos >= self.total {
                return Ok(());
            }
            let inst = self.cur_inst().unwrap();
            inst.exec(&mut self.s)
                .map_err(|e| format!("лист #{} ({}): {}", self.pos, inst, e))?;
            self.pos += 1;
        }
    }

    fn goto(&mut self, target: u64) -> Result<(), String> {
        if target > self.pos {
            self.step_fwd(target - self.pos)
        } else if target < self.pos {
            self.step_back(self.pos - target)
        } else {
            Ok(())
        }
    }
}

fn cmd_dbg(file: &str, o: &Opts) -> ExitCode {
    let nodes = read_prog(file, o.regs);
    let rep = check::check(&nodes);
    if rep
        .violations
        .iter()
        .any(|v| v.kind == check::ViolationKind::Alias)
    {
        eprintln!("программа содержит небиективные инструкции (алиасинг) — отладка невозможна:");
        for v in rep
            .violations
            .iter()
            .filter(|v| v.kind == check::ViolationKind::Alias)
        {
            eprintln!("  лист #{}: {}", v.leaf, v.reason);
        }
        return ExitCode::FAILURE;
    }
    let mut d = Dbg {
        total: count_leaves(&nodes),
        nodes,
        pos: 0,
        s: init_state(o),
        breaks: Vec::new(),
    };
    println!("phase-vm dbg — команды:");
    println!("  f [N] шаг вперёд   b [N] шаг назад   c continue   g N goto");
    println!("  p [rN] regs        pm [N] память     br N / bd N / bl  q выход");
    d.describe();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    loop {
        print!("dbg> ");
        let _ = out.flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.is_empty() {
            continue;
        }
        let narg = |i: usize| t.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
        match t[0] {
            "q" | "quit" | "exit" => break,
            "f" => {
                let n = if t.len() > 1 { narg(1) } else { 1 };
                if let Err(e) = d.step_fwd(n) {
                    eprintln!("{e}");
                }
                if d.pos < d.total {
                    if let Some(i) = d.cur_inst() {
                        println!("→ лист #{}: {}", d.pos, i);
                    }
                } else {
                    println!("конец программы (pos={})", d.pos);
                }
            }
            "b" | "r" => {
                let n = if t.len() > 1 { narg(1) } else { 1 };
                match d.step_back(n) {
                    Ok(()) => {
                        if d.pos < d.total {
                            if let Some(i) = leaf_at(&d.nodes, d.pos) {
                                println!("→ лист #{}: {} (следующий к исполнению)", d.pos, i);
                            }
                        }
                    }
                    Err(e) => eprintln!("{e}"),
                }
            }
            "c" => {
                if let Err(e) = d.cont() {
                    eprintln!("{e}");
                } else if d.pos >= d.total {
                    println!("конец программы");
                }
            }
            "g" => {
                let target = if t.len() > 1 { narg(1) } else { 0 };
                if let Err(e) = d.goto(target) {
                    eprintln!("{e}");
                }
            }
            "p" | "print" => {
                let regs: Vec<&str> = t[1..].to_vec();
                if regs.is_empty() {
                    println!("{}", d.s);
                } else {
                    for reg in regs {
                        if let Some(stripped) =
                            reg.strip_prefix('r').or_else(|| reg.strip_prefix('R'))
                        {
                            if let Ok(idx) = stripped.parse::<usize>() {
                                if idx < d.s.regs.len() {
                                    println!("r{idx}=0x{:016x}", d.s.regs[idx]);
                                    continue;
                                }
                            }
                        }
                        eprintln!("нет регистра {reg}");
                    }
                }
            }
            "pm" => {
                let max = if t.len() > 1 { narg(1) as usize } else { 8 };
                println!("{}", fmt_mem(&d.s, max));
            }
            "br" => {
                if t.len() > 1 {
                    let v = narg(1);
                    if !d.breaks.contains(&v) {
                        d.breaks.push(v);
                    }
                    println!("брейкпоинт на pos={v}");
                } else {
                    eprintln!("br N");
                }
            }
            "bd" => {
                if t.len() > 1 {
                    d.breaks.retain(|&x| x != narg(1));
                }
            }
            "bl" => println!("брейкпоинты: {:?}", d.breaks),
            "s" | "st" | "status" => d.describe(),
            other => eprintln!("неизвестно: {other}"),
        }
    }
    ExitCode::SUCCESS
}

// ---- cipher (c): PC1 demo ----

fn cmd_cipher(_o: &Opts, args: &[String]) -> ExitCode {
    let mut key = [0u64; 4];
    let mut pt = [0u64; 4];
    let mut rounds = 12u64;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--key" => {
                let hex = &args[i + 1];
                if hex.len() != 64 {
                    eprintln!("--key: ожидается 16 байт hex (128 бит)");
                    return ExitCode::FAILURE;
                }
                for w in 0..4 {
                    key[w] = u64::from_str_radix(&hex[w * 16..w * 16 + 16], 16).unwrap();
                }
                i += 2;
            }
            "--pt" => {
                let hex = &args[i + 1];
                if hex.len() != 64 {
                    eprintln!("--pt: ожидается 16 байт hex (128 бит)");
                    return ExitCode::FAILURE;
                }
                for w in 0..4 {
                    pt[w] = u64::from_str_radix(&hex[w * 16..w * 16 + 16], 16).unwrap();
                }
                i += 2;
            }
            "--rounds" => {
                rounds = args[i + 1].parse().unwrap_or_else(|_| usage());
                i += 2;
            }
            _ => usage(),
        }
    }
    let ct = match phase_vm::cipher::pc1_encrypt(pt, key, rounds) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ошибка шифрования: {e}");
            return ExitCode::FAILURE;
        }
    };
    let back = match phase_vm::cipher::pc1_decrypt(ct, key, rounds) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ошибка расшифрования: {e}");
            return ExitCode::FAILURE;
        }
    };
    let f = |a: [u64; 4]| -> String { a.iter().map(|w| format!("{w:016x}")).collect::<String>() };
    println!("PC1 rounds={rounds}");
    println!("key = {}", f(key));
    println!("pt  = {}", f(pt));
    println!("ct  = {}", f(ct));
    println!("pt' = {}  (decrypt = обратный прогон ядра)", f(back));
    println!(
        "{}",
        if back == pt {
            "OK: decrypt(ct) == pt, F⁻¹ без отдельного алгоритма расшифрования"
        } else {
            "ПРОВАЛ: расшифрование не сошлось"
        }
    );
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        usage();
    }
    let sub = argv.remove(0);
    if sub == "cipher" {
        return cmd_cipher(&Opts::default(), &argv);
    }
    let (file, o) = collect_opts(&argv);
    match sub.as_str() {
        "check" => cmd_check(&file, &o),
        "run" => cmd_run(&file, &o),
        "runrev" => cmd_runrev(&file, &o),
        "rev" => cmd_rev(&file, &o),
        "dbg" => cmd_dbg(&file, &o),
        "roundtrip" => cmd_roundtrip(&file, &o),
        "bench" => cmd_bench(&file, &o),
        _ => usage(),
    }
}

// оставлено для будущего: прямой перебор потоков без исполнения
#[allow(dead_code)]
fn _stream_sizes(nodes: &[phase_vm::Node]) -> (u64, u64) {
    (
        iter_forward(nodes).count() as u64,
        iter_reverse(nodes).count() as u64,
    )
}
