//! # phase-vm — обратимая регистровая машина (честная редакция проекта №1)
//!
//! Манифест говорил: «каждая инструкция — симплектический поворот, программа
//! обратима до машинного эпсилон, без единого байта логов». Это невозможно на
//! IEEE-754 (NaN/округление необратимы) и в общем виде (стирание обязано
//! случиться). Реализуем *честное* ядро, которое действительно даёт
//! бесплатный откат:
//!
//! ## Принцип
//! 1. **Состояние** — регистровый файл `u64` (биты/целые, никаких float).
//! 2. **Каждая инструкция модифицирует ровно один destination; все источники
//!    остаются нетронутыми** и достаточны для обращения. Информация не
//!    стирается — она остаётся в неизменённых операндах. Поэтому обратная
//!    инструкция вычислима из *текущего* состояния без какого-либо лога.
//! 3. Единственная необратимость — `Set` (уничтожает старое значение
//!    регистра без источника). Это **граница**: платится один раз при вводе.
//!    Всё между границами — обратимое ядро: откат бесплатен.
//!
//! ## Следствия (для будущих проектов)
//! - `--reverse` внутри фазы = один обратный проход, 0 байт логов, 0 снапшотов.
//! - Bennett «clean computation»: промежуточные ячейки убираются обратным
//!   прогоном (uncompute), результат копируется в свежую ячейку.
//! - ARX-структура (add/rot/xor) сама по себе обратима → фундамент для #12.
//!
//! ## Модель
//! - `Program` = дерево `Node::Inst | Node::Rep{count, body}` (Rep = статический
//!   «sweep»-цикл: биекция, повторенная count раз; обратный = count раз inverse).
//! - Верификатор (`check`) классифицирует инструкции: обратимые vs границы
//!   (`Set`) vs нарушения алиасинга операндов (dst==src и т.п. — небиективно).
//! - `reverse_n` идёт от конца потока и применяет инверсии, останавливаясь на
//!   границе — откат «суффикса» после последнего `Set`.
//!
//! ## Проверка
//! Fuzz: случайное обратимое ядро на случайном состоянии → `F⁻¹(F(S)) == S`
//! бит-в-бит (см. `tests/roundtrip.rs`).

pub mod alloc;
pub mod aot;
pub mod cap;
pub mod cipher;
pub mod ctl;
pub mod inst;
pub mod machine;
pub mod opt;
pub mod patch;
pub mod program;
pub mod state;
pub mod synth;

pub use inst::Inst;
pub use machine::{count_leaves, reverse_n, run_forward};
pub use program::{Node, Program};
pub use state::State;

/// Вердикт верификатора.
pub mod check {
    use crate::program::Node;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ViolationKind {
        /// `Set` — уничтожает значение без источника: граница (необратимо).
        Boundary,
        /// Алиасинг операндов (dst==src и т.п.): операция небиективна.
        Alias,
    }

    #[derive(Debug, Clone)]
    pub struct Violation {
        /// Индекс листа в развёрнутом потоке инструкций (0-based).
        pub leaf: u64,
        pub kind: ViolationKind,
        pub reason: String,
    }

    #[derive(Debug, Clone)]
    pub struct Report {
        pub total_leaves: u64,
        pub invertible_leaves: u64,
        pub irreversible_leaves: u64,
        pub violations: Vec<Violation>,
    }

    impl Report {
        /// Программа чисто обратима: нет ни границ, ни алиасинга.
        pub fn reversible(&self) -> bool {
            self.irreversible_leaves == 0 && self.violations.is_empty()
        }
    }

    fn walk(nodes: &[Node], counter: &mut u64, out: &mut Report) {
        for node in nodes {
            match node {
                Node::Inst(inst) => {
                    let leaf = *counter;
                    *counter += 1;
                    out.total_leaves += 1;
                    if let Some(alias) = inst.operand_alias() {
                        out.violations.push(Violation {
                            leaf,
                            kind: ViolationKind::Alias,
                            reason: alias.to_string(),
                        });
                        out.irreversible_leaves += 1;
                    } else if inst.is_irreversible() {
                        out.violations.push(Violation {
                            leaf,
                            kind: ViolationKind::Boundary,
                            reason: "Set: уничтожает старое значение регистра без источника".into(),
                        });
                        out.irreversible_leaves += 1;
                    } else {
                        out.invertible_leaves += 1;
                    }
                }
                Node::Rep { count, body } => {
                    // В развёрнутом потоке тело повторяется count раз; счётчик
                    // листьев продвигается сквозь повторения автоматически.
                    for _ in 0..*count {
                        walk(body, counter, out);
                    }
                }
            }
        }
    }

    /// Проверить программу на обратимость.
    pub fn check(nodes: &[Node]) -> Report {
        let mut rep = Report {
            total_leaves: 0,
            invertible_leaves: 0,
            irreversible_leaves: 0,
            violations: Vec::new(),
        };
        let mut counter = 0u64;
        walk(nodes, &mut counter, &mut rep);
        rep
    }
}
