//! L4 (P13): миграция фазы через .eml-капсулу между «хостами».
//!
//! Хост A: выполняет тики, на тике k «замораживает» фазу в капсулу .eml.
//! Хост B: импортирует, НЕ доверяя отправителю — проверяет state-hash,
//! откатывает к границе (сверяет boundary-hash), пересчитывает вперёд
//! (детерминизм), продолжает тики. Финальная hash-цепочка обязана совпасть
//! с «чистым» прогоном на A — миграция не оставила следов.

use std::time::Instant;

use phase_vm::cap;
use phase_vm::machine::{reverse_all, run_forward};
use phase_vm::program::parse;
use phase_vm::state::State;

const KERNEL: &str = "\
rep 10000
  add r0 r1
  rotl r1 7
  xor r1 r0
  swp r0 r1
end
";

fn main() {
    // --- Хост A: граница + часть ядра до тика k=7000 ---
    let _nodes = parse(KERNEL, 8).unwrap().nodes;
    // чисто обратимое: rep 10000 — режем «миграцию» на rep 7000… но rep
    // статичен; поэтому моделируем тики как кратные rep-части: берём rep 7000.
    let a_src = KERNEL.replace("rep 10000", "rep 7000");
    let a_nodes = parse(&a_src, 8).unwrap().nodes;

    let mut host_a = State::random(8, 0, 0xAB);
    let boundary_hash = host_a.hash();

    let t0 = Instant::now();
    run_forward(&mut host_a, &a_nodes).unwrap();

    // --- экспорт капсулы (состояние после 7000 итераций) + VCF-манифест ---
    let mf = cap::Manifest {
        registers: vec!["FACT", "LOGIC"],
        ports: vec!["in:state@tick7000".into(), "out:state@tick10000".into()],
        role: "migration-demo/phase@7000".into(),
    };
    let eml = cap::export_with("migration-demo", &a_src, &host_a, boundary_hash, Some(&mf));

    // --- Хост B: импорт + верификация без доверия ---
    let cap_msg = cap::import(&eml).expect("капсула повреждена");
    assert_eq!(cap_msg.state, host_a, "состояние на B != состояние на A");

    let mut host_b = cap_msg.state.clone();
    let b_nodes = parse(&cap_msg.kernel_src, 8).unwrap().nodes;
    let _ = t0;

    // 1) откат к границе — сверяем boundary-hash
    reverse_all(&mut host_b, &b_nodes).unwrap();
    assert_eq!(
        host_b.hash(),
        cap_msg.boundary_hash,
        "boundary-hash не сошёлся: откат оставил след"
    );
    // 2) пересчёт вперёд — детерминизм, сверяем state-hash из капсулы
    run_forward(&mut host_b, &b_nodes).unwrap();
    assert_eq!(host_b.hash(), cap_msg.state_hash, "state-hash не сошёлся");
    assert_eq!(host_b, host_a, "после верификации состояние разошлось");

    // --- продолжение на B: остаток ядра (до rep 10000) ---
    let tail_src = KERNEL.replace("rep 10000", "rep 3000");
    let tail = parse(&tail_src, 8).unwrap().nodes;
    run_forward(&mut host_b, &tail).unwrap();

    // --- эталон: «чистый» прогон целиком на одном хосте ---
    let full = parse(KERNEL, 8).unwrap().nodes;
    let mut reference = State::random(8, 0, 0xAB);
    run_forward(&mut reference, &full).unwrap();
    let elapsed = t0.elapsed();
    assert_eq!(
        host_b, reference,
        "миграция на тике 7000 дала другой финал, чем чистый прогон"
    );

    let eml_bytes = eml.len();
    let naive_bytes = (8 * 8) + eml_bytes; // состояние 64B + всё остальное
    let _ = naive_bytes;
    println!("capsule (L4): миграция фазы через .eml между «хостами»");
    println!(
        "капсула: {} B (заголовки-инварианты + состояние + ядро)",
        eml_bytes
    );
    println!("верификация на B: state-hash OK, reverse→boundary-hash OK, re-forward→state-hash OK");
    println!(
        "финал после миграции на тике 7000/10000 == чистый прогон (assert) — {:?}",
        elapsed
    );
    println!("{}", "-".repeat(30));
    // показать голову капсулы (красиво)
    for l in eml.lines().take(12) {
        println!("{l}");
    }
}
