//! `shahmat`: a movegen-correct chess library.
//!
//! The library owns the board representation ([`Board`]), the attack tables
//! behind one resident slider engine per platform ([`attacks`]), pseudo-legal
//! generation plus a full legality filter with make/unmake ([`movegen`]), FEN
//! parse/render ([`fen`]), exact node counting with an optional transposition
//! table ([`perft`]), PGN loading ([`pgn`]), and a playable core game
//! layer ([`api`]). The `nif` feature adds thin Elixir (Rustler) bindings.
//!
//! Hot paths (movegen, make/unmake, perft counting) use only stack storage —
//! no heap allocation per node. Input failures are plain-data error enums
//! ([`fen::FenError`], [`pgn::PgnError`], [`api::GameError`]) surfaced as
//! `Result`s.
//!
//! ## Example
//!
//! ```rust
//! use shahmat::fen;
//! use shahmat::movegen::{generate_legal, make, unmake, MoveList};
//!
//! let mut b = fen::parse(fen::STARTPOS).unwrap();
//! let mut list = MoveList::new();
//! generate_legal(&mut b, &mut list);
//! let undo = make(&mut b, list.moves[0]);
//! unmake(&mut b, undo, list.moves[0]);
//! assert_eq!(fen::render(&b), fen::STARTPOS);
//! ```
#![warn(missing_docs)]

#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "x86_64",
    target_arch = "wasm32"
)))]
compile_error!("shahmat supports only aarch64, x86_64, wasm32");

#[cfg(all(feature = "pext", not(target_arch = "x86_64")))]
compile_error!("pext feature is x86_64 only");

pub mod api;
pub mod attacks;
pub mod board;
pub mod fen;
pub mod movegen;
#[cfg(feature = "nif")]
pub mod nif;
pub mod perft;
pub mod pgn;

pub use board::{Board, Move, StateInfo};
