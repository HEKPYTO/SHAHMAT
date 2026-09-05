use crate::board::Board;

pub fn perft_full(_board: &Board, _depth: u32) -> u64 {
    0
}

pub fn perft_bulk(_board: &Board, depth: u32) -> u64 {
    perft_full(_board, depth)
}
