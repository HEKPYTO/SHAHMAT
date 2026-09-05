#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Board {
    pub occupancies: [u64; 9],
    pub state: [u64; 2],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StateInfo {
    pub data: [u64; 8],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Move(pub u16);

pub const MOVELIST_CAP: usize = 256;
