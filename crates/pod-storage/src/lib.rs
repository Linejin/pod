//! Pod leaf crate for the JSON files the daemon and its libraries keep
//! (orch runs, the fake forge, mod state, the discovery cache).
//!
//! Only the shared mechanics live here; each caller keeps its own policy:
//! creating the parent directory (or not), choosing the temporary path,
//! converting errors, removing a temporary file after a failure, moving a
//! corrupt file aside. [`replace::write_then_rename`] replaces a file in one
//! rename (readers never see a half-written file); it does not `fsync`, so it
//! is no promise of durability across a power loss.

/// Serialization in the stores' on-disk format.
pub mod json {
    use serde::Serialize;

    /// `value` as pretty JSON (two-space indent, no trailing newline),
    /// exactly `serde_json::to_vec_pretty`. Fails before anything is
    /// written, with serde_json's error so callers keep their conversion.
    pub fn to_pretty_bytes<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(value)
    }
}

/// Whole-file replacement through a temporary file.
pub mod replace {
    use std::path::Path;

    /// Writes `bytes` to `tmp` (created or truncated), then renames `tmp` to
    /// `path`, replacing it. Nothing else: no directory is created and
    /// nothing is removed. A failed write returns before the rename (`path`
    /// untouched; `tmp` may hold part of `bytes`); a failed rename leaves
    /// `tmp` in place. Cleaning up `tmp` is the caller's choice.
    pub fn write_then_rename(path: &Path, tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
        std::fs::write(tmp, bytes)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::json::to_pretty_bytes;
    use super::replace::write_then_rename;
    use serde::Serialize;
    use std::collections::BTreeMap;

    #[derive(Serialize)]
    struct Record {
        id: &'static str,
        n: u32,
        tags: Vec<&'static str>,
        none: Option<u8>,
        #[serde(skip_serializing_if = "Option::is_none")]
        skipped: Option<u8>,
    }

    #[test]
    fn pretty_bytes_are_serde_json_pretty() {
        let r = Record {
            id: "a",
            n: 7,
            tags: vec!["x", "y"],
            none: None,
            skipped: None,
        };
        let golden = "{\n  \"id\": \"a\",\n  \"n\": 7,\n  \"tags\": [\n    \"x\",\n    \"y\"\n  ],\n  \"none\": null\n}";
        let bytes = to_pretty_bytes(&r).unwrap();
        assert_eq!(String::from_utf8(bytes.clone()).unwrap(), golden);
        assert_eq!(bytes, serde_json::to_vec_pretty(&r).unwrap());
        // Unsized values work too (callers pass slices and str).
        assert_eq!(to_pretty_bytes("é").unwrap(), b"\"\xc3\xa9\"");
        assert_eq!(to_pretty_bytes(&[1u8][..]).unwrap(), b"[\n  1\n]");
    }

    struct Fails;
    impl Serialize for Fails {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("refused"))
        }
    }

    #[test]
    fn serialization_failure_is_serde_jsons_error() {
        let e = to_pretty_bytes(&Fails).unwrap_err();
        assert_eq!(e.to_string(), "refused");
        assert!(e.is_data(), "{e:?}");
        // A map key that is not a string fails too (nothing about I/O).
        let mut m = BTreeMap::new();
        m.insert((1, 2), 3);
        let e = to_pretty_bytes(&m).unwrap_err();
        assert_eq!(e.to_string(), "key must be a string");
        assert!(!e.is_io(), "{e:?}");
        // Callers convert it as before: `?` into io::Error is InvalidData,
        // `io::Error::other` keeps the message.
        let io: std::io::Error = to_pretty_bytes(&Fails).unwrap_err().into();
        assert_eq!(io.kind(), std::io::ErrorKind::InvalidData);
        let other = std::io::Error::other(to_pretty_bytes(&Fails).unwrap_err());
        assert_eq!(other.to_string(), "refused");
    }

    #[test]
    fn write_then_rename_creates_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let tmp = dir.path().join("state.json.tmp");
        write_then_rename(&path, &tmp, b"one").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        assert!(!tmp.exists());
        // A stale temporary file from an earlier crash is overwritten.
        std::fs::write(&tmp, b"stale stale stale").unwrap();
        write_then_rename(&path, &tmp, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert!(!tmp.exists());
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["state.json"]);
    }

    #[test]
    fn failed_write_stops_before_the_rename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"old").unwrap();
        // The temporary file's directory does not exist (and is not created).
        let tmp = dir.path().join("missing").join("state.json.tmp");
        let e = write_then_rename(&path, &tmp, b"new").unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert!(!dir.path().join("missing").exists());
        // The destination's directory is not created either.
        let nested = dir.path().join("a").join("state.json");
        let nested_tmp = dir.path().join("a").join("state.json.tmp");
        assert!(write_then_rename(&nested, &nested_tmp, b"x").is_err());
        assert!(!dir.path().join("a").exists());
    }

    #[test]
    fn failed_rename_leaves_the_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        // The destination is a non-empty directory: the write succeeds, the
        // rename cannot replace it.
        let path = dir.path().join("state.json");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep"), b"k").unwrap();
        let tmp = dir.path().join("state.json.tmp");
        assert!(write_then_rename(&path, &tmp, b"new").is_err());
        assert_eq!(std::fs::read(&tmp).unwrap(), b"new");
        assert!(path.is_dir());
        assert_eq!(std::fs::read(path.join("keep")).unwrap(), b"k");
    }
}
