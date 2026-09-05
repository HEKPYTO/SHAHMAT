use crate::board::{Board, Move, MOVELIST_CAP};

pub fn generate_stub(_board: &Board, buf: &mut [Move; MOVELIST_CAP]) -> usize {
    let _ = buf;
    0
}
