//! phase-server: маленький stateful-сервис с транзакциями и undo.
//!
//! Стек: сетевой + прикладной. Каждая команда клиента — ГРАНИЦА фазы.
//! Внутри транзакции (BEGIN..COMMIT/ROLLBACK) команды накапливают журнал
//! ИНВЕРСИЙ (O(1) на команду — это решения/инверсии на границах, не копии
//! состояния); COMMIT фиксирует, ROLLBACK откатывает всю транзакцию,
//! UNDO откатывает последние N закоммиченных команд. AUDIT возвращает
//! дайджест по SINT hash-цепочке. Всё в одном потоке — детерминизм.

use std::collections::HashMap;

#[derive(Default)]
pub struct KV {
    map: HashMap<String, String>,
    /// глобальный журнал инверсий закоммиченных операций (для UNDO)
    undo: Vec<(String, Option<String>)>,
    /// активная транзакция
    tx: Option<Vec<(String, Option<String>)>>,
}

fn digest(map: &HashMap<String, String>) -> u64 {
    let prime = 0x100000001b3u64;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    for k in keys {
        let v = &map[k];
        for b in k.as_bytes().iter().chain(v.as_bytes().iter()) {
            h = (h ^ *b as u64).wrapping_mul(prime);
        }
    }
    h
}

fn apply_inverse(map: &mut HashMap<String, String>, rec: &(String, Option<String>)) {
    match &rec.1 {
        Some(v) => {
            map.insert(rec.0.clone(), v.clone());
        }
        None => {
            map.remove(&rec.0);
        }
    }
}

impl KV {
    pub fn new() -> Self {
        KV::default()
    }

    /// SINT-дайджест состояния (аудит).
    pub fn audit_digest(&self) -> u64 {
        digest(&self.map)
    }

    /// Детерминированный снапшот: "k v" по одной на строку, ключи отсортированы.
    pub fn snapshot(&self) -> String {
        let mut keys: Vec<&String> = self.map.keys().collect();
        keys.sort();
        keys.iter()
            .map(|k| format!("{} {}", k, self.map[*k]))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Загрузить состояние из снапшота (рестарт из капсулы).
    pub fn from_snapshot(s: &str) -> KV {
        let mut kv = KV::new();
        for line in s.lines() {
            if let Some((k, v)) = line.split_once(' ') {
                kv.map.insert(k.to_string(), v.to_string());
            }
        }
        kv
    }

    /// Обработать одну строку команды. Возвращает ответ ("" = закрыть).
    pub fn handle(&mut self, line: &str) -> String {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.is_empty() {
            return "err: пустая команда".into();
        }
        let rec = |k: String, old: Option<String>| (k, old);
        match t[0].to_ascii_uppercase().as_str() {
            "SET" if t.len() == 3 => {
                let k = t[1].to_string();
                let v = t[2].to_string();
                let old = self.map.insert(k.clone(), v);
                let r = rec(k, old);
                match &mut self.tx {
                    Some(buf) => buf.push(r),
                    None => self.undo.push(r),
                }
                "ok".into()
            }
            "GET" if t.len() == 2 => match self.map.get(t[1]) {
                Some(v) => v.clone(),
                None => "nil".into(),
            },
            "DEL" if t.len() == 2 => {
                let old = self.map.remove(t[1]);
                let r = rec(t[1].to_string(), old);
                match &mut self.tx {
                    Some(buf) => buf.push(r),
                    None => self.undo.push(r),
                }
                "ok".into()
            }
            "BEGIN" => {
                if self.tx.is_some() {
                    "err: уже в транзакции".into()
                } else {
                    self.tx = Some(Vec::new());
                    "tx begin".into()
                }
            }
            "COMMIT" => match self.tx.take() {
                Some(buf) => {
                    let n = buf.len();
                    self.undo.extend(buf);
                    format!("committed {n} op(s)")
                }
                None => "err: нет транзакции".into(),
            },
            "ROLLBACK" => match self.tx.take() {
                Some(buf) => {
                    for r in buf.iter().rev() {
                        apply_inverse(&mut self.map, r);
                    }
                    format!("rolled back {} op(s)", buf.len())
                }
                None => "err: нет транзакции".into(),
            },
            "UNDO" => {
                if self.tx.is_some() {
                    return "err: undo внутри транзакции запрещён".into();
                }
                let n: usize = t
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1)
                    .min(self.undo.len());
                let mut done = 0;
                for _ in 0..n {
                    if let Some(r) = self.undo.pop() {
                        apply_inverse(&mut self.map, &r);
                        done += 1;
                    }
                }
                format!("undone {done} op(s)")
            }
            "AUDIT" => {
                format!(
                    "digest=0x{:016x} keys={}",
                    digest(&self.map),
                    self.map.len()
                )
            }
            "QUIT" => String::new(),
            _ => "err: неизвестная команда".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_rollback_and_commit() {
        let mut kv = KV::new();
        assert_eq!(kv.handle("SET a 1"), "ok");
        assert_eq!(kv.handle("BEGIN"), "tx begin");
        assert_eq!(kv.handle("SET a 2"), "ok");
        assert_eq!(kv.handle("SET b 3"), "ok");
        assert_eq!(kv.handle("GET a"), "2");
        assert_eq!(kv.handle("ROLLBACK"), "rolled back 2 op(s)");
        assert_eq!(kv.handle("GET a"), "1");
        assert_eq!(kv.handle("GET b"), "nil");
        // повтор: коммит
        kv.handle("BEGIN");
        kv.handle("SET b 9");
        assert_eq!(kv.handle("COMMIT"), "committed 1 op(s)");
        assert_eq!(kv.handle("GET b"), "9");
    }

    #[test]
    fn undo_pops_committed() {
        let mut kv = KV::new();
        kv.handle("SET a 1");
        kv.handle("SET a 2");
        assert_eq!(kv.handle("GET a"), "2");
        assert_eq!(kv.handle("UNDO"), "undone 1 op(s)");
        assert_eq!(kv.handle("GET a"), "1");
        assert_eq!(kv.handle("UNDO"), "undone 1 op(s)");
        assert_eq!(kv.handle("GET a"), "nil");
        assert_eq!(kv.handle("UNDO"), "undone 0 op(s)");
    }

    #[test]
    fn audit_is_deterministic_and_changes() {
        let mut kv = KV::new();
        let d0 = kv.handle("AUDIT");
        kv.handle("SET a 1");
        kv.handle("SET b 2");
        let d1 = kv.handle("AUDIT");
        assert_ne!(d0, d1);
        // повтор той же последовательности даёт тот же дайджест
        let mut kv2 = KV::new();
        kv2.handle("SET a 1");
        kv2.handle("SET b 2");
        assert_eq!(kv2.handle("AUDIT"), d1, "детерминизм нарушен");
    }

    #[test]
    fn delete_inverse_restores() {
        let mut kv = KV::new();
        kv.handle("SET a 1");
        kv.handle("DEL a");
        assert_eq!(kv.handle("GET a"), "nil");
        kv.handle("UNDO");
        assert_eq!(kv.handle("GET a"), "1");
    }
}
