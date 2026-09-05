#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "x86_64",
    target_arch = "wasm32"
)))]
compile_error!("shahmat supports only aarch64, x86_64, wasm32");

#[cfg(all(feature = "pext", not(target_arch = "x86_64")))]
compile_error!("pext feature is x86_64 only");

pub mod attacks;
pub mod board;
pub mod fen;
pub mod movegen;
#[cfg(feature = "nif")]
pub mod nif;
pub mod perft;
pub mod pgn;

pub use board::{Board, Move, StateInfo};
