//! Loading/saving text files: encodings, line endings, binary and size
//! guards, disk stamps, and the column conversion between the editor and a
//! language server.
//!
//! Loading never loses bytes silently:
//! - The buffer always holds `\n` line endings. A CRLF file is written back
//!   with CRLF. A file that mixes CRLF and LF keeps the ending of every line
//!   that was not edited (new lines get the majority ending).
//! - Text that is not valid in its encoding (UTF-8 unless the user picked
//!   another one) opens read-only ([`Loaded::lossless`] is false); saving it
//!   would replace the bad bytes with U+FFFD. A file read in another encoding
//!   is only editable when it decodes and re-encodes to the same bytes, and a
//!   save that cannot be encoded is refused.
//!
//! Line numbers are unaffected by the conversion and columns only lose the
//! trailing `\r`, so LSP positions stay valid.

use std::hash::{Hash, Hasher};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use encoding_rs::Encoding;

/// Files above this open read-only without syntax highlighting or LSP.
pub const LARGE_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Files above this are not opened at all.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Encodings a non-UTF-8 file can be reopened with (label, encoding).
pub const REOPEN_ENCODINGS: [(&str, &Encoding); 3] = [
    ("EUC-KR (CP949)", encoding_rs::EUC_KR),
    ("Shift_JIS", encoding_rs::SHIFT_JIS),
    ("Windows-1252", encoding_rs::WINDOWS_1252),
];

pub fn utf8() -> &'static Encoding {
    encoding_rs::UTF_8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
        }
    }
}

/// Majority vote; ties and files without newlines use LF.
pub fn detect_line_ending(text: &str) -> LineEnding {
    line_endings(text).0
}

/// Majority ending, and for files that mix CRLF and LF whether each line
/// (one entry per `\n`) ended in CRLF.
fn line_endings(text: &str) -> (LineEnding, Option<Vec<bool>>) {
    let bytes = text.as_bytes();
    let flags: Vec<bool> = bytes
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'\n')
        .map(|(i, _)| i > 0 && bytes[i - 1] == b'\r')
        .collect();
    let crlf = flags.iter().filter(|c| **c).count();
    let lf = flags.len() - crlf;
    let ending = if crlf > lf {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    };
    (ending, (crlf > 0 && lf > 0).then_some(flags))
}

pub fn to_lf(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    }
}

pub fn from_lf(text: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::Lf => text.to_string(),
        LineEnding::CrLf => {
            let mut out = String::with_capacity(text.len() + text.len() / 32);
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    out.push_str("\r\n");
                }
                out.push_str(line.strip_suffix('\r').unwrap_or(line));
            }
            out
        }
    }
}

/// The line endings of a file that mixes CRLF and LF, as read.
#[derive(Debug, PartialEq, Eq)]
pub struct MixedEndings {
    /// LF text as read.
    pub base: String,
    /// Per line of `base` (one per `\n`): ended in CRLF.
    pub crlf: Vec<bool>,
}

impl MixedEndings {
    pub fn counts(&self) -> (usize, usize) {
        let crlf = self.crlf.iter().filter(|c| **c).count();
        (crlf, self.crlf.len() - crlf)
    }
}

/// How a file's bytes map to the LF buffer, to write it back the same way.
#[derive(Clone, Debug, PartialEq)]
pub struct FileFormat {
    pub encoding: &'static Encoding,
    pub bom: bool,
    /// Majority ending (what new lines get).
    pub ending: LineEnding,
    /// Set when the file mixes CRLF and LF.
    pub mixed: Option<Arc<MixedEndings>>,
}

impl Default for FileFormat {
    fn default() -> Self {
        FileFormat {
            encoding: utf8(),
            bom: false,
            ending: LineEnding::Lf,
            mixed: None,
        }
    }
}

impl FileFormat {
    /// Status-bar text: `CRLF`, `LF`, or `혼합 (CRLF 3 · LF 5)`.
    pub fn ending_label(&self) -> String {
        match &self.mixed {
            Some(m) => {
                let (c, l) = m.counts();
                format!("혼합 (CRLF {c} · LF {l})")
            }
            None => self.ending.label().to_string(),
        }
    }
}

/// Identity of a file's bytes, to notice that the disk changed since the
/// buffer last read or wrote it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskStamp {
    pub len: u64,
    pub hash: u64,
}

pub fn stamp_of(bytes: &[u8]) -> DiskStamp {
    let mut h = std::hash::DefaultHasher::new();
    bytes.hash(&mut h);
    DiskStamp {
        len: bytes.len() as u64,
        hash: h.finish(),
    }
}

/// The stamp of the file now; `None` when it does not exist.
pub fn disk_stamp(path: &Path) -> std::io::Result<Option<DiskStamp>> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(stamp_of(&b))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    TooLarge(u64),
    Binary,
    Io(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::TooLarge(n) => write!(f, "파일이 너무 큽니다 ({} MB)", n / (1024 * 1024)),
            LoadError::Binary => write!(f, "바이너리 파일은 열 수 없습니다"),
            LoadError::Io(e) => write!(f, "파일을 읽지 못했습니다: {e}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Loaded {
    /// LF-normalised text.
    pub text: String,
    pub format: FileFormat,
    /// The bytes are valid in `format.encoding` and come back unchanged
    /// when written. When false the text shows U+FFFD for bad bytes and the
    /// file must stay read-only.
    pub lossless: bool,
    pub large: bool,
    pub len: u64,
    /// Stamp of the bytes read.
    pub stamp: DiskStamp,
}

pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    load_as(path, utf8())
}

pub fn load_as(path: &Path, encoding: &'static Encoding) -> Result<Loaded, LoadError> {
    let len = std::fs::metadata(path)
        .map_err(|e| LoadError::Io(e.to_string()))?
        .len();
    if len > MAX_FILE_BYTES {
        return Err(LoadError::TooLarge(len));
    }
    let bytes = std::fs::read(path).map_err(|e| LoadError::Io(e.to_string()))?;
    decode_as(bytes, len, encoding)
}

pub fn decode(bytes: Vec<u8>, len: u64) -> Result<Loaded, LoadError> {
    decode_as(bytes, len, utf8())
}

pub fn decode_as(
    mut bytes: Vec<u8>,
    len: u64,
    encoding: &'static Encoding,
) -> Result<Loaded, LoadError> {
    let stamp = stamp_of(&bytes);
    let bom = encoding == utf8() && bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    if bom {
        bytes.drain(..3);
    }
    if looks_binary(&bytes) {
        return Err(LoadError::Binary);
    }
    let (raw, lossless) = if encoding == utf8() {
        match String::from_utf8(bytes) {
            Ok(s) => (s, true),
            Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), false),
        }
    } else {
        let (text, had_errors) = encoding.decode_without_bom_handling(&bytes);
        let text = text.into_owned();
        // Only editable when writing it back gives the same bytes.
        let lossless = !had_errors && {
            let (back, _, unmappable) = encoding.encode(&text);
            !unmappable && back[..] == bytes[..]
        };
        (text, lossless)
    };
    let (ending, mixed) = line_endings(&raw);
    let text = to_lf(&raw);
    let mixed = mixed.map(|crlf| {
        Arc::new(MixedEndings {
            base: text.clone(),
            crlf,
        })
    });
    Ok(Loaded {
        text,
        format: FileFormat {
            encoding,
            bom,
            ending,
            mixed,
        },
        lossless,
        large: len > LARGE_FILE_BYTES,
        len,
        stamp,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// Characters `encoding` cannot represent.
    Unmappable(&'static str),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::Unmappable(name) => {
                write!(f, "{name}(으)로 저장할 수 없는 글자가 있습니다")
            }
        }
    }
}

/// Bytes to write for an LF buffer, restoring the file's encoding, BOM and
/// line endings.
pub fn encode(text: &str, format: &FileFormat) -> Result<Vec<u8>, EncodeError> {
    let body = match &format.mixed {
        Some(m) => restore_endings(text, m, format.ending),
        None => from_lf(text, format.ending),
    };
    if format.encoding == utf8() {
        let mut out = Vec::with_capacity(body.len() + 3);
        if format.bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        out.extend_from_slice(body.as_bytes());
        return Ok(out);
    }
    let (bytes, _, unmappable) = format.encoding.encode(&body);
    if unmappable {
        return Err(EncodeError::Unmappable(format.encoding.name()));
    }
    Ok(bytes.into_owned())
}

/// Mixed endings: lines matched to an unchanged line of the file as read
/// keep its ending, other lines get `default`.
fn restore_endings(text: &str, m: &MixedEndings, default: LineEnding) -> String {
    let old: Vec<&str> = m.base.split('\n').collect();
    let new: Vec<&str> = text.split('\n').collect();
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut crlf: Vec<Option<bool>> = vec![None; new.len()];
    for op in
        similar::capture_diff_slices_deadline(similar::Algorithm::Myers, &old, &new, Some(deadline))
    {
        if let similar::DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            for k in 0..len {
                crlf[new_index + k] = m.crlf.get(old_index + k).copied();
            }
        }
    }
    let mut out = String::with_capacity(text.len() + new.len());
    for (i, line) in new.iter().enumerate() {
        out.push_str(line);
        if i + 1 < new.len() {
            out.push_str(if crlf[i].unwrap_or(default == LineEnding::CrLf) {
                "\r\n"
            } else {
                "\n"
            });
        }
    }
    out
}

/// The final rename of [`write_atomic`] failed (typically another program
/// holds the file open without delete sharing). The original file is
/// untouched; [`write_in_place`] is the explicit, non-atomic alternative.
#[derive(Debug)]
pub struct ReplaceFailed(pub std::io::Error);

impl std::fmt::Display for ReplaceFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "파일을 바꾸지 못했습니다 (다른 프로그램이 사용 중일 수 있음): {}",
            self.0
        )
    }
}

impl std::error::Error for ReplaceFailed {}

/// The error came from the final rename of [`write_atomic`].
pub fn is_replace_failure(e: &std::io::Error) -> bool {
    e.get_ref().is_some_and(|r| r.is::<ReplaceFailed>())
}

fn resolve_target(path: &Path) -> std::io::Result<PathBuf> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path)?,
        _ => path.to_path_buf(),
    };
    if std::fs::metadata(&target).is_ok_and(|m| m.permissions().readonly()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "읽기 전용 파일입니다",
        ));
    }
    Ok(target)
}

/// A new, unique temporary file next to `target` (`create_new`, so an
/// existing file is never reused or clobbered).
fn create_temp(target: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut last = None;
    for _ in 0..16 {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(
            ".{name}.pod-save-{}-{seq}-{nanos:08x}.tmp",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no temporary file name")))
}

/// Replaces `path` with `bytes` in one step (temporary file in the same
/// folder, then rename), so a reader never sees a half-written file. A
/// symbolic link is followed: its target is replaced, the link stays.
/// Every failure leaves the original as it was and removes the temporary
/// file; a failed rename is reported as [`ReplaceFailed`] (never a silent
/// in-place write).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = resolve_target(path)?;
    let (tmp, mut f) = create_temp(&target)?;
    let res = (|| {
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &target).map_err(|e| std::io::Error::new(e.kind(), ReplaceFailed(e)))
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// Writes `bytes` over `path` in place (truncate + write; not atomic). Only
/// for an explicit "overwrite anyway" after [`write_atomic`] could not
/// replace the file.
pub fn write_in_place(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = resolve_target(path)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&target)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Column unit negotiated with a server (`general.positionEncodings`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PosEncoding {
    Utf8,
    #[default]
    Utf16,
    /// Unicode scalar values: the unit gpui-component's editor uses.
    Utf32,
}

impl PosEncoding {
    pub fn from_kind(kind: Option<&lsp_types::PositionEncodingKind>) -> Self {
        match kind.map(|k| k.as_str()) {
            Some("utf-8") => PosEncoding::Utf8,
            Some("utf-32") => PosEncoding::Utf32,
            _ => PosEncoding::Utf16,
        }
    }
}

/// Editor column (chars) -> wire column for one line of text.
pub fn char_col_to_wire(line: &str, chars: u32, enc: PosEncoding) -> u32 {
    let mut out = 0u32;
    for (i, c) in line.chars().enumerate() {
        if i as u32 >= chars {
            break;
        }
        out += match enc {
            PosEncoding::Utf8 => c.len_utf8() as u32,
            PosEncoding::Utf16 => c.len_utf16() as u32,
            PosEncoding::Utf32 => 1,
        };
    }
    if enc == PosEncoding::Utf32 {
        chars
    } else {
        out
    }
}

/// Wire column -> editor column (chars), clamped to the line.
pub fn wire_col_to_char(line: &str, wire: u32, enc: PosEncoding) -> u32 {
    if enc == PosEncoding::Utf32 {
        return wire;
    }
    let mut units = 0u32;
    let mut chars = 0u32;
    for c in line.chars() {
        if units >= wire {
            break;
        }
        units += match enc {
            PosEncoding::Utf8 => c.len_utf8() as u32,
            _ => c.len_utf16() as u32,
        };
        chars += 1;
    }
    chars
}

/// Text of line `row` without its line terminator (LF buffers).
pub fn line_of(text: &str, row: u32) -> &str {
    text.split('\n')
        .nth(row as usize)
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .unwrap_or("")
}

pub fn pos_to_wire(text: &str, p: lsp_types::Position, enc: PosEncoding) -> lsp_types::Position {
    if enc == PosEncoding::Utf32 {
        return p;
    }
    lsp_types::Position::new(
        p.line,
        char_col_to_wire(line_of(text, p.line), p.character, enc),
    )
}

pub fn pos_from_wire(text: &str, p: lsp_types::Position, enc: PosEncoding) -> lsp_types::Position {
    if enc == PosEncoding::Utf32 {
        return p;
    }
    lsp_types::Position::new(
        p.line,
        wire_col_to_char(line_of(text, p.line), p.character, enc),
    )
}

pub fn range_from_wire(text: &str, r: lsp_types::Range, enc: PosEncoding) -> lsp_types::Range {
    lsp_types::Range::new(
        pos_from_wire(text, r.start, enc),
        pos_from_wire(text, r.end, enc),
    )
}

/// Byte offset of an editor (char-column) position in an LF buffer.
pub fn offset_of(text: &str, p: lsp_types::Position) -> usize {
    let mut off = 0usize;
    for (i, line) in text.split('\n').enumerate() {
        if i as u32 == p.line {
            let col = line
                .char_indices()
                .nth(p.character as usize)
                .map(|(b, _)| b)
                .unwrap_or(line.len());
            return off + col;
        }
        off += line.len() + 1;
    }
    text.len()
}

/// Editor (char-column) position of a byte offset in an LF buffer.
pub fn position_of(text: &str, offset: usize) -> lsp_types::Position {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    lsp_types::Position::new(line, text[start..offset].chars().count() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position as P;

    fn round_trip(bytes: &[u8]) -> Loaded {
        let l = decode(bytes.to_vec(), bytes.len() as u64).unwrap();
        assert_eq!(
            encode(&l.text, &l.format).unwrap(),
            bytes,
            "bytes come back unchanged"
        );
        l
    }

    #[test]
    fn crlf_round_trip_keeps_bytes() {
        let loaded = round_trip(b"a\r\nb\r\n\r\nc");
        assert_eq!(loaded.format.ending, LineEnding::CrLf);
        assert_eq!(loaded.text, "a\nb\n\nc");
        let l = round_trip(b"x\ny\n");
        assert_eq!(l.format.ending, LineEnding::Lf);
        assert!(l.lossless && l.format.mixed.is_none());
    }

    #[test]
    fn bom_and_binary() {
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(b"hi\r\n");
        let l = round_trip(&b);
        assert!(l.format.bom);
        assert_eq!(l.text, "hi\n");
        assert_eq!(
            decode(vec![b'a', 0, b'b'], 3).unwrap_err(),
            LoadError::Binary
        );
    }

    /// editor-02: invalid UTF-8 is flagged (read-only), never saved lossy;
    /// an explicit encoding makes a CP949 file editable without loss.
    #[test]
    fn non_utf8_is_flagged_and_reopens_in_an_encoding() {
        let (cp949, _, _) = encoding_rs::EUC_KR.encode("한글 주석\r\nlet x = 1;\r\n");
        let cp949 = cp949.into_owned();
        let l = decode(cp949.clone(), cp949.len() as u64).unwrap();
        assert!(!l.lossless, "not UTF-8: read-only");
        assert!(l.text.contains('\u{FFFD}'));
        let k = decode_as(cp949.clone(), cp949.len() as u64, encoding_rs::EUC_KR).unwrap();
        assert!(k.lossless);
        assert_eq!(k.text, "한글 주석\nlet x = 1;\n");
        assert_eq!(encode(&k.text, &k.format).unwrap(), cp949);
        let edited = encode("한글 주석 고침\nlet x = 1;\n", &k.format).unwrap();
        assert_eq!(
            encoding_rs::EUC_KR.decode_without_bom_handling(&edited).0,
            "한글 주석 고침\r\nlet x = 1;\r\n"
        );
        // Characters the encoding has no bytes for: refused, not replaced.
        assert_eq!(
            encode("😀", &k.format),
            Err(EncodeError::Unmappable("EUC-KR"))
        );
        // Bytes that are not valid EUC-KR either stay read-only.
        let bad = vec![b'a', 0xFF, 0xFF, b'\n'];
        assert!(
            !decode_as(bad.clone(), 4, encoding_rs::EUC_KR)
                .unwrap()
                .lossless
        );
        assert!(!decode(bad, 4).unwrap().lossless);
    }

    /// editor-08: a file that mixes CRLF and LF keeps the ending of every
    /// line that was not edited; new lines get the majority ending.
    #[test]
    fn mixed_line_endings_survive_an_edit() {
        let raw = b"a\r\nb\nc\r\nd\r\n";
        let l = round_trip(raw);
        assert_eq!(l.format.ending_label(), "혼합 (CRLF 3 · LF 1)");
        assert_eq!(l.format.ending, LineEnding::CrLf);
        // Edit line c, insert a line after a: b keeps LF, the rest CRLF.
        let out = encode("a\nnew\nb\nC2\nd\n", &l.format).unwrap();
        assert_eq!(out, b"a\r\nnew\r\nb\nC2\r\nd\r\n");
        assert_eq!(detect_line_ending("x\ny\r\n"), LineEnding::Lf, "ties: LF");
    }

    #[test]
    fn atomic_write_and_stamps() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        assert_eq!(disk_stamp(&p).unwrap(), None);
        write_atomic(&p, b"one").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"one");
        assert_eq!(disk_stamp(&p).unwrap(), Some(stamp_of(b"one")));
        write_atomic(&p, b"two!").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two!");
        assert_ne!(stamp_of(b"one"), stamp_of(b"two"));
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "no temporary file left: {names:?}");
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&p, perm.clone()).unwrap();
        assert!(
            write_atomic(&p, b"x").is_err(),
            "read-only files are not replaced"
        );
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(&p, perm).unwrap();
    }

    /// ui-editor-02: another program holds the file without delete sharing.
    /// The save fails as a replace failure (no silent in-place write), the
    /// original stays, no temporary file is left; the explicit in-place
    /// overwrite then works, and a retry succeeds once the file is free.
    #[cfg(windows)]
    #[test]
    fn rename_blocked_by_a_reader_is_reported_not_overwritten() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("held.txt");
        std::fs::write(&p, b"original").unwrap();
        let names = || {
            let mut v: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        {
            // FILE_SHARE_READ | FILE_SHARE_WRITE, no FILE_SHARE_DELETE.
            let _reader = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0x1 | 0x2)
                .open(&p)
                .unwrap();
            for _ in 0..3 {
                let e = write_atomic(&p, b"new").expect_err("rename must fail");
                assert!(is_replace_failure(&e), "{e}");
                assert_eq!(std::fs::read(&p).unwrap(), b"original");
                assert_eq!(names(), vec!["held.txt".to_string()], "temp cleaned");
            }
            write_in_place(&p, b"forced").unwrap();
            assert_eq!(std::fs::read(&p).unwrap(), b"forced");
        }
        write_atomic(&p, b"retried").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"retried");
        assert_eq!(names(), vec!["held.txt".to_string()]);
    }

    /// Temporary names never collide (pid alone is not unique within one
    /// process), and an existing file with a candidate name is not reused.
    #[test]
    fn temporary_names_are_unique() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        let (t1, f1) = create_temp(&p).unwrap();
        let (t2, f2) = create_temp(&p).unwrap();
        assert_ne!(t1, t2);
        drop((f1, f2));
        assert!(t1.exists() && t2.exists());
    }

    #[test]
    fn size_guard_flags() {
        let l = decode(b"x".to_vec(), LARGE_FILE_BYTES + 1).unwrap();
        assert!(l.large);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.txt");
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(matches!(load(&p), Err(LoadError::TooLarge(_))));
    }

    #[test]
    fn utf16_columns_with_astral_and_hangul() {
        // '😀' is 2 UTF-16 units / 4 UTF-8 bytes, '한' is 1 unit / 3 bytes.
        let line = "a😀한b";
        assert_eq!(char_col_to_wire(line, 3, PosEncoding::Utf16), 4);
        assert_eq!(char_col_to_wire(line, 3, PosEncoding::Utf8), 8);
        assert_eq!(char_col_to_wire(line, 3, PosEncoding::Utf32), 3);
        assert_eq!(wire_col_to_char(line, 4, PosEncoding::Utf16), 3);
        assert_eq!(wire_col_to_char(line, 8, PosEncoding::Utf8), 3);
        assert_eq!(wire_col_to_char(line, 99, PosEncoding::Utf16), 4);
        let text = "fn x() {}\nlet s = \"😀\"; s.len()\n";
        let p = P::new(1, 13);
        let w = pos_to_wire(text, p, PosEncoding::Utf16);
        assert_eq!(w, P::new(1, 14));
        assert_eq!(pos_from_wire(text, w, PosEncoding::Utf16), p);
    }

    #[test]
    fn offsets_and_positions() {
        let text = "ab\n한c\n";
        assert_eq!(offset_of(text, P::new(1, 1)), 3 + 3);
        assert_eq!(position_of(text, 6), P::new(1, 1));
        assert_eq!(offset_of(text, P::new(9, 0)), text.len());
        assert_eq!(line_of("a\r\nb", 0), "a");
    }
}
