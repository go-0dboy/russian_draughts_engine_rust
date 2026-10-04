//! Compact training/opening formats used by the local engine laboratory.
//! They are intentionally dependency-free and versioned so large historical
//! corpora need to be parsed only once.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::{Move, PdnOutcome, Position, Side};

const TUNE_MAGIC: [u8; 8] = *b"RDTUNE2\0";
const OPEN_MAGIC: [u8; 8] = *b"RDOPEN2\0";
const FORMAT_VERSION: u32 = 1;
const TUNE_RECORD_SIZE: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DataSplit {
    Train = 0,
    Validation = 1,
    Test = 2,
}

impl DataSplit {
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Train),
            1 => Some(Self::Validation),
            2 => Some(Self::Test),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuneRecord {
    pub position: Position,
    pub outcome: PdnOutcome,
    pub split: DataSplit,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TuneFileInfo {
    pub records: u64,
    pub source_games: u64,
    pub accepted_games: u64,
    pub rejected_games: u64,
}

pub fn write_tune_file(
    path: impl AsRef<Path>,
    records: &[TuneRecord],
    info: TuneFileInfo,
) -> io::Result<()> {
    let file = File::create(path)?;
    let mut out = BufWriter::new(file);
    out.write_all(&TUNE_MAGIC)?;
    write_u32(&mut out, FORMAT_VERSION)?;
    write_u32(&mut out, TUNE_RECORD_SIZE)?;
    write_u64(&mut out, records.len() as u64)?;
    write_u64(&mut out, info.source_games)?;
    write_u64(&mut out, info.accepted_games)?;
    write_u64(&mut out, info.rejected_games)?;
    for record in records {
        write_u32(&mut out, record.position.white)?;
        write_u32(&mut out, record.position.black)?;
        write_u32(&mut out, record.position.kings)?;
        out.write_all(&[
            record.position.side_to_move as u8,
            record.outcome.code(),
            record.split as u8,
            0,
        ])?;
    }
    out.flush()
}

pub fn read_tune_file(path: impl AsRef<Path>) -> io::Result<(TuneFileInfo, Vec<TuneRecord>)> {
    let file = File::open(path)?;
    let mut input = BufReader::new(file);
    let mut magic = [0u8; 8];
    input.read_exact(&mut magic)?;
    if magic != TUNE_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not an RDTUNE2 file",
        ));
    }
    let version = read_u32(&mut input)?;
    if version != FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported tune format version {version}"),
        ));
    }
    let record_size = read_u32(&mut input)?;
    if record_size != TUNE_RECORD_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected tune record size {record_size}"),
        ));
    }
    let count = read_u64(&mut input)?;
    let source_games = read_u64(&mut input)?;
    let accepted_games = read_u64(&mut input)?;
    let rejected_games = read_u64(&mut input)?;
    if count > 50_000_000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unreasonable tune record count",
        ));
    }
    let mut records = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let white = read_u32(&mut input)?;
        let black = read_u32(&mut input)?;
        let kings = read_u32(&mut input)?;
        let mut tail = [0u8; 4];
        input.read_exact(&mut tail)?;
        let side_to_move = match tail[0] {
            0 => Side::White,
            1 => Side::Black,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid side code",
                ))
            }
        };
        let outcome = PdnOutcome::from_code(tail[1])
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid outcome code"))?;
        let split = DataSplit::from_code(tail[2])
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid split code"))?;
        if tail[3] != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "non-zero tune reserved byte",
            ));
        }
        let position = Position {
            white,
            black,
            kings,
            side_to_move,
        };
        if !position.is_valid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid position in tune file",
            ));
        }
        records.push(TuneRecord {
            position,
            outcome,
            split,
        });
    }
    reject_trailing_bytes(&mut input, "tune")?;
    Ok((
        TuneFileInfo {
            records: count,
            source_games,
            accepted_games,
            rejected_games,
        },
        records,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpeningLine {
    pub moves: Vec<Move>,
}

pub fn write_opening_file(path: impl AsRef<Path>, openings: &[OpeningLine]) -> io::Result<()> {
    let file = File::create(path)?;
    let mut out = BufWriter::new(file);
    out.write_all(&OPEN_MAGIC)?;
    write_u32(&mut out, FORMAT_VERSION)?;
    write_u32(&mut out, 0)?; // reserved
    write_u64(&mut out, openings.len() as u64)?;
    for opening in openings {
        if opening.moves.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "opening is too long",
            ));
        }
        write_u16(&mut out, opening.moves.len() as u16)?;
        write_u16(&mut out, 0)?;
        for mv in &opening.moves {
            write_u64(&mut out, mv.raw())?;
        }
    }
    out.flush()
}

pub fn read_opening_file(path: impl AsRef<Path>) -> io::Result<Vec<OpeningLine>> {
    let file = File::open(path)?;
    let mut input = BufReader::new(file);
    let mut magic = [0u8; 8];
    input.read_exact(&mut magic)?;
    if magic != OPEN_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not an RDOPEN2 file",
        ));
    }
    let version = read_u32(&mut input)?;
    if version != FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported opening format",
        ));
    }
    let reserved = read_u32(&mut input)?;
    if reserved != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "non-zero opening header reserved field",
        ));
    }
    let count = read_u64(&mut input)?;
    if count > 10_000_000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unreasonable opening count",
        ));
    }
    let mut openings = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let len = read_u16(&mut input)? as usize;
        let flags = read_u16(&mut input)?;
        if flags != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported opening flags",
            ));
        }
        let mut moves = Vec::with_capacity(len);
        for _ in 0..len {
            moves.push(Move::from_raw(read_u64(&mut input)?));
        }
        openings.push(OpeningLine { moves });
    }
    reject_trailing_bytes(&mut input, "opening")?;
    Ok(openings)
}

fn reject_trailing_bytes(input: &mut impl Read, kind: &str) -> io::Result<()> {
    let mut extra = [0u8; 1];
    if input.read(&mut extra)? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("trailing bytes after {kind} records"),
        ));
    }
    Ok(())
}

fn write_u16(out: &mut impl Write, value: u16) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}
fn write_u32(out: &mut impl Write, value: u32) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}
fn write_u64(out: &mut impl Write, value: u64) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}
fn read_u16(input: &mut impl Read) -> io::Result<u16> {
    let mut b = [0u8; 2];
    input.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn read_u32(input: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    input.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn read_u64(input: &mut impl Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    input.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate_moves, Position, DEFAULT_MOVE_BUFFER_CAPACITY};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(ext: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rd-classic-{nonce}.{ext}"))
    }

    #[test]
    fn tune_file_round_trip() {
        let path = temp_path("rdtune");
        let records = [
            TuneRecord {
                position: Position::initial(),
                outcome: PdnOutcome::Draw,
                split: DataSplit::Train,
            },
            TuneRecord {
                position: Position {
                    side_to_move: Side::Black,
                    ..Position::initial()
                },
                outcome: PdnOutcome::WhiteWin,
                split: DataSplit::Validation,
            },
        ];
        let info = TuneFileInfo {
            records: records.len() as u64,
            source_games: 10,
            accepted_games: 9,
            rejected_games: 1,
        };
        write_tune_file(&path, &records, info).unwrap();
        let (loaded_info, loaded) = read_tune_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded_info, info);
        assert_eq!(loaded.as_slice(), &records);
    }

    #[test]
    fn opening_file_round_trip_preserves_packed_moves() {
        let path = temp_path("rdopen");
        let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let len = generate_moves(Position::initial(), &mut legal);
        assert!(len > 0);
        let openings = vec![OpeningLine {
            moves: vec![legal[0]],
        }];
        write_opening_file(&path, &openings).unwrap();
        let loaded = read_opening_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded, openings);
    }
}
