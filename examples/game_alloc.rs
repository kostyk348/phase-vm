//! Кастомный ИГРОВОЙ аллокатор: io_uring + фазовые арены + пул сущностей.
//!
//! Три прикола PHASE в одном игровом цикле:
//!  1) КАДР = ФАЗА: frame_arena (mark в начале кадра, reset в конце O(1)) —
//!     временные объекты кадра умирают пачкой, ноль per-object free;
//!  2) СЦЕНА = ФАЗА ЗАГРУЗКИ: ассеты читаются АСИНХРОННО через io_uring
//!     (батч чтений = фаза I/O, буферы из asset_arena — не из кучи);
//!  3) СУЩНОСТИ = ПУЛ (фиксированный слот-массив): спавн/деспавн без malloc,
//!     детерминированный тик, дайджест на кадр (SINT-аудит).
//!
//! Всё детерминировано: два прогона дают одинаковый финальный дайджест.

use std::os::unix::io::AsRawFd;
use std::time::Instant;

use phase_vm::alloc::Arena;
use phase_vm::uring::Uring;

const FRAMES: usize = 400;
const ENTITIES: usize = 512;
const N_ASSETS: usize = 6;

struct Ent {
    active: bool,
    pos: u64,
    vel: u64,
    color: u32,
}

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn main() {
    // --- готовим «ассеты» (детерминированные файлы) ---
    let dir = "/tmp/ga_assets";
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let mut rng = Xs(0x61E7);
    let mut asset_sizes = Vec::new();
    for a in 0..N_ASSETS {
        let n = 256 + (rng.next() % 4096) as usize;
        let mut data = Vec::with_capacity(n);
        for _ in 0..n {
            data.push((rng.next() & 0xff) as u8);
        }
        std::fs::write(format!("{dir}/a{a}.bin"), &data).unwrap();
        asset_sizes.push(n);
    }

    // --- пул сущностей + арены ---
    let mut ents: Vec<Ent> = (0..ENTITIES)
        .map(|_| Ent {
            active: false,
            pos: 0,
            vel: 0,
            color: 0,
        })
        .collect();
    let mut frame_arena = Arena::new();
    let mut asset_arena = Arena::with_capacity_words(1 << 16); // 512KB: без realloc во время загрузки
    let mut ur = Uring::new(64).expect("io_uring");

    // открываем файлы ассетов заранее
    let files: Vec<std::fs::File> = (0..N_ASSETS)
        .map(|a| std::fs::File::open(format!("{dir}/a{a}.bin")).unwrap())
        .collect();

    let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
    let mut assets_loaded = [false; N_ASSETS];
    let (mut spawns, mut despawns) = (0u64, 0u64);
    let mut load_enters = 0u64;
    let mut rng = Xs(0xB0A7);

    let t0 = Instant::now();
    for frame in 0..FRAMES {
        // --- фаза кадра: временное ---
        let fmark = frame_arena.push_mark();

        // --- фаза загрузки: асинхронно читаем до 2 ассетов за кадр ---
        let mut pending = 0usize;
        for a in 0..N_ASSETS {
            if !assets_loaded[a] && pending < 2 {
                let off = asset_arena.alloc(asset_sizes[a]).unwrap();
                let ptr = asset_arena.words().as_ptr() as *mut u8;
                let buf = unsafe { ptr.add(off) };
                ur.enqueue_read(files[a].as_raw_fd(), buf, asset_sizes[a], 0, a as u64);
                assets_loaded[a] = true;
                pending += 1;
            }
        }
        if pending > 0 {
            ur.submit().unwrap();
            load_enters += 1;
            for (tag, res) in ur.wait(pending) {
                assert_eq!(
                    res as usize, asset_sizes[tag as usize],
                    "ассет {tag} прочитан не полностью"
                );
            }
        }

        // --- игровой тик: спавн/деспавн/движение (пул, без malloc) ---
        for _ in 0..8 {
            let i = (rng.next() % ENTITIES as u64) as usize;
            if !ents[i].active {
                ents[i].active = true;
                ents[i].pos = rng.next() % 1_000_000;
                ents[i].vel = 1 + rng.next() % 10;
                ents[i].color = (rng.next() & 0xffffff) as u32;
                spawns += 1;
            } else {
                ents[i].active = false;
                despawns += 1;
            }
        }
        // временный объект кадра (напр. партикл-буфер) в frame_arena
        let tmp = frame_arena.alloc(256).unwrap();
        frame_arena.write_u64(tmp, frame as u64);
        // детерминированный апдейт активных + дайджест кадра
        for e in ents.iter_mut().filter(|e| e.active) {
            e.pos = e.pos.wrapping_add(e.vel);
        }
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for e in ents.iter().filter(|e| e.active) {
            h = h.wrapping_mul(31).wrapping_add(e.pos ^ (e.color as u64));
        }
        digest = phase_vm::state::State::chain_step(digest, h);

        // --- конец кадра: времена кадра умирают O(1) ---
        frame_arena.rollback(fmark);
    }
    let dt = t0.elapsed();

    let active: usize = ents.iter().filter(|e| e.active).count();
    println!("game-alloc: FRAMES={FRAMES}, пул сущностей={ENTITIES}, ассетов={N_ASSETS}");
    println!(
        "спавнов={spawns} деспавнов={despawns} активных на конце={active} (пул, 0 malloc на сущность)"
    );
    println!(
        "io_uring: загрузка {N_ASSETS} ассетов = {load_enters} enter-ов (батч = фаза I/O); буферы в asset_arena"
    );
    println!(
        "арены: frame-времена сброшены {FRAMES} раз O(1); asset_arena использует {} B",
        asset_arena.used_bytes()
    );
    println!(
        "кадр: {:?} ({} ns/frame)",
        dt,
        dt.as_nanos() as usize / FRAMES
    );
    println!("детерминизм-дайджест: 0x{digest:016x}");
    let _ = &mut frame_arena;
}
