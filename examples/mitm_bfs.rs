//! Ось B (MITM): двунаправленный поиск пути на сетке — sqrt-выигрыш честно.
//!
//! Meet-in-the-middle даёт ускорение там, где есть структура: прямой BFS из A
//! и обратный из B встречаются на «экваторе». Замеряем число раскрытых
//! вершин (работа поиска) uni- vs bi-directional на случайных лабиринтах.
//! Пути обязаны совпасть по длине. НЕ обещаем 2^N/2: это эмпирика на графах.

use std::collections::VecDeque;
use std::time::Instant;

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

const W: usize = 200;
const H: usize = 200;
const N: usize = W * H;

fn idx(x: usize, y: usize) -> usize {
    y * W + x
}

fn neighbors(v: usize, out: &mut [usize; 4]) -> usize {
    let x = v % W;
    let y = v / W;
    let mut k = 0;
    if x > 0 {
        out[k] = v - 1;
        k += 1;
    }
    if x + 1 < W {
        out[k] = v + 1;
        k += 1;
    }
    if y > 0 {
        out[k] = v - W;
        k += 1;
    }
    if y + 1 < H {
        out[k] = v + W;
        k += 1;
    }
    k
}

/// Однонаправленный BFS. Возвращает (длина пути или None, раскрыто вершин).
fn bfs_uni(walls: &[bool], start: usize, goal: usize) -> (Option<u32>, u32) {
    if walls[start] || walls[goal] {
        return (None, 0);
    }
    let mut dist = vec![u32::MAX; N];
    let mut q = VecDeque::new();
    dist[start] = 0;
    q.push_back(start);
    let mut expanded = 0u32;
    while let Some(v) = q.pop_front() {
        expanded += 1;
        if v == goal {
            return (Some(dist[v]), expanded);
        }
        let d = dist[v] + 1;
        let mut nb = [0usize; 4];
        let cnt = neighbors(v, &mut nb);
        for &n in &nb[..cnt] {
            if !walls[n] && dist[n] == u32::MAX {
                dist[n] = d;
                q.push_back(n);
            }
        }
    }
    (None, expanded)
}

/// Двунаправленный BFS с корректным bound-завершением.
/// Расширяем меньший фронт; пересечение даёт верхнюю границу best; останавливаемся,
/// когда минимальные глубины фронтов в сумме > best (дальше улучшить нельзя).
fn bfs_bi(walls: &[bool], start: usize, goal: usize) -> (Option<u32>, u32) {
    if walls[start] || walls[goal] {
        return (None, 0);
    }
    if start == goal {
        return (Some(0), 1);
    }
    let mut da = vec![u32::MAX; N];
    let mut db = vec![u32::MAX; N];
    let mut qa = VecDeque::new();
    let mut qb = VecDeque::new();
    da[start] = 0;
    db[goal] = 0;
    qa.push_back(start);
    qb.push_back(goal);
    let mut expanded = 0u32;
    let mut best = u32::MAX;
    loop {
        if qa.is_empty() || qb.is_empty() {
            break;
        }
        // bound: минимальные глубины фронтов в сумме уже >= best -> улучшить нельзя
        let ca = da[*qa.front().unwrap()];
        let cb = db[*qb.front().unwrap()];
        if best != u32::MAX && ca + cb >= best {
            break;
        }
        // расширяем меньший фронт
        if qa.len() <= qb.len() {
            let v = qa.pop_front().unwrap();
            expanded += 1;
            if db[v] != u32::MAX {
                best = best.min(da[v] + db[v]);
            }
            let d = da[v] + 1;
            if d >= best {
                continue;
            }
            let mut nb = [0usize; 4];
            let cnt = neighbors(v, &mut nb);
            for &n in &nb[..cnt] {
                if !walls[n] && da[n] == u32::MAX {
                    da[n] = d;
                    qa.push_back(n);
                }
            }
        } else {
            let v = qb.pop_front().unwrap();
            expanded += 1;
            if da[v] != u32::MAX {
                best = best.min(da[v] + db[v]);
            }
            let d = db[v] + 1;
            if d >= best {
                continue;
            }
            let mut nb = [0usize; 4];
            let cnt = neighbors(v, &mut nb);
            for &n in &nb[..cnt] {
                if !walls[n] && db[n] == u32::MAX {
                    db[n] = d;
                    qb.push_back(n);
                }
            }
        }
    }
    if best == u32::MAX {
        (None, expanded)
    } else {
        (Some(best), expanded)
    }
}

fn main() {
    let mut rng = Xs(0xB1D1);
    let trials = 40usize;
    let (mut e_uni, mut e_bi) = (0u64, 0u64);
    let mut paths_ok = 0u64;
    let mut with_path = 0u64;
    let t0 = Instant::now();

    for t in 0..trials {
        let mut walls = vec![false; N];
        // случайные стены ~25%
        for w in walls.iter_mut() {
            *w = rng.next() % 100 < 25;
        }
        // гарантируем проходимые старт/цель
        walls[0] = false;
        walls[N - 1] = false;
        let start = idx(
            3 + (rng.next() % (W as u64 - 6)) as usize,
            3 + (rng.next() % (H as u64 - 6)) as usize,
        );
        let goal = idx(
            W - 4 - (rng.next() % (W as u64 - 6)) as usize,
            H - 4 - (rng.next() % (H as u64 - 6)) as usize,
        );
        walls[start] = false;
        walls[goal] = false;
        let _ = t;

        let (pu, eu) = bfs_uni(&walls, start, goal);
        let (pb, eb) = bfs_bi(&walls, start, goal);
        if pu.is_some() {
            with_path += 1;
            assert_eq!(pu, pb, "длины путей разошлись (uni vs bi)");
            paths_ok += 1;
            e_uni += eu as u64;
            e_bi += eb as u64;
        } else {
            assert!(pb.is_none(), "bi нашёл путь, uni нет");
        }
    }
    let dt = t0.elapsed();

    println!("MITM (ось B): сетка {W}×{H}, {trials} лабиринтов, ~25% стен");
    println!("путей найдено: {paths_ok}/{with_path}");
    println!(
        "раскрыто вершин: uni={e_uni}  bi={e_bi}  ({:.1}× меньше)",
        e_uni as f64 / e_bi as f64
    );
    println!(
        "sqrt-ориентир: {:.2}× (эмпирика, не гарантия 2^N/2)",
        (e_uni as f64 / e_bi as f64).sqrt()
    );
    println!("длины путей uni == bi (assert); время {:?}", dt);
    println!("вывод: MITM честно работает на поиске с декомпозицией — ось B открыта");
}
