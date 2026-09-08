//! FIDE game-over layer (strict Laws of Chess): checkmate, stalemate, and
//! draws — fifty-move (claimable) / seventy-five-move (automatic), threefold
//! (claimable) / fivefold (automatic) repetition, and dead positions (bare
//! kings, minor vs bare king, same-colored-bishop endings). Repetition rides
//! a per-ply Zobrist history; adjudication costs nothing in movegen/perft
//! (game layer only) and switches off via [`Game::without_adjudication`].
//!
//! [`Game`] owns a [`Board`] plus the per-ply history
//! needed for [`Game::undo`], a header map, and the initial FEN. Load a FEN
//! or the startpos, then play through games with [`Game::push_san`] or
//! [`Game::push_uci`], query legal moves as SAN
//! ([`Game::moves_san`]/[`Game::moves_verbose`]), and export
//! ([`Game::fen`]/[`Game::to_pgn`]).
//!
//! ## Scope
//!
//! - FIDE game-over only: [`Game::is_game_over`] is checkmate, stalemate, or
//!   an automatic draw (seventy-five moves, fivefold repetition, dead
//!   position). Claimable draws (fifty moves, threefold) are reported as
//!   facts ([`Game::is_fifty_move_rule`], [`Game::is_threefold_repetition`])
//!   for the caller to claim — the lib never claims on a player's behalf.
//!   [`Game::without_adjudication`] restores bare mate/stalemate mode.
//! - No board editing: no piece placement, removal, or clearing, no loading
//!   moves into an arbitrary position. Games start from
//!   [`STARTPOS`] or a FEN via [`Game::from_fen`]/
//!   [`Game::load_fen`] only.
//! - Move numbering in [`Game::to_pgn`] always starts at 1 (fullmove is not
//!   stored by FEN render, which always emits `1`).

use crate::board::{board_hash, Board, Move, StateInfo};
use crate::fen::{self, STARTPOS};
use crate::movegen::{generate_legal, has_legal, is_in_check, make, unmake, MoveList};
use crate::pgn::{self, PgnError};
use std::collections::BTreeMap;
use std::fmt;

/// Game failure: bad FEN, unparseable/ambiguous SAN, bad square, or bad PGN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameError {
    /// FEN text did not parse.
    Fen(crate::fen::FenError),
    /// PGN text did not parse.
    Pgn(PgnError),
    /// SAN token is malformed or matches no legal move.
    BadSan(String),
    /// SAN token matches more than one legal move.
    AmbiguousSan(String),
    /// Square name is malformed (`from`/`to` in UCI, `get`, `square_color`).
    BadSquare(String),
    /// UCI move matches no legal move (or promotion piece missing/invalid).
    BadUci(String),
    /// PGN holds no games.
    BadPgn(String),
}

impl fmt::Display for GameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GameError::Fen(e) => write!(f, "bad FEN: {e}"),
            GameError::Pgn(e) => write!(f, "bad PGN: {e}"),
            GameError::BadSan(s) => write!(f, "bad SAN: {s}"),
            GameError::AmbiguousSan(s) => write!(f, "ambiguous SAN: {s}"),
            GameError::BadSquare(s) => write!(f, "bad square: {s}"),
            GameError::BadUci(s) => write!(f, "bad UCI move: {s}"),
            GameError::BadPgn(s) => write!(f, "bad PGN: {s}"),
        }
    }
}

impl std::error::Error for GameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GameError::Fen(e) => Some(e),
            GameError::Pgn(e) => Some(e),
            _ => None,
        }
    }
}

impl From<crate::fen::FenError> for GameError {
    fn from(e: crate::fen::FenError) -> GameError {
        GameError::Fen(e)
    }
}

impl From<PgnError> for GameError {
    fn from(e: PgnError) -> GameError {
        GameError::Pgn(e)
    }
}

/// One legal move with verbose fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerboseMove {
    /// Canonical SAN (with `+`/`#`).
    pub san: String,
    /// Origin square name (`"e2"`).
    pub from: String,
    /// Destination square name (`"e4"`).
    pub to: String,
    /// Moved piece type, lowercase (`'p'`/`'n'`/`'b'`/`'r'`/`'q'`/`'k'`).
    pub piece: char,
    /// Side moved, `'w'`/`'b'`.
    pub color: char,
    /// Captured piece type, lowercase; `None` when quiet.
    pub captured: Option<char>,
    /// Promotion piece type, lowercase; `None` when not a promotion.
    pub promotion: Option<char>,
}

/// A playable game: position plus undo history, headers, and initial FEN.
#[derive(Clone, Debug)]
pub struct Game {
    board: Board,
    history: Vec<(Move, StateInfo, String)>,
    /// Zobrist history: initial position hash plus one per ply, mirroring
    /// `history` (repetition counts over these; see [`Game::repetition_count`]).
    hashes: Vec<u64>,
    /// FIDE adjudication switch (draws only — mate/stalemate always apply).
    adjudicate: bool,
    headers: BTreeMap<String, String>,
    initial_fen: String,
}

/// File letter of square `sq` (0 = a1 … 63 = h8).
#[inline]
fn file_of(sq: u8) -> u8 {
    sq % 8
}

/// Rank number (0-based) of square `sq`.
#[inline]
fn rank_of(sq: u8) -> u8 {
    sq / 8
}

/// Square name (`"e4"`) of square `sq`.
fn square_name(sq: u8) -> String {
    let mut s = String::with_capacity(2);
    s.push((b'a' + file_of(sq)) as char);
    s.push((b'1' + rank_of(sq)) as char);
    s
}

/// Square index of a name like `"e4"`; `None` when malformed.
fn parse_square(s: &str) -> Option<u8> {
    let b = s.as_bytes();
    if b.len() != 2 {
        return None;
    }
    if !(b'a'..=b'h').contains(&b[0]) || !(b'1'..=b'8').contains(&b[1]) {
        return None;
    }
    Some((b[1] - b'1') * 8 + (b[0] - b'a'))
}

/// Piece index (0 pawn … 5 king) on `sq`; `None` when empty.
fn piece_on(board: &Board, sq: u8) -> Option<usize> {
    let bit = 1u64 << sq;
    (0..6).find(|&i| board.occupancies[i] & bit != 0)
}

/// Lowercase piece-type letter of a piece index.
fn piece_letter(piece: usize) -> char {
    // Total like perft's `move_text`: legal tokens index 0–5, but `Move.0`
    // is pub so a crafted token must render, never panic.
    *b"pnbrqk".get(piece).unwrap_or(&b'?') as char
}

/// True when `mv` captures on `board` (enemy-occupied dest, or a pawn
/// diagonal onto an empty square = en passant).
fn is_capture(board: &Board, mv: Move, piece: usize) -> bool {
    let white = board.state[0] & 1 == 0;
    let enemy = board.occupancies[if white { 7 } else { 6 }];
    let occ = board.occupancies[8];
    let (from, to) = (mv.from(), mv.to());
    (enemy >> to) & 1 == 1 || (piece == 0 && file_of(from) != file_of(to) && (occ >> to) & 1 == 0)
}

/// Legal moves of `board` (stack buffer, zero heap per call).
fn legal_moves(board: &Board) -> MoveList {
    let mut copy = *board;
    let mut list = MoveList::new();
    generate_legal(&mut copy, &mut list);
    list
}

/// Promotion code (1 N … 4 Q) of a lowercase promo letter.
fn promo_code(ch: char) -> Option<u8> {
    match ch {
        'n' => Some(1),
        'b' => Some(2),
        'r' => Some(3),
        'q' => Some(4),
        _ => None,
    }
}

impl Game {
    /// Startpos game with empty headers (FIDE adjudication on).
    pub fn new() -> Game {
        Self::from_fen(STARTPOS).expect("STARTPOS parses")
    }

    /// Bare mate/stalemate mode: draw adjudication off (perft-shaped purity;
    /// movegen/perft never adjudicate either way).
    pub fn without_adjudication() -> Game {
        let mut g = Game::new();
        g.adjudicate = false;
        g
    }

    /// FIDE draw adjudication on (`true` by default; `false` after
    /// [`Game::without_adjudication`] or [`Game::set_adjudication`]).
    pub fn adjudication(&self) -> bool {
        self.adjudicate
    }

    /// Switch FIDE draw adjudication on/off (mate/stalemate unaffected).
    pub fn set_adjudication(&mut self, on: bool) {
        self.adjudicate = on;
    }
    /// Game from a FEN string (also becomes the [`Game::reset`] target).
    pub fn from_fen(fen_str: &str) -> Result<Game, GameError> {
        let board = fen::parse(fen_str)?;
        let hashes = vec![board_hash(&board)];
        Ok(Game {
            board,
            history: Vec::new(),
            hashes,
            adjudicate: true,
            headers: BTreeMap::new(),
            initial_fen: fen_str.to_string(),
        })
    }

    /// Side to move: `'w'` or `'b'`.
    pub fn turn(&self) -> char {
        if self.board.state[0] & 1 == 0 {
            'w'
        } else {
            'b'
        }
    }

    /// Current position as FEN.
    pub fn fen(&self) -> String {
        fen::render(&self.board)
    }

    /// Board with rank 8 first: `rows[0]` is rank 8, `rows[7]` rank 1;
    /// within a row index 0 is file a. Pieces are FEN letters
    /// (uppercase = white), empties `None`.
    pub fn board_rank8_first(&self) -> [[Option<char>; 8]; 8] {
        let mut rows = [[None; 8]; 8];
        for sq in 0..64u8 {
            if let Some(p) = piece_on(&self.board, sq) {
                let mut ch = piece_letter(p);
                if self.board.occupancies[6] & (1u64 << sq) != 0 {
                    ch = ch.to_ascii_uppercase();
                }
                rows[7 - rank_of(sq) as usize][file_of(sq) as usize] = Some(ch);
            }
        }
        rows
    }

    /// Canonical SAN for a legal move in the current position (piece
    /// letter, file/rank disambiguation against other same-piece legal
    /// moves to the same square, `x` including en passant, `=promotion`,
    /// `+`/`#` by a make-test on a board copy).
    pub fn san_of_move(&self, mv: Move) -> String {
        let from = mv.from();
        let to = mv.to();
        let piece = piece_on(&self.board, from).unwrap_or(0);
        if piece == 5 && file_of(from) == 4 && (file_of(to) == 6 || file_of(to) == 2) {
            let castle = if file_of(to) == 6 { "O-O" } else { "O-O-O" };
            return format!("{castle}{}", self.suffix_of(mv));
        }
        let list = legal_moves(&self.board);
        let mut san = String::new();
        if piece != 0 {
            san.push(piece_letter(piece).to_ascii_uppercase());
            let mut same_file = false;
            let mut same_rank = false;
            for &other in list.as_slice() {
                if other.to() != to || other.from() == from {
                    continue;
                }
                if piece_on(&self.board, other.from()) != Some(piece) {
                    continue;
                }
                if file_of(other.from()) == file_of(from) {
                    same_file = true;
                }
                if rank_of(other.from()) == rank_of(from) {
                    same_rank = true;
                }
            }
            if same_file || same_rank {
                if !same_file {
                    san.push((b'a' + file_of(from)) as char);
                } else if !same_rank {
                    san.push((b'1' + rank_of(from)) as char);
                } else {
                    san.push((b'a' + file_of(from)) as char);
                    san.push((b'1' + rank_of(from)) as char);
                }
            }
        } else if file_of(from) != file_of(to) {
            san.push((b'a' + file_of(from)) as char);
        }
        if is_capture(&self.board, mv, piece) {
            san.push('x');
        }
        san.push_str(&square_name(to));
        if mv.promo() != 0 {
            san.push('=');
            san.push(piece_letter(mv.promo() as usize).to_ascii_uppercase());
        }
        san.push_str(&self.suffix_of(mv));
        san
    }

    /// `""`, `"+"`, or `"#"` for `mv` (legal-move-only make-test).
    fn suffix_of(&self, mv: Move) -> String {
        let mover_white = self.board.state[0] & 1 == 0;
        let mut copy = self.board;
        let _ = make(&mut copy, mv);
        if !is_in_check(&copy, !mover_white) {
            return String::new();
        }
        if has_legal(&mut copy, !mover_white) {
            "+".to_string()
        } else {
            "#".to_string()
        }
    }

    /// All legal moves as SAN, in movegen order.
    pub fn moves_san(&self) -> Vec<String> {
        legal_moves(&self.board)
            .as_slice()
            .iter()
            .map(|&mv| self.san_of_move(mv))
            .collect()
    }

    /// All legal moves with from/to/piece/capture/promotion fields.
    pub fn moves_verbose(&self) -> Vec<VerboseMove> {
        let white = self.board.state[0] & 1 == 0;
        legal_moves(&self.board)
            .as_slice()
            .iter()
            .map(|&mv| {
                let from = mv.from();
                let piece = piece_on(&self.board, from).unwrap_or(0);
                let captured = if is_capture(&self.board, mv, piece) {
                    let to = mv.to();
                    if piece == 0 && (self.board.occupancies[8] >> to) & 1 == 0 {
                        Some('p')
                    } else {
                        piece_on(&self.board, to).map(piece_letter)
                    }
                } else {
                    None
                };
                VerboseMove {
                    san: self.san_of_move(mv),
                    from: square_name(from),
                    to: square_name(mv.to()),
                    piece: piece_letter(piece),
                    color: if white { 'w' } else { 'b' },
                    captured,
                    promotion: if mv.promo() != 0 {
                        Some(piece_letter(mv.promo() as usize))
                    } else {
                        None
                    },
                }
            })
            .collect()
    }

    /// Play a SAN move; returns the canonical SAN (with `+`/`#`).
    pub fn push_san(&mut self, san: &str) -> Result<String, GameError> {
        let parsed =
            pgn::parse_san(san.trim()).ok_or_else(|| GameError::BadSan(san.to_string()))?;
        let list = legal_moves(&self.board);
        match pgn::resolve_san(&self.board, &parsed, list.as_slice()) {
            Ok(mv) => self.push_move(mv),
            Err(1..) => Err(GameError::AmbiguousSan(san.to_string())),
            Err(_) => Err(GameError::BadSan(san.to_string())),
        }
    }

    /// Play a UCI move (`from`/`to` square names, optional lowercase
    /// promotion piece); returns the canonical SAN.
    pub fn push_uci(
        &mut self,
        from: &str,
        to: &str,
        promo: Option<char>,
    ) -> Result<String, GameError> {
        let from_sq = parse_square(from).ok_or_else(|| GameError::BadSquare(from.to_string()))?;
        let to_sq = parse_square(to).ok_or_else(|| GameError::BadSquare(to.to_string()))?;
        let want = match promo {
            None => 0,
            Some(ch) => promo_code(ch.to_ascii_lowercase())
                .ok_or_else(|| GameError::BadUci(format!("{from}{to}{ch}")))?,
        };
        let list = legal_moves(&self.board);
        let mut found: Option<Move> = None;
        for &mv in list.as_slice() {
            if mv.from() != from_sq || mv.to() != to_sq {
                continue;
            }
            if mv.promo() != want {
                continue;
            }
            found = Some(mv);
            break;
        }
        match found {
            Some(mv) => self.push_move(mv),
            None => Err(GameError::BadUci(match promo {
                Some(ch) => format!("{from}{to}{ch}"),
                None => format!("{from}{to}"),
            })),
        }
    }

    /// Record a resolved legal move; shared tail of push_san/push_uci.
    fn push_move(&mut self, mv: Move) -> Result<String, GameError> {
        let san = self.san_of_move(mv);
        let undo = make(&mut self.board, mv);
        self.history.push((mv, undo, san.clone()));
        self.hashes.push(board_hash(&self.board));
        Ok(san)
    }

    /// Take back the last ply; returns its SAN, or `None` when empty.
    pub fn undo(&mut self) -> Option<String> {
        let (mv, undo, san) = self.history.pop()?;
        unmake(&mut self.board, undo, mv);
        self.hashes.pop();
        Some(san)
    }

    /// Played moves as SAN, oldest first.
    pub fn history_san(&self) -> Vec<String> {
        self.history.iter().map(|(_, _, s)| s.clone()).collect()
    }

    /// Side to move is in check.
    pub fn in_check(&self) -> bool {
        is_in_check(&self.board, self.turn() == 'w')
    }

    /// Side to move is checkmated.
    pub fn is_checkmate(&self) -> bool {
        if !self.in_check() {
            return false;
        }
        let mut copy = self.board;
        !has_legal(&mut copy, self.turn() == 'w')
    }

    /// Side to move is stalemated.
    pub fn is_stalemate(&self) -> bool {
        if self.in_check() {
            return false;
        }
        let mut copy = self.board;
        !has_legal(&mut copy, self.turn() == 'w')
    }

    /// Checkmate, stalemate, or an automatic FIDE draw (seventy-five moves,
    /// fivefold repetition, dead position). Claimable draws (fifty moves,
    /// threefold) are facts for the caller to claim, never automatic here.
    pub fn is_game_over(&self) -> bool {
        self.is_checkmate()
            || self.is_stalemate()
            || (self.adjudicate
                && (self.is_seventy_five_move_rule()
                    || self.is_fivefold_repetition()
                    || self.is_dead_position()))
    }

    /// Drawn game: stalemate or any automatic FIDE draw (same set as
    /// [`Game::is_game_over`] minus checkmate). Claimable draws excluded.
    pub fn is_draw(&self) -> bool {
        self.is_stalemate()
            || (self.adjudicate
                && (self.is_seventy_five_move_rule()
                    || self.is_fivefold_repetition()
                    || self.is_dead_position()))
    }

    /// Halfmove clock (plies since last pawn move or capture).
    fn halfmove(&self) -> u64 {
        (self.board.state[0] >> 12) & 0x3fff
    }

    /// Fifty-move rule claimable: 100+ halfmoves. A fact for the player to
    /// claim — never automatic in [`Game::is_game_over`].
    pub fn is_fifty_move_rule(&self) -> bool {
        self.adjudicate && self.halfmove() >= 100
    }

    /// Seventy-five-move rule: 150+ halfmoves, automatic draw.
    pub fn is_seventy_five_move_rule(&self) -> bool {
        self.adjudicate && self.halfmove() >= 150
    }

    /// Occurrences of the current position in this game (initial plus one
    /// per ply). Keys are full-recompute Zobrist hashes (pieces + side to
    /// move + rights + EP square): a dead EP square (no legal EP capture)
    /// still hashes distinctly, so rare repetitions through dead EP squares
    /// can undercount — errs toward fewer draws, never phantom ones.
    pub fn repetition_count(&self) -> u8 {
        let cur = board_hash(&self.board);
        self.hashes.iter().filter(|&&h| h == cur).count().min(255) as u8
    }

    /// Threefold repetition claimable: current position occurred 3+ times.
    /// A fact for the player to claim — never automatic here.
    pub fn is_threefold_repetition(&self) -> bool {
        self.adjudicate && self.repetition_count() >= 3
    }

    /// Fivefold repetition: current position occurred 5+ times, automatic.
    pub fn is_fivefold_repetition(&self) -> bool {
        self.adjudicate && self.repetition_count() >= 5
    }

    /// Dead position (FIDE 5.2.2 known cases): neither side can possibly
    /// mate — bare kings; minor (bishop/knight) vs bare king; bishop vs
    /// bishop on same-colored squares. Opposite-colored bishops can mate,
    /// so they are NOT dead. Wider dead-position search is out of scope.
    pub fn is_dead_position(&self) -> bool {
        if !self.adjudicate {
            return false;
        }
        let o = &self.board.occupancies;
        // Any pawn, rook, or queen mates trivially — only minor-piece shells.
        if o[0] | o[3] | o[4] != 0 {
            return false;
        }
        let minor = o[1] | o[2];
        let loco = minor.count_ones();
        if loco == 0 {
            return true; // bare kings
        }
        if loco == 1 {
            // Single bishop or knight vs bare king.
            return true;
        }
        if loco == 2 && o[1] == 0 {
            // Two bishops, one each side, on same-colored squares.
            let b = o[2];
            let w = o[6] & b;
            let bl = o[7] & b;
            if w.count_ones() == 1 && bl.count_ones() == 1 {
                let wsq = w.trailing_zeros();
                let bsq = bl.trailing_zeros();
                return (wsq + (wsq >> 3)) & 1 == (bsq + (bsq >> 3)) & 1;
            }
        }
        false
    }

    /// Back to the initial FEN with empty history (headers kept).
    /// The adjudication flag is preserved (unlike [`Game::from_fen`], which
    /// always starts it on): toggling draws off survives position loads.
    pub fn reset(&mut self) {
        if let Ok(board) = fen::parse(&self.initial_fen) {
            self.board = board;
        }
        self.history.clear();
        self.hashes = vec![board_hash(&self.board)];
    }

    /// Load a new FEN: clears history, keeps headers, and becomes the new
    /// [`Game::reset`] target. The adjudication flag is preserved.
    pub fn load_fen(&mut self, fen_str: &str) -> Result<(), GameError> {
        self.board = fen::parse(fen_str)?;
        self.history.clear();
        self.hashes = vec![board_hash(&self.board)];
        self.initial_fen = fen_str.to_string();
        Ok(())
    }

    /// Export tags plus numbered SAN with a trailing `*`.
    pub fn to_pgn(&self) -> String {
        let mut out = String::new();
        for (name, value) in &self.headers {
            out.push_str(&format!("[{name} \"{value}\"]\n"));
        }
        if !self.headers.is_empty() {
            out.push('\n');
        }
        let white_starts = self
            .initial_fen
            .split_whitespace()
            .nth(1)
            .map(|stm| stm != "b")
            .unwrap_or(true);
        let mut no = 1u32;
        let mut white_to_move = white_starts;
        let mut tokens: Vec<String> = Vec::with_capacity(self.history.len() + 1);
        for (_, _, san) in &self.history {
            if white_to_move {
                tokens.push(format!("{no}. {san}"));
            } else if tokens.is_empty() {
                tokens.push(format!("{no}... {san}"));
                no += 1;
                white_to_move = true;
                continue;
            } else {
                if let Some(last) = tokens.last_mut() {
                    last.push(' ');
                    last.push_str(san);
                }
            }
            if !white_to_move {
                no += 1;
            }
            white_to_move = !white_to_move;
        }
        out.push_str(&tokens.join(" "));
        if !tokens.is_empty() {
            out.push(' ');
        }
        out.push('*');
        out
    }

    /// Load the first game of a PGN string: tags become headers, the `FEN`
    /// tag (or startpos) becomes the position, and its moves replay.
    /// The adjudication flag is preserved.
    pub fn load_pgn(&mut self, src: &str) -> Result<(), GameError> {
        let games = pgn::load_pgn(src)?;
        let game = games
            .into_iter()
            .next()
            .ok_or_else(|| GameError::BadPgn("no games".to_string()))?;
        let start = game
            .startfen
            .clone()
            .unwrap_or_else(|| STARTPOS.to_string());
        self.board = fen::parse(&start)?;
        self.initial_fen = start;
        self.history.clear();
        self.hashes = vec![board_hash(&self.board)];
        self.headers.clear();
        for (name, value) in &game.tags {
            self.headers.insert(name.clone(), value.clone());
        }
        for &mv in &game.moves {
            self.push_move(mv)?;
        }
        Ok(())
    }

    /// Piece on a square as a FEN letter (uppercase = white);
    /// `None` when empty or the square name is bad.
    pub fn get(&self, square: &str) -> Option<char> {
        let sq = parse_square(square)?;
        let piece = piece_on(&self.board, sq)?;
        let mut ch = piece_letter(piece);
        if self.board.occupancies[6] & (1u64 << sq) != 0 {
            ch = ch.to_ascii_uppercase();
        }
        Some(ch)
    }

    /// `"light"`/`"dark"` for a square (`a1` is dark); `None` when bad.
    pub fn square_color(&self, square: &str) -> Option<&'static str> {
        let sq = parse_square(square)?;
        if (file_of(sq) + rank_of(sq)).is_multiple_of(2) {
            Some("dark")
        } else {
            Some("light")
        }
    }

    /// Header value (`None` when absent).
    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// Set a header (emitted by [`Game::to_pgn`], restored by [`Game::load_pgn`]).
    /// Tag values cannot hold raw newlines (PGN tags are single-line), so
    /// they become spaces here to keep the `to_pgn`/`load_pgn` round-trip.
    /// Names stay caller-responsible: bad names fail loudly in `load_pgn`.
    pub fn set_header(&mut self, name: &str, value: &str) {
        self.headers
            .insert(name.to_string(), value.replace(['\n', '\r'], " "));
    }
}

impl Default for Game {
    fn default() -> Game {
        Game::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(moves: &[&str]) -> Game {
        let mut g = Game::new();
        for m in moves {
            g.push_san(m).unwrap();
        }
        g
    }

    #[test]
    fn header_newline_sanitizes_and_round_trips() {
        let mut g = Game::new();
        g.set_header("Note", "a\nb\rc");
        assert_eq!(g.get_header("Note"), Some("a b c"));
        let text = g.to_pgn();
        let mut back = Game::new();
        back.load_pgn(&text).expect("own export loads");
        assert_eq!(back.get_header("Note"), Some("a b c"));
    }

    #[test]
    fn scholars_mate_exact_sans() {
        let mut g = Game::new();
        let want = ["e4", "e5", "Qh5", "Nc6", "Bc4", "Nf6", "Qxf7#"];
        for (i, m) in want.iter().enumerate() {
            assert_eq!(g.push_san(m).unwrap(), *m, "ply {i}");
        }
        assert!(g.in_check());
        assert!(g.is_checkmate());
        assert!(g.is_game_over());
        assert!(!g.is_stalemate());
        assert_eq!(g.history_san(), want);
    }

    #[test]
    fn fools_mate_exact_sans() {
        let mut g = Game::new();
        let want = ["f3", "e5", "g4", "Qh4#"];
        for m in want {
            assert_eq!(g.push_san(m).unwrap(), m);
        }
        assert!(g.is_checkmate());
        assert!(g.is_game_over());
    }

    #[test]
    fn castling_game_both_sides() {
        let mut g = play(&[
            "e4", "e5", "Nf3", "Nc6", "Bc4", "Bc5", "O-O", "Nf6", "d3", "O-O",
        ]);
        assert_eq!(g.history_san()[6], "O-O");
        assert_eq!(g.history_san()[9], "O-O");
        assert_eq!(g.get("g1"), Some('K'));
        assert_eq!(g.get("e1"), None);
        assert_eq!(g.get("g8"), Some('k'));
        assert_eq!(g.get("f1"), Some('R'));
        assert_eq!(g.get("f8"), Some('r'));
        assert!(!g.is_game_over());
        g.undo();
        assert_eq!(g.get("e8"), Some('k'));
    }

    #[test]
    fn promotion_san_and_check() {
        let mut g = Game::from_fen("7k/5P2/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(g.push_san("f8=Q").unwrap(), "f8=Q+");
        assert!(g.in_check());
        assert!(!g.is_checkmate());
        g.undo();
        assert_eq!(g.push_uci("f7", "f8", Some('n')).unwrap(), "f8=N");
        assert!(!g.in_check());
    }

    #[test]
    fn knight_file_disambiguation() {
        let g = Game::from_fen("7k/8/8/8/8/2N1N3/8/4K3 w - - 0 1").unwrap();
        let sans = g.moves_san();
        assert!(sans.contains(&"Ncd5".to_string()));
        assert!(sans.contains(&"Ned5".to_string()));
        let mut g = g;
        assert_eq!(g.push_san("Ncd5").unwrap(), "Ncd5");
        assert_eq!(g.get("d5"), Some('N'));
    }

    #[test]
    fn knight_rank_disambiguation() {
        let g = Game::from_fen("7k/8/8/8/8/3N4/8/3NK3 w - - 0 1").unwrap();
        let sans = g.moves_san();
        assert!(sans.contains(&"N1b2".to_string()));
        assert!(sans.contains(&"N3b2".to_string()));
        let mut g = g;
        assert_eq!(g.push_san("N3b2").unwrap(), "N3b2");
    }

    #[test]
    fn rook_rank_disambiguation() {
        let g = Game::from_fen("R7/7k/8/8/8/8/8/R3K3 w - - 0 1").unwrap();
        let sans = g.moves_san();
        assert!(sans.contains(&"R1a5".to_string()));
        assert!(sans.contains(&"R8a5".to_string()));
        let mut g = g;
        assert_eq!(g.push_san("R1a5").unwrap(), "R1a5");
    }

    #[test]
    fn undo_restores_fen() {
        let mut g = Game::new();
        let start = g.fen();
        g.push_san("e4").unwrap();
        let after_e4 = g.fen();
        g.push_san("e5").unwrap();
        g.undo();
        assert_eq!(g.fen(), after_e4);
        g.undo();
        assert_eq!(g.fen(), start);
        assert!(g.undo().is_none());
        assert_eq!(g.history_san(), Vec::<String>::new());
    }

    #[test]
    fn from_fen_error_paths() {
        assert!(Game::from_fen("not a fen").is_err());
        assert!(Game::from_fen("").is_err());
        assert!(
            Game::from_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPXPPPP/RNBQKBNR w KQkq - 0 1").is_err()
        );
        assert!(Game::from_fen(STARTPOS).is_ok());
    }

    #[test]
    fn push_errors() {
        let mut g = Game::new();
        assert!(matches!(g.push_san("Qh5"), Err(GameError::BadSan(_))));
        assert!(matches!(g.push_san("junk"), Err(GameError::BadSan(_))));
        assert!(matches!(
            g.push_uci("e2", "e9", None),
            Err(GameError::BadSquare(_))
        ));
        assert_eq!(
            g.push_uci("e2", "e5", None),
            Err(GameError::BadUci("e2e5".to_string()))
        );
        assert_eq!(g.push_uci("e2", "e4", None).unwrap(), "e4");
    }

    #[test]
    fn en_passant_san() {
        let mut g =
            Game::from_fen("rnbqkb1r/ppp1pppp/5n2/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3")
                .unwrap();
        assert!(g.moves_san().contains(&"exd6".to_string()));
        assert_eq!(g.push_san("exd6").unwrap(), "exd6");
        assert_eq!(g.get("d6"), Some('P'));
        assert_eq!(g.get("d5"), None);
    }

    #[test]
    fn to_pgn_round_trip_via_load_pgn() {
        let mut g = play(&["e4", "e5", "Qh5", "Nc6", "Bc4", "Nf6", "Qxf7#"]);
        g.set_header("White", "Alice");
        g.set_header("Black", "Bob");
        let pgn = g.to_pgn();
        assert!(pgn.contains("[White \"Alice\"]"));
        assert!(pgn.ends_with('*'));
        let mut h = Game::new();
        h.load_pgn(&pgn).unwrap();
        assert_eq!(h.history_san(), g.history_san());
        assert_eq!(h.fen(), g.fen());
        assert_eq!(h.get_header("White"), Some("Alice"));
    }

    #[test]
    fn reset_and_load_fen() {
        let mut g = play(&["e4", "e5"]);
        g.reset();
        assert_eq!(g.fen(), STARTPOS);
        assert!(g.history_san().is_empty());
        g.load_fen("7k/5P2/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(g.turn(), 'w');
        g.reset();
        assert_eq!(g.get("f7"), Some('P'));
        assert!(g.load_fen("bogus").is_err());
    }

    #[test]
    fn get_and_square_color_spots() {
        let g = Game::new();
        assert_eq!(g.get("e1"), Some('K'));
        assert_eq!(g.get("e2"), Some('P'));
        assert_eq!(g.get("e7"), Some('p'));
        assert_eq!(g.get("e4"), None);
        assert_eq!(g.get("i9"), None);
        assert_eq!(g.square_color("a1"), Some("dark"));
        assert_eq!(g.square_color("h1"), Some("light"));
        assert_eq!(g.square_color("e4"), Some("light"));
        assert_eq!(g.square_color("a8"), Some("light"));
        assert_eq!(g.square_color("z9"), None);
        let rows = g.board_rank8_first();
        assert_eq!(rows[0][0], Some('r'));
        assert_eq!(rows[7][4], Some('K'));
        assert_eq!(rows[4][4], None);
    }

    #[test]
    fn verbose_and_turn_and_board() {
        let g = Game::new();
        assert_eq!(g.turn(), 'w');
        assert_eq!(g.moves_verbose().len(), 20);
        let e4 = g
            .moves_verbose()
            .into_iter()
            .find(|m| m.san == "e4")
            .unwrap();
        assert_eq!(e4.from, "e2");
        assert_eq!(e4.to, "e4");
        assert_eq!(e4.piece, 'p');
        assert_eq!(e4.color, 'w');
    }
}
