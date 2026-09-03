//! L4 (P13): капсула фазы в .eml-конверте.
//!
//! Заголовки = метаданные инвариантов (state-hash, boundary-hash, kernel-hash,
//! счётчики); тело = состояние (regs/mem) + исходник ядра. Капсула
//! самодостаточна: принимающая машина может (1) сверить state-hash,
//! (2) откатить к границе и сверить boundary-hash, (3) пересчитать вперёд и
//! убедиться в детерминизме — миграция без доверия к отправителю.

use crate::state::State;

#[derive(Debug, Clone)]
pub struct Capsule {
    pub tag: String,
    pub state: State,
    pub kernel_src: String,
    pub state_hash: u64,
    pub boundary_hash: u64,
    pub kernel_hash: u64,
}

fn fnv1a(s: &str) -> u64 {
    let prime = 0x100000001b3u64;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in s.as_bytes() {
        h = (h ^ b as u64).wrapping_mul(prime);
    }
    h
}

fn hex_words(v: &[u64]) -> String {
    v.iter()
        .map(|w| format!("{w:016x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn unhex_words(s: &str) -> Vec<u64> {
    s.split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| u64::from_str_radix(t, 16).unwrap())
        .collect()
}

/// Манифест (VCF-стиль): семантические регистры (SINT), порты, роль.
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub registers: Vec<&'static str>, // SENSE/FACT/LOGIC/OPINION/CAUSALITY
    pub ports: Vec<String>,
    pub role: String,
}

/// Экспорт капсулы в .eml-текст (+ манифест, если задан).
pub fn export(tag: &str, kernel_src: &str, state: &State, boundary_hash: u64) -> String {
    export_with(tag, kernel_src, state, boundary_hash, None)
}

/// Экспорт с VCF-манифестом (L4: капсула несёт регистры+порты+роль).
pub fn export_with(
    tag: &str,
    kernel_src: &str,
    state: &State,
    boundary_hash: u64,
    mf: Option<&Manifest>,
) -> String {
    let state_hash = state.hash();
    let kernel_hash = fnv1a(kernel_src);
    let mut out = String::new();
    out.push_str("From: phase-vm@localhost\n");
    out.push_str(&format!("To: {tag}\n"));
    out.push_str(&format!("Subject: phase capsule [{tag}]\n"));
    out.push_str("Message-ID: <phase.capsule@localhost>\n");
    out.push_str("Date: Thu, 03 Sep 2026 00:00:00 +0000\n");
    out.push_str(&format!("X-Phase-Tag: {tag}\n"));
    out.push_str(&format!("X-Phase-Kernel-Hash: {kernel_hash:016x}\n"));
    out.push_str(&format!("X-Phase-Boundary-Hash: {boundary_hash:016x}\n"));
    out.push_str(&format!("X-Phase-State-Hash: {state_hash:016x}\n"));
    out.push_str(&format!("X-Phase-NRegs: {}\n", state.regs.len()));
    out.push_str(&format!("X-Phase-NMem: {}\n", state.mem.len()));
    if let Some(m) = mf {
        out.push_str(&format!(
            "X-Phase-Vcf: BEGIN:VCARD\nX-Phase-Vcf: VERSION:3.0\nX-Phase-Vcf: ROLE:{}\n",
            m.role
        ));
        if !m.registers.is_empty() {
            out.push_str(&format!(
                "X-Phase-Vcf: REGISTERS:{}\n",
                m.registers.join(",")
            ));
        }
        if !m.ports.is_empty() {
            out.push_str(&format!("X-Phase-Vcf: PORTS:{}\n", m.ports.join(";")));
        }
        out.push_str("X-Phase-Vcf: END:VCARD\n");
    }
    out.push_str("Content-Type: text/plain; charset=utf-8\n");
    out.push('\n');
    // тело: секции
    out.push_str(";; phase-state\n");
    out.push_str(&format!("REGS: {}\n", hex_words(&state.regs)));
    if !state.mem.is_empty() {
        out.push_str(&format!("MEM: {}\n", hex_words(&state.mem)));
    }
    out.push_str(";; phase-program\n");
    out.push_str(kernel_src);
    out
}

/// Импорт капсулы из .eml-текста. Проверяет state-hash по заголовку.
pub fn import(text: &str) -> Result<Capsule, String> {
    let (head, body) = text
        .split_once("\n\n")
        .ok_or("нет разделителя заголовков")?;
    let mut hdr = std::collections::HashMap::new();
    for line in head.lines() {
        if let Some((k, v)) = line.split_once(':') {
            hdr.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let get = |k: &str| -> Result<String, String> {
        hdr.get(k)
            .cloned()
            .ok_or_else(|| format!("нет заголовка {k}"))
    };
    let tag = get("X-Phase-Tag")?;
    let state_hash = u64::from_str_radix(&get("X-Phase-State-Hash")?, 16).unwrap();
    let boundary_hash = u64::from_str_radix(&get("X-Phase-Boundary-Hash")?, 16).unwrap();
    let kernel_hash = u64::from_str_radix(&get("X-Phase-Kernel-Hash")?, 16).unwrap();
    let nregs: usize = get("X-Phase-NRegs")?.parse().unwrap();
    let nmem: usize = get("X-Phase-NMem")?.parse().unwrap();

    let (state_sec, prog_sec) = body
        .split_once(";; phase-program\n")
        .ok_or("нет секции program")?;
    let mut regs = vec![0u64; nregs];
    let mut mem = vec![0u64; nmem];
    for line in state_sec.lines() {
        if let Some(v) = line.strip_prefix("REGS: ") {
            let w = unhex_words(v);
            if w.len() == nregs {
                regs = w;
            }
        } else if let Some(v) = line.strip_prefix("MEM: ") {
            let w = unhex_words(v);
            if w.len() == nmem {
                mem = w;
            }
        }
    }
    let kernel_src = prog_sec.to_string();
    if fnv1a(&kernel_src) != kernel_hash {
        return Err("kernel-hash не совпал".into());
    }
    let state = State { regs, mem };
    if state.hash() != state_hash {
        return Err("state-hash не совпал: капсула повреждена".into());
    }
    Ok(Capsule {
        tag,
        state,
        kernel_src,
        state_hash,
        boundary_hash,
        kernel_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_import_roundtrip() {
        let mut st = State::random(8, 4, 7);
        st.regs[2] = 0xABCD;
        let src = "set r0 1\nadd r0 r1\n";
        let eml = export("t1", src, &st, 0x1234);
        let cap = import(&eml).unwrap();
        assert_eq!(cap.state, st);
        assert_eq!(cap.tag, "t1");
        assert_eq!(cap.boundary_hash, 0x1234);
        assert_eq!(cap.kernel_src, src);
    }

    #[test]
    fn manifest_vcf_embedded() {
        let st = State::random(4, 0, 9);
        let mf = Manifest {
            registers: vec!["FACT", "LOGIC"],
            ports: vec!["in:a".into(), "out:b".into()],
            role: "r".into(),
        };
        let eml = export_with("m", "add r0 r1\n", &st, 0, Some(&mf));
        assert!(eml.contains("BEGIN:VCARD"));
        assert!(eml.contains("REGISTERS:FACT,LOGIC"));
        assert!(eml.contains("PORTS:in:a;out:b"));
        assert!(eml.contains("END:VCARD"));
    }

    #[test]
    fn tamper_detected() {
        let st = State::random(8, 0, 3);
        let src = "add r0 r1\n";
        let eml = export("t", src, &st, 0);
        // портим тело
        let bad = eml.replace("REGS: ", "REGS: 0000000000000001 ");
        assert!(import(&bad).is_err(), "порча тела не обнаружена");
    }
}
