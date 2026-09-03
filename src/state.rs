//! Состояние машины: регистровый файл `u64` + линейная слово-адресуемая память.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub regs: Vec<u64>,
    /// Линейная память (слова u64). Инструкции madd/msub/mxor/rmadd/rmsub/mset.
    pub mem: Vec<u64>,
}

impl State {
    pub fn new(nregs: usize, nmem: usize) -> Self {
        State {
            regs: vec![0; nregs],
            mem: vec![0; nmem],
        }
    }

    pub fn nregs(&self) -> usize {
        self.regs.len()
    }

    pub fn nmem(&self) -> usize {
        self.mem.len()
    }

    pub fn get(&self, r: u8) -> u64 {
        self.regs[r as usize]
    }

    pub fn set(&mut self, r: u8, v: u64) {
        self.regs[r as usize] = v;
    }

    /// Детерминированный псевдослучайный регистровый файл + память (xorshift64*).
    pub fn random(nregs: usize, nmem: usize, seed: u64) -> Self {
        let mut x = seed.max(1);
        let mut next = move || {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        State {
            regs: (0..nregs).map(|_| next()).collect(),
            mem: (0..nmem).map(|_| next()).collect(),
        }
    }
}

impl State {
    /// FNV-1a 64 над regs+mem — детерминированный снимок состояния.
    /// (Прививка из SINT: hash-chain «дёшев и окупается» — здесь аудит
    /// границ фаз и проверка детерминизма resim/rollback.)
    pub fn hash(&self) -> u64 {
        let prime = 0x100000001b3u64;
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &w in &self.regs {
            h = (h ^ w).wrapping_mul(prime);
        }
        for &w in &self.mem {
            h = (h ^ w).wrapping_mul(prime);
        }
        h
    }

    /// Один шаг hash-цепочки: chain' = FNV(chain xor hash(state)).
    /// Позволяет построить нестираемый журнал границ фаз (как prev_hash в SINT).
    pub fn chain_step(prev: u64, state_hash: u64) -> u64 {
        let prime = 0x100000001b3u64;
        let mut h = 0xcbf2_9ce4_8422_2325u64 ^ prev;
        h = (h ^ state_hash).wrapping_mul(prime);
        h
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let items: Vec<String> = self
            .regs
            .iter()
            .enumerate()
            .map(|(i, v)| format!("r{}=0x{:016x}", i, v))
            .collect();
        write!(f, "{}", items.join(" "))
    }
}

/// Память для показа: до 8 слов, при большей — с многоточием.
pub fn fmt_mem(state: &State, max: usize) -> String {
    let n = state.mem.len();
    if n == 0 {
        return "(память не выделена, --mem N)".into();
    }
    let shown = n.min(max);
    let mut s = String::new();
    for i in 0..shown {
        s.push_str(&format!("m[{}]=0x{:016x} ", i, state.mem[i]));
    }
    if n > shown {
        s.push_str(&format!("… (всего {} слов)", n));
    }
    s
}
