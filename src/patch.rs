//! phase-patch: атомарное all-or-nothing применение unified-diff.
//!
//! Приколы парадигмы, вшитые в утилиту:
//! - **all-or-nothing** (реверсивная семантика): хунки накапливаются в памяти,
//!   файл не трогается до полного успеха; конфликт хунка = «откат» — файл
//!   остаётся бит-в-бит исходным, ноль временных файлов (нет temp+rename);
//! - **детерминизм + SINT hash-аудит**: дайджест результата по hash-цепочке;
//! - готова к капсуле .eml (L4) как «траектория применения».

/// Результат применения патча.
pub struct PatchResult {
    pub text: String,
    /// Строк результата (после split) — для дайджеста.
    pub digest: u64,
}

pub fn fnv_digest(s: &str) -> u64 {
    fnv1a(s)
}

fn fnv1a(s: &str) -> u64 {
    let prime = 0x100000001b3u64;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in s.as_bytes() {
        h = (h ^ b as u64).wrapping_mul(prime);
    }
    h
}

fn chain_step(prev: u64, x: u64) -> u64 {
    let prime = 0x100000001b3u64;
    (0xcbf2_9ce4_8422_2325u64 ^ prev)
        .wrapping_add(x)
        .wrapping_mul(prime)
}

/// Применить unified-diff к тексту. При ошибке любого хунка возвращает Err —
/// исходный текст НЕ изменяется (мы работаем над копией до конца).
pub fn apply_unified(orig: &str, diff: &str) -> Result<PatchResult, String> {
    // 1) парсинг хунков
    let mut hunks: Vec<(usize, Vec<(char, &str)>)> = Vec::new();
    let lines: Vec<&str> = diff.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let l = lines[i];
        if let Some(h) = l.strip_prefix("@@ ") {
            // формат: -old,count +new,count @@...
            let rest = h.split(" @@").next().unwrap_or("");
            let (oldp, newp) = rest.split_once(" +").ok_or("bad hunk header")?;
            let old_line: usize = oldp
                .trim_start_matches('-')
                .split(',')
                .next()
                .unwrap()
                .parse()
                .map_err(|_| "bad old line")?;
            let old_cnt: usize = oldp
                .split(',')
                .nth(1)
                .map(|s| s.parse().unwrap_or(1))
                .unwrap_or(1);
            let new_cnt: usize = newp
                .trim_start()
                .split(',')
                .nth(1)
                .map(|s| s.parse().unwrap_or(1))
                .unwrap_or(1);
            let _ = new_cnt;
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].starts_with("@@") {
                let s = lines[i];
                let kind = match s.as_bytes().first() {
                    Some(b' ') => ' ',
                    Some(b'-') => '-',
                    Some(b'+') => '+',
                    Some(b'\\') => {
                        i += 1; // "\ No newline..." — пропускаем
                        continue;
                    }
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                body.push((kind, &s[1..]));
                i += 1;
            }
            let mut removed = 0;
            let mut _added = 0;
            let mut ctx = 0;
            for (k, _) in &body {
                match k {
                    '-' => removed += 1,
                    '+' => _added += 1,
                    _ => ctx += 1,
                }
            }
            // old_count в заголовке = контекст + удалённые строки старой стороны
            if removed + ctx != old_cnt {
                return Err(format!(
                    "хунок {}-й: (removed {removed} + ctx {ctx}) != old_count {old_cnt}",
                    hunks.len() + 1
                ));
            }
            // old_line 1-based -> 0-based позиция начала хунка
            hunks.push((old_line.saturating_sub(1), body));
        } else {
            i += 1;
        }
    }

    // 2) применение поверх копии: строки оригинала
    let mut out: Vec<String> = orig.lines().map(|s| s.to_string()).collect();
    let mut cum = 0isize; // сдвиг от предыдущих хунков
    for (hi, (start, body)) in hunks.iter().enumerate() {
        let idx = (*start as isize + cum) as usize;
        // собрать ожидаемые старые строки (контекст+удалённые)
        let old_lines: Vec<&str> = body
            .iter()
            .filter(|(k, _)| *k != '+')
            .map(|(_, s)| *s)
            .collect();
        if idx + old_lines.len() > out.len() {
            return Err(format!("хунок {}: выходит за конец файла", hi + 1));
        }
        for (k, expect) in old_lines.iter().enumerate() {
            if out[idx + k] != *expect {
                return Err(format!(
                    "хунок {}: контекст не совпал на строке {} (ожидалось {:?})",
                    hi + 1,
                    idx + k + 1,
                    expect
                ));
            }
        }
        // заменяем old_count строк на новые (контекст+' ')
        let added_lines: Vec<&str> = body
            .iter()
            .filter(|(k, _)| *k != '-')
            .map(|(_, s)| *s)
            .collect();
        out.splice(
            idx..idx + old_lines.len(),
            added_lines.iter().map(|s| s.to_string()),
        );
        cum += added_lines.len() as isize - old_lines.len() as isize;
    }

    // 3) дайджест (SINT hash-цепочка)
    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    for ln in &out {
        digest = chain_step(digest, fnv1a(ln));
    }
    let text = out.join("\n");
    // сохранить финальный перевод строки, если оригинал его имел
    let text = if orig.ends_with('\n') && !text.ends_with('\n') {
        format!("{text}\n")
    } else {
        text
    };
    Ok(PatchResult { text, digest })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_simple_unified_diff() {
        let orig = "alpha\nbeta\ngamma\ndelta\n";
        let diff = "\
--- a/f.txt
+++ b/f.txt
@@ -1,4 +1,4 @@
 alpha
-beta
+BETA
 gamma
 delta
";
        let r = apply_unified(orig, diff).unwrap();
        assert_eq!(r.text, "alpha\nBETA\ngamma\ndelta\n");
    }

    #[test]
    fn conflict_leaves_original_untouched() {
        let orig = "alpha\nWRONG\ngamma\n";
        let diff = "\
@@ -1,3 +1,3 @@
 alpha
-beta
+beta
 gamma
";
        let r = apply_unified(orig, diff);
        assert!(r.is_err(), "конфликт обязан вернуть ошибку");
        // оригинал не менялся — мы его и не трогали (в этом суть)
        assert_eq!(orig, "alpha\nWRONG\ngamma\n");
    }

    #[test]
    fn multiple_hunks_shift_positions() {
        let orig = "a\nb\nc\nd\ne\nf\n";
        let diff = "\
@@ -1,3 +1,4 @@
 a
 b
+bb
 c
@@ -4,3 +5,3 @@
 d
 e
-f
+F
";
        let r = apply_unified(orig, diff).unwrap();
        assert_eq!(r.text, "a\nb\nbb\nc\nd\ne\nF\n");
        // детерминизм
        let r2 = apply_unified(orig, diff).unwrap();
        assert_eq!(r.digest, r2.digest);
    }
}
