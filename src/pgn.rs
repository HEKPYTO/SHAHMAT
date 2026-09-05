//! PGN loader: games, tags, and SAN movetext resolved to [`Move`]s.
//!
//! [`load_pgn`] splits a PGN file into games, parses each game's tags, and
//! resolves its movetext SAN tokens into [`Move`]s against the legal-move
//! list ([`generate_legal`], read-only — movegen is frozen).
//!
//! Supported input:
//!
//! - Tags: `[Name "value"]` lines (name = ASCII alphanumerics + `_`).
//! - Movetext: SAN tokens with piece letter + disambiguation + `x` +
//!   destination + `=promotion` + trailing `+`/`#`, castles `O-O`/`O-O-O`
//!   (zeros `0-0`/`0-0-0` accepted), move numbers (`1.`, `1...`, `1.e4`),
//!   `!`/`?` suffixes stripped, `{...}` comments, `;` to end-of-line
//!   comments, `$n` NAGs, and results `1-0`/`0-1`/`1/2-1/2`/`*` skipped.
//! - Start position: the `FEN` tag when present (default [`STARTPOS`]);
//!   `startfen` records the `FEN` tag value, `None` for the default.
//!
//! SAN contract: each SAN is matched against the legal moves for the
//! position — moved piece, destination, promotion piece, capture flag
//! (`x` must agree with the board), disambiguation (file/rank/both), and
//! castle geometry (king two squares from e-file). Zero moves match →
//! [`PgnError::BadSan`]; more than one → [`PgnError::AmbiguousSan`].
//!
//! Allocation: tags, the per-game movetext buffer, the `&str` token list,
//! and the output move `Vec`s allocate; the resolve loop itself works on
//! `&str` slices with no per-token `String` churn (error paths only).

use crate::board::{Board, Move};
use crate::fen::{parse, STARTPOS};
use crate::movegen::{generate_legal, make, MoveList};
use core::fmt;

/// One parsed PGN game: tag pairs in file order, resolved moves, and the
/// start position (`Some(FEN)` when a `FEN` tag was present, else `None`
/// for the default [`STARTPOS`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PgnGame {
    /// `(name, value)` tag pairs in file order.
    pub tags: Vec<(String, String)>,
    /// Movetext SAN tokens resolved to legal moves, in ply order.
    pub moves: Vec<Move>,
    /// The `FEN` tag value, or `None` when the game starts at [`STARTPOS`].
    pub startfen: Option<String>,
}

/// PGN load failure. `game` is 1-based; movetext errors also carry the
/// 1-based `ply` and the offending SAN `token`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PgnError {
    /// A `[Tag "value"]` line is malformed (not closed, nameless, or the
    /// value is not double-quoted).
    BadTag { game: usize, detail: String },
    /// The `FEN` tag does not parse (`detail` is the [`FenError`] text).
    BadFen {
        game: usize,
        fen: String,
        detail: String,
    },
    /// A `{...}` comment is never closed.
    UnterminatedComment { game: usize },
    /// A SAN token is malformed or matches no legal move.
    BadSan {
        game: usize,
        ply: usize,
        token: String,
    },
    /// A SAN token matches more than one legal move.
    AmbiguousSan {
        game: usize,
        ply: usize,
        token: String,
        candidates: usize,
    },
}

impl fmt::Display for PgnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PgnError::BadTag { game, detail } => {
                write!(f, "game {game}: bad tag: {detail}")
            }
            PgnError::BadFen { game, fen, detail } => {
                write!(f, "game {game}: bad FEN {fen:?}: {detail}")
            }
            PgnError::UnterminatedComment { game } => {
                write!(f, "game {game}: unterminated '{{...}}' comment")
            }
            PgnError::BadSan { game, ply, token } => {
                write!(f, "game {game} ply {ply}: bad SAN {token:?}")
            }
            PgnError::AmbiguousSan {
                game,
                ply,
                token,
                candidates,
            } => {
                write!(
                    f,
                    "game {game} ply {ply}: ambiguous SAN {token:?} ({candidates} candidates)"
                )
            }
        }
    }
}

impl std::error::Error for PgnError {}

/// Load every game in a PGN file; see the module section above for the
/// accepted syntax. An empty (or whitespace/comment-only) file yields an
/// empty `Vec`.
pub fn load_pgn(src: &str) -> Result<Vec<PgnGame>, PgnError> {
    let mut games: Vec<PgnGame> = Vec::new();
    for (gi, raw) in split_games(src)?.into_iter().enumerate() {
        let game = gi + 1;
        let mut fen = STARTPOS;
        for (name, value) in raw.tags.iter() {
            if name == "FEN" {
                fen = value.as_str();
            }
        }
        let startfen = (fen != STARTPOS).then(|| fen.to_string());
        let mut board = parse(fen).map_err(|e| PgnError::BadFen {
            game,
            fen: fen.to_string(),
            detail: e.to_string(),
        })?;
        let sans = tokenize(game, &raw.text)?;
        let mut moves: Vec<Move> = Vec::with_capacity(sans.len());
        let mut list = MoveList::new();
        for (i, tok) in sans.iter().enumerate() {
            let ply = i + 1;
            let san = parse_san(tok).ok_or_else(|| PgnError::BadSan {
                game,
                ply,
                token: tok.to_string(),
            })?;
            list.len = 0;
            generate_legal(&mut board, &mut list);
            match resolve_san(&board, &san, list.as_slice()) {
                Ok(mv) => {
                    let _ = make(&mut board, mv);
                    moves.push(mv);
                }
                Err(0) => {
                    return Err(PgnError::BadSan {
                        game,
                        ply,
                        token: tok.to_string(),
                    });
                }
                Err(candidates) => {
                    return Err(PgnError::AmbiguousSan {
                        game,
                        ply,
                        token: tok.to_string(),
                        candidates,
                    });
                }
            }
        }
        games.push(PgnGame {
            tags: raw.tags,
            moves,
            startfen,
        });
    }
    Ok(games)
}

/// One game's raw tags plus its movetext buffer (comments already cut at
/// `;` line level; `{...}` handled in [`tokenize`]).
struct RawGame {
    tags: Vec<(String, String)>,
    text: String,
}

/// Split the file into games: tag lines (`[...]`) open/continue the tag
/// section, any other non-blank line is movetext. A tag line arriving
/// after movetext starts the next game.
fn split_games(src: &str) -> Result<Vec<RawGame>, PgnError> {
    let mut games: Vec<RawGame> = Vec::new();
    let mut tags: Vec<(String, String)> = Vec::new();
    let mut text = String::new();
    let mut has_tags = false;
    let mut has_text = false;
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with('[') {
            if has_text {
                games.push(RawGame {
                    tags: core::mem::take(&mut tags),
                    text: core::mem::take(&mut text),
                });
                has_text = false;
            }
            tags.push(parse_tag(games.len() + 1, t)?);
            has_tags = true;
        } else {
            let code = match line.find(';') {
                Some(i) => &line[..i],
                None => line,
            };
            let code = code.trim();
            if code.is_empty() {
                continue;
            }
            if has_text {
                text.push(' ');
            }
            text.push_str(code);
            has_text = true;
        }
    }
    if has_tags || has_text {
        games.push(RawGame { tags, text });
    }
    Ok(games)
}

/// Parse one `[Name "value"]` line (`line` already trimmed).
fn parse_tag(game: usize, line: &str) -> Result<(String, String), PgnError> {
    let err = |detail: &str| PgnError::BadTag {
        game,
        detail: format!("{detail}: {line:?}"),
    };
    if !line.ends_with(']') {
        return Err(err("tag line not closed with ']'"));
    }
    let inner = line[1..line.len() - 1].trim();
    let sp = inner
        .find(|c: char| c.is_whitespace())
        .ok_or_else(|| err("tag needs a name and a quoted value"))?;
    let name = inner[..sp].trim();
    let value = inner[sp..].trim();
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(err("bad tag name"));
    }
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return Err(err("tag value must be double-quoted"));
    }
    Ok((name.to_string(), value[1..value.len() - 1].to_string()))
}

/// Cut `{...}` comments (spans borrow `text`), then split into SAN
/// candidates: NAGs (`$n`), results, move numbers, and bare `...`
/// separators are skipped; `(`/`)` (variations) are rejected as bad SAN
/// at their 1-based ply.
fn tokenize(game: usize, text: &str) -> Result<Vec<&str>, PgnError> {
    let mut spans: Vec<&str> = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if start < i {
                spans.push(&text[start..i]);
            }
            match text[i..].find('}') {
                Some(off) => {
                    i += off + 1;
                    start = i;
                }
                None => return Err(PgnError::UnterminatedComment { game }),
            }
        } else {
            i += 1;
        }
    }
    if start < text.len() {
        spans.push(&text[start..]);
    }
    let mut sans: Vec<&str> = Vec::new();
    for w in spans.iter().flat_map(|s| s.split_whitespace()) {
        if w.starts_with('$') || w == "1-0" || w == "0-1" || w == "1/2-1/2" || w == "*" {
            continue;
        }
        if w.contains('(') || w.contains(')') {
            return Err(PgnError::BadSan {
                game,
                ply: sans.len() + 1,
                token: w.to_string(),
            });
        }
        let w = strip_move_number(w);
        if w.is_empty() || w.bytes().all(|b| b == b'.') {
            continue;
        }
        sans.push(w);
    }
    Ok(sans)
}

/// Strip a leading `N.` / `N...` move-number prefix (`1.e4` → `e4`,
/// `12...` → ``); anything else passes through untouched.
fn strip_move_number(w: &str) -> &str {
    let b = w.as_bytes();
    let mut i = 0usize;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i < b.len() && b[i] == b'.' {
        let mut j = i;
        while j < b.len() && b[j] == b'.' {
            j += 1;
        }
        return &w[j..];
    }
    w
}

/// A SAN token parsed against `&str` slices only (no allocation).
struct ParsedSan {
    /// 0 = normal move, 1 = kingside castle, 2 = queenside castle.
    castle: u8,
    /// Moved piece index (0 pawn … 5 king).
    piece: usize,
    /// Destination square (0 = a1 … 63 = h8).
    dest: u8,
    /// Promotion piece (0 = none, 1 = N, 2 = B, 3 = R, 4 = Q).
    promo: u8,
    /// A capture (`x`) was written.
    capture: bool,
    /// Disambiguation file/rank (file-first order, e.g. `Nb1`).
    dfile: Option<u8>,
    drank: Option<u8>,
}

/// Parse a SAN token; `None` when malformed. Trailing `+`/`#`/`!`/`?`
/// are ignored for matching (check/mate needs no legal-move filter).
fn parse_san(tok: &str) -> Option<ParsedSan> {
    let t = tok.trim_end_matches(['!', '?', '+', '#']);
    if t.is_empty() {
        return None;
    }
    if t == "O-O" || t == "0-0" || t == "o-o" {
        return Some(ParsedSan {
            castle: 1,
            piece: 5,
            dest: 0,
            promo: 0,
            capture: false,
            dfile: None,
            drank: None,
        });
    }
    if t == "O-O-O" || t == "0-0-0" || t == "o-o-o" {
        return Some(ParsedSan {
            castle: 2,
            piece: 5,
            dest: 0,
            promo: 0,
            capture: false,
            dfile: None,
            drank: None,
        });
    }
    let (s, promo) = match t.find('=') {
        Some(eq) => {
            let after = &t[eq + 1..];
            if after.len() != 1 {
                return None;
            }
            let p = match after.as_bytes()[0] {
                b'N' => 1,
                b'B' => 2,
                b'R' => 3,
                b'Q' => 4,
                _ => return None,
            };
            (&t[..eq], p)
        }
        None => (t, 0),
    };
    let sb = s.as_bytes();
    if sb.is_empty() {
        return None;
    }
    let (piece, rest) = match sb[0] {
        b'K' => (5, &s[1..]),
        b'Q' => (4, &s[1..]),
        b'R' => (3, &s[1..]),
        b'B' => (2, &s[1..]),
        b'N' => (1, &s[1..]),
        _ => (0, s),
    };
    if rest.len() < 2 || !rest.is_char_boundary(rest.len() - 2) {
        return None;
    }
    let (mid, dst) = rest.split_at(rest.len() - 2);
    let db = dst.as_bytes();
    if !(b'a'..=b'h').contains(&db[0]) || !(b'1'..=b'8').contains(&db[1]) {
        return None;
    }
    let dest = (db[1] - b'1') * 8 + (db[0] - b'a');
    let capture = mid.contains('x');
    let core: &str = if capture {
        if !mid.ends_with('x') || mid.matches('x').count() != 1 {
            return None;
        }
        &mid[..mid.len() - 1]
    } else {
        mid
    };
    let (dfile, drank) = match core.len() {
        0 => (None, None),
        1 => {
            let c = core.as_bytes()[0];
            if (b'a'..=b'h').contains(&c) {
                (Some(c - b'a'), None)
            } else if (b'1'..=b'8').contains(&c) {
                (None, Some(c - b'1'))
            } else {
                return None;
            }
        }
        2 => {
            let cb = core.as_bytes();
            if !(b'a'..=b'h').contains(&cb[0]) || !(b'1'..=b'8').contains(&cb[1]) {
                return None;
            }
            (Some(cb[0] - b'a'), Some(cb[1] - b'1'))
        }
        _ => return None,
    };
    Some(ParsedSan {
        castle: 0,
        piece,
        dest,
        promo,
        capture,
        dfile,
        drank,
    })
}

/// Match a parsed SAN against the legal-move list. `Ok` on exactly one
/// hit; `Err(n)` counts the hits (`0` → bad SAN, `>1` → ambiguous).
fn resolve_san(board: &Board, san: &ParsedSan, list: &[Move]) -> Result<Move, usize> {
    let white = board.state[0] & 1 == 0;
    let enemy = board.occupancies[if white { 7 } else { 6 }];
    let occ = board.occupancies[8];
    let mut best: Option<Move> = None;
    let mut count = 0usize;
    for &mv in list {
        let from = mv.from();
        let to = mv.to();
        let bit = 1u64 << from;
        let mut piece = 8usize;
        for i in 0..6 {
            if board.occupancies[i] & bit != 0 {
                piece = i;
                break;
            }
        }
        if san.castle != 0 {
            if piece != 5 || from % 8 != 4 || mv.promo() != 0 {
                continue;
            }
            let home = if white { 0 } else { 7 };
            if from / 8 != home || to / 8 != home {
                continue;
            }
            let want = if san.castle == 1 { 6 } else { 2 };
            if to % 8 != want {
                continue;
            }
        } else {
            if piece != san.piece || to != san.dest || mv.promo() != san.promo {
                continue;
            }
            let takes = (enemy >> to) & 1 == 1
                || (piece == 0 && from % 8 != to % 8 && (occ >> to) & 1 == 0);
            if takes != san.capture {
                continue;
            }
            if let Some(f) = san.dfile {
                if from % 8 != f {
                    continue;
                }
            }
            if let Some(r) = san.drank {
                if from / 8 != r {
                    continue;
                }
            }
        }
        count += 1;
        best = Some(mv);
    }
    match (best, count) {
        (Some(mv), 1) => Ok(mv),
        _ => Err(count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fen::render;

    #[test]
    fn pgn_scholars_mate() {
        let src = "[Event \"Scholar\"]\n[Site \"?\"]\n\n\
            1. e4! e5 { King's pawn } 2. Qh5?! Nc6 $1 3. Bc4 Nf6 ; develop\n\
            4. Qxf7# 1-0\n";
        let games = load_pgn(src).expect("scholar's mate loads");
        assert_eq!(games.len(), 1);
        assert_eq!(
            games[0].tags[0],
            ("Event".to_string(), "Scholar".to_string())
        );
        assert_eq!(games[0].moves.len(), 7);
        assert_eq!(games[0].startfen, None);
        let mut b = parse(STARTPOS).expect("startpos parses");
        for &mv in games[0].moves.iter() {
            let _ = crate::movegen::make(&mut b, mv);
        }
        assert_eq!(
            render(&b),
            "r1bqkb1r/pppp1Qpp/2n2n2/4p3/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 1"
        );
    }

    #[test]
    fn pgn_castling_both_sides() {
        let fen = "r3k2r/pppppppp/8/8/8/8/PPPPPPPP/R3K2R w KQkq - 0 1";
        let src = format!("[Event \"Castle\"]\n[SetUp \"1\"]\n[FEN \"{fen}\"]\n\n1. O-O O-O-O *\n");
        let games = load_pgn(&src).expect("castling game loads");
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].moves.len(), 2);
        assert_eq!(games[0].startfen.as_deref(), Some(fen));
        assert_eq!(
            (games[0].moves[0].from(), games[0].moves[0].to()),
            (4, 6),
            "white kingside: e1 -> g1"
        );
        assert_eq!(
            (games[0].moves[1].from(), games[0].moves[1].to()),
            (60, 58),
            "black queenside: e8 -> c8"
        );
    }

    #[test]
    fn pgn_promotion() {
        let fen = "8/4P3/8/8/8/1k6/8/4K3 w - - 0 1";
        let src = format!("[Event \"Promo\"]\n[SetUp \"1\"]\n[FEN \"{fen}\"]\n\n1. e8=Q *\n");
        let games = load_pgn(&src).expect("promotion game loads");
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].moves.len(), 1);
        let mv = games[0].moves[0];
        assert_eq!(mv.from(), 52, "e7");
        assert_eq!(mv.to(), 60, "e8");
        assert_eq!(mv.promo(), 4, "queen");
    }

    #[test]
    fn pgn_multi_game() {
        let src = "[Event \"A\"]\n\n1. e4 e5 *\n\n[Event \"B\"]\n\n1. d4 d5 *\n";
        let games = load_pgn(src).expect("two games load");
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].tags[0], ("Event".to_string(), "A".to_string()));
        assert_eq!(games[1].tags[0], ("Event".to_string(), "B".to_string()));
        assert_eq!(games[0].moves.len(), 2);
        assert_eq!(games[1].moves.len(), 2);
    }

    #[test]
    fn pgn_bad_san_errors() {
        let src = "[Event \"Bad\"]\n\n1. e4 e5 2. Zz9 *\n";
        let e = load_pgn(src).unwrap_err();
        assert!(
            matches!(
                e,
                PgnError::BadSan {
                    game: 1,
                    ply: 3,
                    ..
                }
            ),
            "names game 1, ply 3: {e}"
        );
        let s = format!("{e}");
        assert!(s.contains("Zz9"), "message names the token: {s}");
        let _: &dyn std::error::Error = &e;
    }

    #[test]
    fn pgn_bad_tag_errors() {
        let e = load_pgn("[Event noquotes]\n\n1. e4 *\n").unwrap_err();
        assert!(matches!(e, PgnError::BadTag { game: 1, .. }), "got: {e}");
        let s = format!("{e}");
        assert!(s.contains("game 1"), "message names the game: {s}");
        let _: &dyn std::error::Error = &e;
    }

    #[test]
    fn pgn_non_ascii_san_is_bad_san() {
        // `\u{e9}` is 2 bytes in UTF-8, so `rest.len() - 2` lands
        // mid-character in a byte split; an emoji token does the same.
        // Both must surface as `BadSan`, never panic.
        for tok in ["a\u{e9}b", "\u{1F600}"] {
            let src = format!("[Event \"Unicode\"]\n\n1. e4 {tok} *\n");
            let e = load_pgn(&src).unwrap_err();
            assert!(
                matches!(e, PgnError::BadSan { .. }),
                "token {tok:?} must be BadSan, got: {e}"
            );
        }
    }
}
