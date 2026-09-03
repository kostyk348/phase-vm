//! Фазовая арена (проект #10, честное ядро).
//!
//! Что обратимость даёт аллокатору (и чего НЕ даёт — см. анализ в README):
//! - bump-аллокация **обратима**: `malloc` = сдвиг указателя; откат = сдвиг назад.
//! - `rollback(offset)` всей пачки аллокаций = **O(1)** (одно truncate),
//!   независимо от числа объектов; per-object заголовков нет (ноль метаданных).
//! - free в произвольном порядке НЕ обратим без журнала — поэтому арена
//!   честно позиционируется на **phase-shaped** нагрузках (тик/транзакция/
//!   ECS): объекты живут до границы фазы, граница = bulk-reset или commit.
//!
//! Семантика (без unsafe, POC):
//! - хранение: `Vec<u64>` (выравнивание 8), аллокация в словах.
//! - `mark() -> usize` (смещение-метка), `rollback(mark)`: buf.truncate + снять
//!   метки выше — объекты после метки «исчезают» за O(1), без обхода.
//! - `commit(mark)`: снять метку, данные остаются (объекты живут до reset).
//! - `reset()`: O(1), вся фаза.
//! - Память после rollback НЕ зануляется — переиспользуется следующим bump
//!   (нулевой след = нет остаточного bookkeeping, не «затёртые байты»).

/// Фазовая арена. Single-thread (POC); per-thread — следующий шаг.
#[derive(Debug, Clone)]
pub struct Arena {
    buf: Vec<u64>,
    /// Стек меток (смещения в словах), строго возрастающий.
    marks: Vec<usize>,
    /// Счётчик аллокаций за время жизни (для статистики).
    pub allocs: u64,
}

impl Arena {
    pub fn new() -> Self {
        Arena::with_capacity_words(1024)
    }

    pub fn with_capacity_words(words: usize) -> Self {
        Arena {
            buf: Vec::with_capacity(words),
            marks: Vec::new(),
            allocs: 0,
        }
    }

    /// Выделить `n` байт (округляется до 8). Возвращает смещение в байтах.
    pub fn alloc(&mut self, n: usize) -> Option<usize> {
        let words = n.div_ceil(8);
        self.alloc_words(words)
    }

    pub fn alloc_words(&mut self, words: usize) -> Option<usize> {
        let off = self.buf.len();
        self.buf.try_reserve(words).ok()?;
        self.buf.resize(off + words, 0);
        self.allocs += 1;
        Some(off * 8)
    }

    /// Смещение текущей границы (в байтах) — это и есть «метка фазы».
    pub fn mark(&self) -> usize {
        self.buf.len() * 8
    }

    /// Зарегистрировать метку в стеке (для commit/rollback по имени).
    pub fn push_mark(&mut self) -> usize {
        let m = self.mark();
        self.marks.push(self.buf.len());
        m
    }

    /// Откат к метке (байты): всё, что выделено после неё, исчезает за O(1).
    /// Метки выше target снимаются.
    pub fn rollback(&mut self, mark_bytes: usize) {
        debug_assert!(mark_bytes.is_multiple_of(8));
        let words = mark_bytes / 8;
        debug_assert!(words <= self.buf.len(), "rollback за пределы арены");
        self.buf.truncate(words);
        // снять метки на уровне отката и выше (сама фаза-метка тоже закрывается)
        while let Some(&m) = self.marks.last() {
            if m >= words {
                self.marks.pop();
            } else {
                break;
            }
        }
    }

    /// Коммит: снять метку (данные остаются до reset).
    pub fn commit(&mut self) {
        self.marks.pop();
    }

    /// Сброс всей фазы: O(1), все объекты исчезают.
    pub fn reset(&mut self) {
        self.buf.clear();
        self.marks.clear();
    }

    /// Словный доступ к живым данным (объект = слово-срез, без unsafe).
    pub fn words(&self) -> &[u64] {
        &self.buf
    }

    pub fn write_u64(&mut self, off_bytes: usize, v: u64) {
        debug_assert!(off_bytes.is_multiple_of(8));
        self.buf[off_bytes / 8] = v;
    }

    pub fn read_u64(&self, off_bytes: usize) -> u64 {
        debug_assert!(off_bytes.is_multiple_of(8));
        self.buf[off_bytes / 8]
    }

    pub fn used_bytes(&self) -> usize {
        self.buf.len() * 8
    }

    pub fn capacity_bytes(&self) -> usize {
        self.buf.capacity() * 8
    }

    pub fn live_marks(&self) -> usize {
        self.marks.len()
    }
}

impl Default for Arena {
    fn default() -> Self {
        Arena::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_reuse_after_rollback_is_exact() {
        // Нулевой след: после rollback следующий alloc возвращает ТОТ ЖЕ offset.
        let mut a = Arena::new();
        let m = a.push_mark();
        let o1 = a.alloc(64).unwrap();
        a.write_u64(o1, 42);
        assert_eq!(a.read_u64(o1), 42);
        a.rollback(m);
        assert_eq!(a.used_bytes(), m);
        // снова та же область — детерминированное переиспользование
        let o2 = a.alloc(64).unwrap();
        assert_eq!(o1, o2, "rollback не освободил место детерминированно");
        a.write_u64(o2, 7);
        assert_eq!(a.read_u64(o2), 7);
    }

    #[test]
    fn rollback_many_objects_o1() {
        let mut a = Arena::new();
        let m = a.push_mark();
        for i in 0..100_000u64 {
            let o = a.alloc(16).unwrap();
            a.write_u64(o, i);
        }
        assert_eq!(a.used_bytes(), m + 100_000 * 16);
        a.rollback(m);
        assert_eq!(a.used_bytes(), m, "rollback должен быть O(1)-truncate");
        assert_eq!(a.live_marks(), 0);
    }

    #[test]
    fn nested_marks_commit_and_abort() {
        let mut a = Arena::new();
        let outer = a.push_mark();
        let o0 = a.alloc(8).unwrap();
        let _ = o0;

        let _inner = a.push_mark();
        let o1 = a.alloc(8).unwrap();
        let _ = o1;
        a.commit(); // inner: данные остаются

        a.rollback(outer); // откат всего после outer
        assert_eq!(a.used_bytes(), outer);
        assert_eq!(a.live_marks(), 0);
    }

    #[test]
    fn reset_and_capacity() {
        let mut a = Arena::new();
        let m = a.push_mark();
        a.alloc(4096).unwrap();
        assert!(a.used_bytes() > m);
        a.reset();
        assert_eq!(a.used_bytes(), 0);
        assert_eq!(a.live_marks(), 0);
        assert!(a.capacity_bytes() > 0, "capacity сохраняется после reset");
    }

    #[test]
    fn no_metadata_no_alloc_headers() {
        // Ноль per-object заголовков: used == сумме запрошенного (по словам).
        let mut a = Arena::new();
        let m = a.push_mark();
        for _ in 0..1000 {
            a.alloc(8).unwrap();
        }
        assert_eq!(a.used_bytes() - m, 1000 * 8);
        a.rollback(m);
    }
}
