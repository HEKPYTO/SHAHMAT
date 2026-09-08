//! Slider dispatch: exactly one resident table set.
//!
//! Gate contract (arch × feature matrix; `pext`-misuse rejection lives in
//! `lib.rs` and is not repeated here):
//! - `aarch64` → HQ + `rbit` (2 KB). No runtime dispatch (`rbit` is base-A64).
//!   The `magic-black` feature is ignored on this arch; HQ is the only
//!   resident set.
//! - `wasm32` → Black fixed-shift magic (~863 KB measured). `min-mem` builds use
//!   `--no-default-features` for HQ-only (2 KB); see below.
//! - `x86_64` + `pext` → runtime CPUID via [`std::arch`]: BMI2 present AND
//!   vendor-model NOT in the slow list (`znver1`/`znver2`/`bdver4`) → PEXT
//!   (~842 KB measured); else exactly one fallback resident — Black magic
//!   (~863 KB measured) or AVX2 Dual-HQ — by the AVX2 CPU predicate.
//! - `x86_64` without `pext` → Black magic only.
//! - `min-mem` + `magic-black` is a conflicting combination (documented, not
//!   compiled out): HQ-only is built with `--no-default-features --features
//!   min-mem`. Enabling both does not yield a single resident set.
//!
//! This phase wires the selection function + slow-list predicate and their
//! unit tests. The table sets land in this file later behind the same engine selection.

// `min-mem` (HQ-only 2 KB) and `pext` (~843 KB) each name a different resident
// set; a binary combining them could not keep the single-resident promise.
#[cfg(all(feature = "min-mem", feature = "pext"))]
compile_error!(
    "features `min-mem` and `pext` are mutually exclusive: exactly one slider table set may be resident"
);

/// CPU vendor from CPUID leaf 0 (`EBX, EDX, ECX` words).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CpuVendor {
    /// AMD (`AuthenticAMD`); slowness depends on family/model (see [`bmi2_is_slow`]).
    Amd,
    /// Intel (`GenuineIntel`); never slow-BMI2.
    Intel,
    /// Any other vendor string; never slow-BMI2.
    Other,
}

/// Decode CPUID leaf-0 vendor words into a [`CpuVendor`].
///
/// Pure over injected words: host-independent.
pub fn vendor_from_cpuid_words(ebx: u32, edx: u32, ecx: u32) -> CpuVendor {
    // "AuthenticAMD" / "GenuineIntel" in leaf-0 word order: EBX, EDX, ECX.
    if ebx == 0x6875_7541 && edx == 0x6974_6e65 && ecx == 0x444d_4163 {
        CpuVendor::Amd
    } else if ebx == 0x756e_6547 && edx == 0x4965_6e69 && ecx == 0x6c65_746e {
        CpuVendor::Intel
    } else {
        CpuVendor::Other
    }
}

/// Decode CPUID leaf-1 `EAX` into `(display_family, display_model)`.
///
/// Pure over the injected `EAX` value: host-independent.
pub fn decode_family_model(eax: u32) -> (u32, u32) {
    let base_family = (eax >> 8) & 0x0f;
    let base_model = (eax >> 4) & 0x0f;
    let ext_model = (eax >> 16) & 0x0f;
    let ext_family = (eax >> 20) & 0xff;
    let family = if base_family == 0x0f {
        base_family + ext_family
    } else {
        base_family
    };
    let model = if base_family == 0x06 || base_family == 0x0f {
        (ext_model << 4) | base_model
    } else {
        base_model
    };
    (family, model)
}

/// Slow-BMI2 predicate: vendor-model in `znver1`/`znver2`/`bdver4`.
///
/// Pure over injected values: host-independent.
/// - AMD family `0x17` (Zen1 `znver1` + Zen2 `znver2`, all models) → slow.
/// - AMD family `0x15` models `0x60..=0x7f` (Excavator `bdver4`) → slow.
/// - Everything else (Intel incl. Haswell, AMD Zen3+ incl. family `0x19`) → fast.
pub fn bmi2_is_slow(vendor: CpuVendor, family: u32, model: u32) -> bool {
    match vendor {
        CpuVendor::Amd => family == 0x17 || (family == 0x15 && (0x60..=0x7f).contains(&model)),
        _ => false,
    }
}

/// Resident slider engine: exactly one large table set per binary.
///
/// Matched ONCE per dispatch site — exhaustively, so a typo can never
/// silently fall through to a wrong engine the way string codes could.
/// - `Hq` ..... hyperbola quintessence + `rbit` (2 KiB line tables).
/// - `Black` . Black fixed-shift magic (~863 KB measured).
/// - `Pext` .. x86_64 BMI2 gather (~842 KB measured).
/// - `Avx2Hq`  AVX2-present fallback: runs on the HQ core below (shared 2 KiB
///   line masks; the AVX2-vectorised form is a later perf opportunity,
///   results are identical).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Engine {
    /// Hyperbola quintessence + `rbit` (2 KiB line tables).
    Hq,
    /// Black fixed-shift magic (~863 KB measured).
    Black,
    /// x86_64 BMI2 gather (~842 KB measured); selected only when BMI2 is
    /// present and not slow (see [`select_engine`]).
    Pext,
    /// AVX2-present fallback: runs on the HQ core (identical results; the
    /// AVX2-vectorised form is a later perf opportunity).
    Avx2Hq,
}
/// x86_64 path selection: BMI2 present AND not slow → PEXT, else the
/// single fallback resident (`Avx2Hq` iff AVX2, else `Black`).
///
/// Pure over injected flags: host-independent.
pub fn select_engine(bmi2: bool, slow_bmi2: bool, avx2: bool) -> Engine {
    if bmi2 && !slow_bmi2 {
        Engine::Pext
    } else if avx2 {
        Engine::Avx2Hq
    } else {
        Engine::Black
    }
}

/// Runtime slow-list probe via [`std::arch`] CPUID (x86_64 only).
// `__cpuid` is safe on recent toolchains; the `unsafe` block stays for older ones.
#[cfg(all(target_arch = "x86_64", feature = "pext"))]
#[allow(unused_unsafe)]
fn cpu_is_slow_bmi2() -> bool {
    // SAFETY: CPUID leaf 0/1 reads are side-effect free; x86_64-only module.
    unsafe {
        let v = std::arch::x86_64::__cpuid(0);
        let vendor = vendor_from_cpuid_words(v.ebx, v.edx, v.ecx);
        let s = std::arch::x86_64::__cpuid(1);
        let (family, model) = decode_family_model(s.eax);
        bmi2_is_slow(vendor, family, model)
    }
}

/// Resident engine: HQ (`rbit` is base-A64; the `magic-black` feature is ignored here).
#[cfg(target_arch = "aarch64")]
pub fn engine() -> Engine {
    Engine::Hq
}

/// Resident engine: HQ (2 KiB; keeps `min-mem` wasm binaries small).
#[cfg(all(target_arch = "wasm32", feature = "min-mem"))]
pub fn engine() -> Engine {
    Engine::Hq
}

/// Resident engine: Black magic (default wasm tables).
#[cfg(all(target_arch = "wasm32", not(feature = "min-mem")))]
pub fn engine() -> Engine {
    Engine::Black
}

/// Resident engine: HQ (`min-mem` opts out of the Black magic tables).
#[cfg(all(target_arch = "x86_64", not(feature = "pext"), feature = "min-mem"))]
pub fn engine() -> Engine {
    Engine::Hq
}

/// Resident engine: Black magic (default x86_64 tables without `pext`).
#[cfg(all(
    target_arch = "x86_64",
    not(feature = "pext"),
    not(feature = "min-mem")
))]
pub fn engine() -> Engine {
    Engine::Black
}
/// Resident engine: runtime pick — PEXT unless BMI2 is missing or slow, else
/// the single compiled-in fallback (see [`select_engine`]).
#[cfg(all(target_arch = "x86_64", feature = "pext", not(feature = "min-mem")))]
pub fn engine() -> Engine {
    select_engine(
        std::arch::is_x86_feature_detected!("bmi2"),
        cpu_is_slow_bmi2(),
        std::arch::is_x86_feature_detected!("avx2"),
    )
}

// ================================================================
// Phase 1 tables: real slider attacks behind the selection fns.
//
// Resident-set contract (exactly one large set per binary; the
// `engine()` / `select_engine()` fns above pick at runtime
// among what is compiled in):
// - `aarch64` or `min-mem` ........... HQ hyperbola only (2 KiB).
// - `wasm32`, or `x86_64` w/o `pext` . Black fixed-shift magic only.
// - `x86_64` + `pext` ................ PEXT + runtime fallback. The
//   fallback follows `select_engine()`: `Avx2Hq` runs on the
//   HQ core below (shared 2 KiB line masks; the AVX2-vectorised form is
//   a later perf opportunity, results are identical), `Black` on
//   the Black engine. Both fallback engines are compiled in this one
//   configuration, but their large tables live behind `LazyLock` heap
//   storage initialised on first use, so at most one fallback's tables
//   are ever resident per process — never both.
// Queen = rook | bishop via [`queen_attacks`]; it owns no tables.
//
// Attack-path guarantees: no allocation, no `HashMap`, no per-call
// synchronisation beyond the one-time `LazyLock` init check. Per call
// it is a few integer ops plus table loads (HQ), one multiply-shift
// plus one load (Black), or one `pext` plus one load (PEXT).
// ================================================================

/// Guards the `sq < 64` contract. Release cost: zero.
#[inline(always)]
fn check_sq(sq: u8) {
    debug_assert!(sq < 64, "square out of range: {sq}");
}

/// Full rank line through `sq` (slider square included).
const fn rank_line(sq: u8) -> u64 {
    0xffu64 << ((sq >> 3) << 3)
}

/// Full file line through `sq` (slider square included).
const fn file_line(sq: u8) -> u64 {
    0x0101_0101_0101_0101u64 << (sq & 7)
}

/// Full diagonal through `sq` (slider square included): a1–h8 when
/// `anti` is false (file − rank constant), h1–a8 when true (file + rank).
const fn diag_line(sq: u8, anti: bool) -> u64 {
    let f = sq & 7;
    let r = sq >> 3;
    let mut bb = 0u64;
    let mut s: u8 = 0;
    while s < 64 {
        let same = if anti {
            (s & 7) + (s >> 3) == f + r
        } else {
            (s & 7) + r == f + (s >> 3)
        };
        if same {
            bb |= 1u64 << s;
        }
        s += 1;
    }
    bb
}

/// Shared index-mask machinery for the Black/PEXT arms. Compiled exactly
/// when one of those arms can execute (Black on `wasm32`/`x86_64`, PEXT
/// on `pext` `x86_64`; never under `min-mem`, never on `aarch64` where
/// the `magic-black` feature is ignored and HQ is the only resident set).
///
/// Edge-excluded rook occupancy (index) mask: the squares whose occupancy
/// can change the attack set. Edge squares never block — a ray reaches
/// them either way, occupied or not — and neither can the slider square.
#[cfg(all(
    not(feature = "min-mem"),
    any(target_arch = "wasm32", target_arch = "x86_64")
))]
const fn rook_occ_mask(sq: u8) -> u64 {
    ((rank_line(sq) & 0x7e7e_7e7e_7e7e_7e7eu64) | (file_line(sq) & 0x00ff_ffff_ffff_ff00u64))
        & !(1u64 << sq)
}

/// Edge-excluded bishop occupancy (index) mask; same rationale as above.
#[cfg(all(
    not(feature = "min-mem"),
    any(target_arch = "wasm32", target_arch = "x86_64")
))]
const fn bishop_occ_mask(sq: u8) -> u64 {
    ((diag_line(sq, false) | diag_line(sq, true)) & 0x007e_7e7e_7e7e_7e00u64) & !(1u64 << sq)
}

/// Total attack-table entries for one mask family (sum of `2^bits`).
#[cfg(all(
    not(feature = "min-mem"),
    any(target_arch = "wasm32", target_arch = "x86_64")
))]
const fn occ_total_len(rook: bool) -> usize {
    let mut total = 0usize;
    let mut s: u8 = 0;
    while s < 64 {
        let bits = (if rook {
            rook_occ_mask(s)
        } else {
            bishop_occ_mask(s)
        })
        .count_ones();
        total += 1usize << bits;
        s += 1;
    }
    total
}

/// Index masks shared by the Black and PEXT arms (1 KiB static).
#[cfg(all(
    not(feature = "min-mem"),
    any(target_arch = "wasm32", target_arch = "x86_64")
))]
static ROOK_MASK_TAB: [u64; 64] = build_mask_tab(true);
/// Index masks shared by the Black and PEXT arms (1 KiB static).
#[cfg(all(
    not(feature = "min-mem"),
    any(target_arch = "wasm32", target_arch = "x86_64")
))]
static BISHOP_MASK_TAB: [u64; 64] = build_mask_tab(false);

#[cfg(all(
    not(feature = "min-mem"),
    any(target_arch = "wasm32", target_arch = "x86_64")
))]
const fn build_mask_tab(rook: bool) -> [u64; 64] {
    let mut t = [0u64; 64];
    let mut s: u8 = 0;
    while s < 64 {
        t[s as usize] = if rook {
            rook_occ_mask(s)
        } else {
            bishop_occ_mask(s)
        };
        s += 1;
    }
    t
}

/// Plain ray marcher. Init-time only: oracle for table builds and magic
/// search. (The `#[cfg(test)]` oracle below is a separate copy on purpose
/// — a test must not share code with the implementation it checks.)
#[cfg(all(
    not(feature = "min-mem"),
    any(target_arch = "wasm32", target_arch = "x86_64")
))]
fn ray_march(sq: u8, occ: u64, rook: bool) -> u64 {
    const DIRS: [(i8, i8); 8] = [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ];
    let (start, end) = if rook { (0, 4) } else { (4, 8) };
    let f = (sq & 7) as i8;
    let r = (sq >> 3) as i8;
    let mut att = 0u64;
    let mut d = start;
    while d < end {
        let (df, dr) = DIRS[d];
        let mut nf = f + df;
        let mut nr = r + dr;
        while (0..8).contains(&nf) && (0..8).contains(&nr) {
            let s = (nr as u64) * 8 + nf as u64;
            att |= 1u64 << s;
            if occ & (1u64 << s) != 0 {
                break;
            }
            nf += df;
            nr += dr;
        }
        d += 1;
    }
    att
}

/// Portable bit deposit: spreads the low `popcount(mask)` bits of `x` onto
/// the set bits of `mask` (LSB-first, i.e. `_pdep_u64` order). Table-build
/// enumeration only — never on the attack path.
#[cfg(all(
    not(feature = "min-mem"),
    any(target_arch = "wasm32", target_arch = "x86_64")
))]
fn deposit(mut x: u64, mask: u64) -> u64 {
    let mut out = 0u64;
    let mut m = mask;
    while m != 0 {
        let l = 1u64 << m.trailing_zeros();
        if x & 1 != 0 {
            out |= l;
        }
        m &= m - 1;
        x >>= 1;
    }
    out
}

/// Arm 1 — hyperbola quintessence (HQ).
///
/// One ray = `(o - 2r) ^ reverse(reverse(o) - 2·reverse(r))` masked to its
/// line, with the slider square cleared. `u64::reverse_bits` is the
/// portable `rbit` (base-A64, no CPU feature gate needed). Beyond the four
/// 512 B line tables (2 KiB total) this arm is table-free.
#[cfg(any(
    target_arch = "aarch64",
    feature = "min-mem",
    all(target_arch = "x86_64", feature = "pext")
))]
mod hq {
    const fn build(axis: u8) -> [u64; 64] {
        let mut t = [0u64; 64];
        let mut s: u8 = 0;
        while s < 64 {
            t[s as usize] = match axis {
                0 => super::rank_line(s),
                1 => super::file_line(s),
                2 => super::diag_line(s, false),
                _ => super::diag_line(s, true),
            };
            s += 1;
        }
        t
    }

    /// Rank lines, 512 B static.
    pub(super) static RANK: [u64; 64] = build(0);
    /// File lines, 512 B static.
    pub(super) static FILE: [u64; 64] = build(1);
    /// a1–h8 diagonals, 512 B static.
    pub(super) static DIAG: [u64; 64] = build(2);
    /// h1–a8 diagonals, 512 B static.
    pub(super) static ANTI: [u64; 64] = build(3);

    /// Shared HQ line core: `r`-derived quantities (`2r`, `2·reverse(r)`,
    /// `!r`) are passed in so the fused queen computes them once for all
    /// four lines. Per-line work stays identical to [`slide`]: mask the
    /// occupancy, one forward `rbit` round, one reverse round, xor + mask.
    #[inline(always)]
    fn slide_inner(occ: u64, r2: u64, rr2: u64, mask: u64, not_r: u64) -> u64 {
        let o = occ & mask;
        let fwd = o.wrapping_sub(r2);
        let back = o.reverse_bits().wrapping_sub(rr2).reverse_bits();
        (fwd ^ back) & mask & not_r
    }

    #[inline(always)]
    fn slide(occ: u64, sq: u8, mask: u64) -> u64 {
        let r = 1u64 << sq;
        slide_inner(
            occ,
            r.wrapping_mul(2),
            r.reverse_bits().wrapping_mul(2),
            mask,
            !r,
        )
    }

    // Only called from the HQ-arm `queen_attacks` dispatch below; gated so
    // x86_64+pext builds (where `mod hq` compiles but the queen takes the
    // union path) stay warning-free under `-D warnings`.
    #[cfg(any(target_arch = "aarch64", feature = "min-mem"))]
    #[inline(always)]
    fn queen_inner(occ: u64, sq: u8) -> u64 {
        let r = 1u64 << sq;
        let r2 = r.wrapping_mul(2);
        let rr2 = r.reverse_bits().wrapping_mul(2);
        let not_r = !r;
        slide_inner(occ, r2, rr2, RANK[sq as usize], not_r)
            | slide_inner(occ, r2, rr2, FILE[sq as usize], not_r)
            | slide_inner(occ, r2, rr2, DIAG[sq as usize], not_r)
            | slide_inner(occ, r2, rr2, ANTI[sq as usize], not_r)
    }

    #[inline]
    pub(super) fn rook_attacks(sq: u8, occ: u64) -> u64 {
        slide(occ, sq, RANK[sq as usize]) | slide(occ, sq, FILE[sq as usize])
    }

    #[inline]
    pub(super) fn bishop_attacks(sq: u8, occ: u64) -> u64 {
        slide(occ, sq, DIAG[sq as usize]) | slide(occ, sq, ANTI[sq as usize])
    }

    /// Fused queen: one occupancy read, `r`/`reverse(r)`/`2r`/`!r` computed
    /// once and shared across all four lines. Saves per queen call one
    /// dispatch layer plus 3x `rbit` + 3x mul + redundant mask/`!r` work
    /// vs `rook_attacks | bishop_attacks`. HQ-arm only (see `queen_inner`).
    #[cfg(any(target_arch = "aarch64", feature = "min-mem"))]
    #[inline]
    pub(super) fn queen_attacks(sq: u8, occ: u64) -> u64 {
        queen_inner(occ, sq)
    }
}

/// Arm 2 — Black fixed-shift magic.
/// Index = `((occ & mask) * magic) >> (64 - popcount(mask))` into one flat
/// table per piece. Magics are searched once, deterministically (fixed-seed
/// xorshift, suitability heuristic + full collision check over every
/// occupancy subset), the first time an attack fn runs; the resulting
/// tables then serve lock-free reads with zero per-call allocation.
#[cfg(all(
    not(feature = "min-mem"),
    any(target_arch = "wasm32", target_arch = "x86_64")
))]
mod black {
    use std::sync::LazyLock;

    /// Classic fixed-shift totals; pinned at compile time against the masks.
    pub(super) const ROOK_TABLE_LEN: usize = 102_400;
    /// Classic fixed-shift totals; pinned at compile time against the masks.
    pub(super) const BISHOP_TABLE_LEN: usize = 5_248;
    const _: () = assert!(super::occ_total_len(true) == ROOK_TABLE_LEN);
    const _: () = assert!(super::occ_total_len(false) == BISHOP_TABLE_LEN);

    pub(super) struct Tables {
        pub(super) rook_magic: [u64; 64],
        pub(super) bishop_magic: [u64; 64],
        pub(super) rook_shift: [u32; 64],
        pub(super) bishop_shift: [u32; 64],
        pub(super) rook_off: [u32; 64],
        pub(super) bishop_off: [u32; 64],
        pub(super) rook_tbl: Box<[u64]>,
        pub(super) bishop_tbl: Box<[u64]>,
    }

    pub(super) static TABLES: LazyLock<Tables> = LazyLock::new(build);

    /// Deterministic xorshift64* (fixed seed ⇒ reproducible magic search).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        /// Few-bits candidate, the standard magic-search distribution.
        fn sparse(&mut self) -> u64 {
            self.next() & self.next() & self.next()
        }
    }

    fn enumerate(sq: u8, mask: u64, rook: bool) -> (Vec<u64>, Vec<u64>) {
        let n = 1usize << mask.count_ones();
        let mut occs = Vec::with_capacity(n);
        let mut atts = Vec::with_capacity(n);
        let mut i = 0usize;
        while i < n {
            let o = super::deposit(i as u64, mask);
            occs.push(o);
            atts.push(super::ray_march(sq, o, rook));
            i += 1;
        }
        (occs, atts)
    }

    fn find_magic(occs: &[u64], atts: &[u64], mask: u64, shift: u32, rng: &mut Rng) -> u64 {
        let n = occs.len();
        // Epoch stamps avoid re-clearing the witness arrays per candidate.
        let mut stamp = vec![0u32; n];
        let mut seen = vec![0u64; n];
        let mut cur: u32 = 1;
        let mut tries: u64 = 0;
        loop {
            tries += 1;
            if tries > 10_000_000 {
                panic!("black magic search exhausted: deterministic seed failed to converge");
            }
            let magic = rng.sparse();
            // Suitability heuristic: the masked product must spread into the
            // top byte, else the high index bits could never vary.
            if (mask.wrapping_mul(magic) & 0xff00_0000_0000_0000).count_ones() < 6 {
                continue;
            }
            let mut ok = true;
            let mut k = 0;
            while k < n {
                let j = occs[k].wrapping_mul(magic).wrapping_shr(shift) as usize;
                if stamp[j] == cur {
                    if seen[j] != atts[k] {
                        ok = false;
                        break;
                    }
                } else {
                    stamp[j] = cur;
                    seen[j] = atts[k];
                }
                k += 1;
            }
            if ok {
                return magic;
            }
            cur = cur.wrapping_add(1);
            if cur == 0 {
                // Epoch rollover (practically unreachable): clear and restart.
                stamp.iter_mut().for_each(|s| *s = 0);
                cur = 1;
            }
        }
    }

    fn build() -> Tables {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut t = Tables {
            rook_magic: [0; 64],
            bishop_magic: [0; 64],
            rook_shift: [0; 64],
            bishop_shift: [0; 64],
            rook_off: [0; 64],
            bishop_off: [0; 64],
            rook_tbl: vec![0u64; ROOK_TABLE_LEN].into_boxed_slice(),
            bishop_tbl: vec![0u64; BISHOP_TABLE_LEN].into_boxed_slice(),
        };
        let mut ro = 0usize;
        for s in 0..64 {
            let sq = s as u8;
            let mask = super::ROOK_MASK_TAB[s];
            let shift = 64 - mask.count_ones();
            let (occs, atts) = enumerate(sq, mask, true);
            let magic = find_magic(&occs, &atts, mask, shift, &mut rng);
            t.rook_magic[s] = magic;
            t.rook_shift[s] = shift;
            t.rook_off[s] = ro as u32;
            for k in 0..occs.len() {
                let j = ro + occs[k].wrapping_mul(magic).wrapping_shr(shift) as usize;
                // Perfect hash: every slot is written exactly once. (Rook
                // attack sets are never empty, so a zero slot means unwritten.)
                debug_assert_eq!(t.rook_tbl[j], 0);
                t.rook_tbl[j] = atts[k];
            }
            ro += occs.len();
        }
        debug_assert_eq!(ro, ROOK_TABLE_LEN);
        let mut bo = 0usize;
        for s in 0..64 {
            let sq = s as u8;
            let mask = super::BISHOP_MASK_TAB[s];
            let shift = 64 - mask.count_ones();
            let (occs, atts) = enumerate(sq, mask, false);
            let magic = find_magic(&occs, &atts, mask, shift, &mut rng);
            t.bishop_magic[s] = magic;
            t.bishop_shift[s] = shift;
            t.bishop_off[s] = bo as u32;
            for k in 0..occs.len() {
                let j = bo + occs[k].wrapping_mul(magic).wrapping_shr(shift) as usize;
                debug_assert_eq!(t.bishop_tbl[j], 0);
                t.bishop_tbl[j] = atts[k];
            }
            bo += occs.len();
        }
        debug_assert_eq!(bo, BISHOP_TABLE_LEN);
        t
    }

    #[inline]
    pub(super) fn rook_attacks(sq: u8, occ: u64) -> u64 {
        let t = &*TABLES;
        let s = sq as usize;
        let j = t.rook_off[s] as usize
            + (occ & super::ROOK_MASK_TAB[s])
                .wrapping_mul(t.rook_magic[s])
                .wrapping_shr(t.rook_shift[s]) as usize;
        t.rook_tbl[j]
    }

    #[inline]
    pub(super) fn bishop_attacks(sq: u8, occ: u64) -> u64 {
        let t = &*TABLES;
        let s = sq as usize;
        let j = t.bishop_off[s] as usize
            + (occ & super::BISHOP_MASK_TAB[s])
                .wrapping_mul(t.bishop_magic[s])
                .wrapping_shr(t.bishop_shift[s]) as usize;
        t.bishop_tbl[j]
    }
}

/// Arm 3 — x86_64 PEXT (`pext` feature + fast BMI2 at runtime).
///
/// Index = `_pext_u64(occ, mask)`: the hardware gathers the masked
/// occupancy bits, so no magic search is needed — tables are filled once
/// by enumerating subsets in deposit order. Masks/offsets reuse the shared
/// 1 KiB tables; only the attack tables below are PEXT-private.
#[cfg(all(target_arch = "x86_64", feature = "pext", not(feature = "min-mem")))]
mod pext {
    use std::sync::LazyLock;

    pub(super) const ROOK_TABLE_LEN: usize = 102_400;
    pub(super) const BISHOP_TABLE_LEN: usize = 5_248;
    const _: () = assert!(super::occ_total_len(true) == ROOK_TABLE_LEN);
    const _: () = assert!(super::occ_total_len(false) == BISHOP_TABLE_LEN);

    pub(super) struct Tables {
        pub(super) rook_off: [u32; 64],
        pub(super) bishop_off: [u32; 64],
        pub(super) rook_tbl: Box<[u64]>,
        pub(super) bishop_tbl: Box<[u64]>,
    }

    pub(super) static TABLES: LazyLock<Tables> = LazyLock::new(build);

    /// Software `pext` (bit gather in mask order — identical indexing to
    /// `_pext_u64`). Table-build enumeration only, never on the attack path.
    fn compress(occ: u64, mask: u64) -> usize {
        let mut out = 0u64;
        let mut bit = 0u32;
        let mut m = mask;
        while m != 0 {
            let l = 1u64 << m.trailing_zeros();
            if occ & l != 0 {
                out |= 1u64 << bit;
            }
            bit += 1;
            m &= m - 1;
        }
        out as usize
    }

    fn build() -> Tables {
        let mut t = Tables {
            rook_off: [0; 64],
            bishop_off: [0; 64],
            rook_tbl: vec![0u64; ROOK_TABLE_LEN].into_boxed_slice(),
            bishop_tbl: vec![0u64; BISHOP_TABLE_LEN].into_boxed_slice(),
        };
        let mut ro = 0usize;
        for s in 0..64 {
            let mask = super::ROOK_MASK_TAB[s];
            t.rook_off[s] = ro as u32;
            let n = 1usize << mask.count_ones();
            let mut i = 0usize;
            while i < n {
                let o = super::deposit(i as u64, mask);
                t.rook_tbl[ro + i] = super::ray_march(s as u8, o, true);
                debug_assert_eq!(compress(o, mask), i);
                i += 1;
            }
            ro += n;
        }
        debug_assert_eq!(ro, ROOK_TABLE_LEN);
        let mut bo = 0usize;
        for s in 0..64 {
            let mask = super::BISHOP_MASK_TAB[s];
            t.bishop_off[s] = bo as u32;
            let n = 1usize << mask.count_ones();
            let mut i = 0usize;
            while i < n {
                let o = super::deposit(i as u64, mask);
                t.bishop_tbl[bo + i] = super::ray_march(s as u8, o, false);
                debug_assert_eq!(compress(o, mask), i);
                i += 1;
            }
            bo += n;
        }
        debug_assert_eq!(bo, BISHOP_TABLE_LEN);
        t
    }

    #[target_feature(enable = "bmi2")]
    unsafe fn pext_index(occ: u64, mask: u64) -> usize {
        core::arch::x86_64::_pext_u64(occ, mask) as usize
    }

    #[inline]
    pub(super) fn rook_attacks(sq: u8, occ: u64) -> u64 {
        let t = &*TABLES;
        let s = sq as usize;
        // SAFETY: called only when `cached_engine()` reports `Engine::Pext`,
        // i.e. BMI2 present and not slow; `_pext_u64` needs BMI2, nothing else.
        let j = t.rook_off[s] as usize + unsafe { pext_index(occ, super::ROOK_MASK_TAB[s]) };
        t.rook_tbl[j]
    }

    #[inline]
    pub(super) fn bishop_attacks(sq: u8, occ: u64) -> u64 {
        let t = &*TABLES;
        let s = sq as usize;
        // SAFETY: as above.
        let j = t.bishop_off[s] as usize + unsafe { pext_index(occ, super::BISHOP_MASK_TAB[s]) };
        t.bishop_tbl[j]
    }
}

/// Cached [`engine()`] for the `pext`-gated dispatch below (one CPUID
/// sequence per process, not per call).
#[cfg(all(target_arch = "x86_64", feature = "pext", not(feature = "min-mem")))]
fn cached_engine() -> Engine {
    static KIND: std::sync::LazyLock<Engine> = std::sync::LazyLock::new(engine);
    *KIND
}

/// Rook attacks for `sq` (0 = a1 … 63 = h8) under occupancy `occ`.
///
/// Dispatches to the single resident engine: HQ on `aarch64`/`min-mem`,
/// Black magic on `wasm32` / non-`pext` `x86_64`, and on `pext` `x86_64`
/// to PEXT or its `select_engine` fallback. Zero heap allocation on
/// the attack path (tables initialise once, on first use).
pub fn rook_attacks(sq: u8, occ: u64) -> u64 {
    check_sq(sq);
    #[cfg(any(target_arch = "aarch64", feature = "min-mem"))]
    {
        hq::rook_attacks(sq, occ)
    }
    #[cfg(all(target_arch = "wasm32", not(feature = "min-mem")))]
    {
        black::rook_attacks(sq, occ)
    }
    #[cfg(all(
        target_arch = "x86_64",
        not(feature = "pext"),
        not(feature = "min-mem")
    ))]
    {
        black::rook_attacks(sq, occ)
    }
    #[cfg(all(target_arch = "x86_64", feature = "pext", not(feature = "min-mem")))]
    {
        match cached_engine() {
            Engine::Pext => pext::rook_attacks(sq, occ),
            Engine::Black => black::rook_attacks(sq, occ),
            // `Avx2Hq` runs on the HQ core (identical results); `Hq` is
            // unreachable on this config but keeps the match total.
            Engine::Avx2Hq | Engine::Hq => hq::rook_attacks(sq, occ),
        }
    }
}

/// Bishop attacks for `sq` (0 = a1 … 63 = h8) under occupancy `occ`.
///
/// Same resident-engine dispatch as [`rook_attacks`]; see its docs.
pub fn bishop_attacks(sq: u8, occ: u64) -> u64 {
    check_sq(sq);
    #[cfg(any(target_arch = "aarch64", feature = "min-mem"))]
    {
        hq::bishop_attacks(sq, occ)
    }
    #[cfg(all(target_arch = "wasm32", not(feature = "min-mem")))]
    {
        black::bishop_attacks(sq, occ)
    }
    #[cfg(all(
        target_arch = "x86_64",
        not(feature = "pext"),
        not(feature = "min-mem")
    ))]
    {
        black::bishop_attacks(sq, occ)
    }
    #[cfg(all(target_arch = "x86_64", feature = "pext", not(feature = "min-mem")))]
    {
        match cached_engine() {
            Engine::Pext => pext::bishop_attacks(sq, occ),
            Engine::Black => black::bishop_attacks(sq, occ),
            // `Avx2Hq` runs on the HQ core (identical results); `Hq` is
            // unreachable on this config but keeps the match total.
            Engine::Avx2Hq | Engine::Hq => hq::bishop_attacks(sq, occ),
        }
    }
}

/// Queen attacks: fused single-pass HQ on the HQ arm (one occupancy read,
/// `r`-derived quantities shared across all four lines), else rook ∪ bishop
/// union of the resident engine. Owns no tables. Results bit-identical.
#[inline]
pub fn queen_attacks(sq: u8, occ: u64) -> u64 {
    check_sq(sq);
    #[cfg(any(target_arch = "aarch64", feature = "min-mem"))]
    {
        hq::queen_attacks(sq, occ)
    }
    #[cfg(all(target_arch = "x86_64", feature = "pext", not(feature = "min-mem")))]
    {
        match cached_engine() {
            Engine::Pext => pext::rook_attacks(sq, occ) | pext::bishop_attacks(sq, occ),
            Engine::Black => black::rook_attacks(sq, occ) | black::bishop_attacks(sq, occ),
            Engine::Avx2Hq | Engine::Hq => hq::rook_attacks(sq, occ) | hq::bishop_attacks(sq, occ),
        }
    }
    #[cfg(not(any(target_arch = "aarch64", feature = "min-mem", feature = "pext")))]
    {
        rook_attacks(sq, occ) | bishop_attacks(sq, occ)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zen1_is_slow() {
        assert!(bmi2_is_slow(CpuVendor::Amd, 0x17, 0x01));
    }

    #[test]
    fn zen2_is_slow() {
        assert!(bmi2_is_slow(CpuVendor::Amd, 0x17, 0x31));
    }

    #[test]
    fn excavator_is_slow() {
        assert!(bmi2_is_slow(CpuVendor::Amd, 0x15, 0x70));
    }

    #[test]
    fn bulldozer_outside_excavator_range_is_fast() {
        assert!(!bmi2_is_slow(CpuVendor::Amd, 0x15, 0x02));
    }

    #[test]
    fn haswell_is_fast() {
        assert!(!bmi2_is_slow(CpuVendor::Intel, 0x06, 0x3c));
    }

    #[test]
    fn zen3_is_fast() {
        assert!(!bmi2_is_slow(CpuVendor::Amd, 0x19, 0x21));
    }

    #[test]
    fn unknown_vendor_is_fast() {
        assert!(!bmi2_is_slow(CpuVendor::Other, 0x17, 0x01));
    }

    #[test]
    fn vendor_words_decode() {
        // "AuthenticAMD": EBX, EDX, ECX.
        assert_eq!(
            vendor_from_cpuid_words(0x6875_7541, 0x6974_6e65, 0x444d_4163),
            CpuVendor::Amd
        );
        // "GenuineIntel": EBX, EDX, ECX.
        assert_eq!(
            vendor_from_cpuid_words(0x756e_6547, 0x4965_6e69, 0x6c65_746e),
            CpuVendor::Intel
        );
        assert_eq!(vendor_from_cpuid_words(0, 0, 0), CpuVendor::Other);
    }

    #[test]
    fn family_model_decode_haswell() {
        // Haswell i7: family 0x06, model 0x3c → EAX 0x0306c0.
        assert_eq!(decode_family_model(0x0003_06c0), (0x06, 0x3c));
    }

    #[test]
    fn family_model_decode_zen3() {
        // Zen3: base family 0x0f + ext 0x0a → 0x19; ext model 2, base 1 → 0x21.
        assert_eq!(decode_family_model(0x00a2_0f10), (0x19, 0x21));
    }

    #[test]
    fn select_fast_bmi2_is_pext() {
        assert_eq!(select_engine(true, false, false), Engine::Pext);
        assert_eq!(select_engine(true, false, true), Engine::Pext);
    }

    #[test]
    fn select_slow_bmi2_falls_back_by_avx2() {
        assert_eq!(select_engine(true, true, true), Engine::Avx2Hq);
        assert_eq!(select_engine(true, true, false), Engine::Black);
    }

    #[test]
    fn select_no_bmi2_falls_back_by_avx2() {
        assert_eq!(select_engine(false, false, true), Engine::Avx2Hq);
        assert_eq!(select_engine(false, false, false), Engine::Black);
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn aarch64_is_hq() {
        assert_eq!(engine(), Engine::Hq);
    }

    #[test]
    #[cfg(target_arch = "wasm32")]
    fn wasm32_is_black() {
        assert_eq!(engine(), Engine::Black);
    }

    #[test]
    #[cfg(all(target_arch = "x86_64", not(feature = "pext")))]
    fn non_pext_build_is_black_only() {
        assert_eq!(engine(), Engine::Black);
    }
    // ---- Phase 1 tables: correctness vs an independent oracle ----

    /// Independent oracle: no ray stepping at all — a target square is
    /// attacked iff it shares the slider's line with no blocker strictly
    /// between. Structurally disjoint from every marcher above on purpose:
    /// a test must not share code with the implementation it checks.
    fn between_rank(sq: u8, t: u8) -> u64 {
        let (lo, hi) = if (sq & 7) < (t & 7) {
            (sq & 7, t & 7)
        } else {
            (t & 7, sq & 7)
        };
        if hi - lo < 2 {
            return 0;
        }
        (1u64 << (hi - lo - 1)).wrapping_sub(1) << ((sq >> 3) * 8 + lo + 1)
    }

    fn between_file(sq: u8, t: u8) -> u64 {
        let (lo, hi) = if (sq >> 3) < (t >> 3) {
            (sq >> 3, t >> 3)
        } else {
            (t >> 3, sq >> 3)
        };
        if hi - lo < 2 {
            return 0;
        }
        let mut b = 0u64;
        let mut r = lo + 1;
        while r < hi {
            b |= 1u64 << (r * 8 + (sq & 7));
            r += 1;
        }
        b
    }

    fn on_diag(sq: u8, t: u8) -> bool {
        let df = (sq & 7) as i16 - (t & 7) as i16;
        let dr = (sq >> 3) as i16 - (t >> 3) as i16;
        df != 0 && df.abs() == dr.abs()
    }

    fn between_diag(sq: u8, t: u8) -> u64 {
        let f = (sq & 7) as i16;
        let r = (sq >> 3) as i16;
        let tf = (t & 7) as i16;
        let tr = (t >> 3) as i16;
        let (sf, sr) = ((tf - f).signum(), (tr - r).signum());
        let mut b = 0u64;
        let (mut cf, mut cr) = (f + sf, r + sr);
        while cf != tf && cr != tr {
            b |= 1u64 << (cr as u8 * 8 + cf as u8);
            cf += sf;
            cr += sr;
        }
        b
    }

    fn oracle(sq: u8, occ: u64, rook: bool) -> u64 {
        let mut a = 0u64;
        for t in 0..64u8 {
            if t == sq {
                continue;
            }
            let between = if rook {
                if (sq >> 3) == (t >> 3) {
                    Some(between_rank(sq, t))
                } else if (sq & 7) == (t & 7) {
                    Some(between_file(sq, t))
                } else {
                    None
                }
            } else if on_diag(sq, t) {
                Some(between_diag(sq, t))
            } else {
                None
            };
            if let Some(b) = between {
                if occ & b == 0 {
                    a |= 1u64 << t;
                }
            }
        }
        a
    }

    fn bits(sqs: &[u8]) -> u64 {
        let mut b = 0u64;
        for &s in sqs {
            b |= 1u64 << s;
        }
        b
    }

    #[test]
    fn rook_a1_empty_covers_rank_and_file() {
        // Rank 1 minus a1 (0xFE) plus the a-file minus a1.
        assert_eq!(rook_attacks(0, 0), 0x0101_0101_0101_01FE);
        assert_eq!(rook_attacks(0, 0), oracle(0, 0, true));
    }

    #[test]
    fn rook_e4_blocked_in_all_directions() {
        // e4 = 28; blockers e6 (44), e2 (12), b4 (25), g4 (30).
        let occ = bits(&[44, 12, 25, 30]);
        // N: e5, e6(capture) | S: e3, e2(capture) |
        // W: d4, c4, b4(capture) | E: f4, g4(capture).
        assert_eq!(
            rook_attacks(28, occ),
            bits(&[36, 44, 20, 12, 27, 26, 25, 29, 30])
        );
        assert_eq!(rook_attacks(28, occ), oracle(28, occ, true));
    }

    #[test]
    fn bishop_d4_empty_covers_both_diagonals() {
        // d4 = 27. NE e5 f6 g7 h8; NW c5 b6 a7; SE e3 f2 g1; SW c3 b2 a1.
        let want = bits(&[36, 45, 54, 63, 34, 41, 48, 20, 13, 6, 18, 9, 0]);
        assert_eq!(bishop_attacks(27, 0), want);
        assert_eq!(bishop_attacks(27, 0), oracle(27, 0, false));
    }

    #[test]
    fn bishop_d5_edge_blockers_stop_rays() {
        // d5 = 35; blockers b7 (49, NW ray), g2 (14, SE ray), a2 (8, SW tip).
        let occ = bits(&[49, 14, 8]);
        // NW: c6, b7(capture) — a8 cut off. NE: e6, f7, g8 (open).
        // SE: e4, f3, g2(capture) — h1 cut off. SW: c4, b3, a2(capture).
        let want = bits(&[42, 49, 44, 53, 62, 28, 21, 14, 26, 17, 8]);
        assert_eq!(bishop_attacks(35, occ), want);
        assert_eq!(bishop_attacks(35, occ), oracle(35, occ, false));
    }

    #[test]
    fn queen_is_rook_or_bishop() {
        for sq in [0u8, 27, 35, 63] {
            for occ in [0u64, u64::MAX, 0xAA55_AA55_AA55_AA55, 0x1234_5678_9ABC_DEF0] {
                assert_eq!(
                    queen_attacks(sq, occ),
                    rook_attacks(sq, occ) | bishop_attacks(sq, occ)
                );
            }
        }
    }

    #[test]
    fn sliders_match_naive_on_all_squares() {
        const PATTERNS: [u64; 14] = [
            0x0000_0000_0000_0000, // empty
            0xFFFF_FFFF_FFFF_FFFF, // full
            0xAA55_AA55_AA55_AA55, // checkerboard
            0x55AA_55AA_55AA_55AA, // inverse checkerboard
            0xFF00_FF00_FF00_FF00, // rank stripes
            0x00FF_00FF_00FF_00FF, // inverse rank stripes
            0x0F0F_F0F0_00FF_FF00,
            0x8000_0000_0000_0001, // corners only
            0x7E7E_7E7E_7E7E_7E7E, // interior files
            0x00FF_FFFF_FFFF_FF00, // interior ranks
            0x1234_5678_9ABC_DEF0,
            0xDEAD_BEEF_CAFE_F00D,
            0x9242_4952_9492_4294,
            0x1111_2222_3333_4444,
        ];
        for sq in 0..64u8 {
            for &occ in &PATTERNS {
                assert_eq!(
                    rook_attacks(sq, occ),
                    oracle(sq, occ, true),
                    "rook sq={sq} occ={occ:016x}"
                );
                assert_eq!(
                    bishop_attacks(sq, occ),
                    oracle(sq, occ, false),
                    "bishop sq={sq} occ={occ:016x}"
                );
            }
            // Every single-blocker occupancy too.
            for b in 0..64u8 {
                let occ = 1u64 << b;
                assert_eq!(
                    rook_attacks(sq, occ),
                    oracle(sq, occ, true),
                    "rook sq={sq} b={b}"
                );
                assert_eq!(
                    bishop_attacks(sq, occ),
                    oracle(sq, occ, false),
                    "bishop sq={sq} b={b}"
                );
            }
        }
    }

    #[test]
    fn table_size_note() {
        // Static bytes added per resident arm (attack tables only; code extra).
        #[cfg(any(
            target_arch = "aarch64",
            feature = "min-mem",
            all(target_arch = "x86_64", feature = "pext")
        ))]
        assert_eq!(
            std::mem::size_of_val(&super::hq::RANK)
                + std::mem::size_of_val(&super::hq::FILE)
                + std::mem::size_of_val(&super::hq::DIAG)
                + std::mem::size_of_val(&super::hq::ANTI),
            2048
        );
        #[cfg(all(
            not(feature = "min-mem"),
            any(target_arch = "wasm32", target_arch = "x86_64")
        ))]
        {
            assert_eq!(std::mem::size_of_val(&super::ROOK_MASK_TAB), 512);
            assert_eq!(std::mem::size_of_val(&super::BISHOP_MASK_TAB), 512);
            // Force Black init, then measure the live heap tables.
            let _ = super::rook_attacks(0, 0);
            let t = &*super::black::TABLES;
            assert_eq!(std::mem::size_of_val(&*t.rook_tbl), 102_400 * 8);
            assert_eq!(std::mem::size_of_val(&*t.bishop_tbl), 5_248 * 8);
            assert_eq!(super::black::ROOK_TABLE_LEN, 102_400);
            assert_eq!(super::black::BISHOP_TABLE_LEN, 5_248);
        }
        #[cfg(all(target_arch = "x86_64", feature = "pext", not(feature = "min-mem")))]
        {
            if super::cached_engine() == Engine::Pext {
                let _ = super::rook_attacks(0, 0);
                let t = &*super::pext::TABLES;
                assert_eq!(std::mem::size_of_val(&*t.rook_tbl), 102_400 * 8);
                assert_eq!(std::mem::size_of_val(&*t.bishop_tbl), 5_248 * 8);
            }
        }
    }
}
