//! Content-addressed stage fingerprints.
//!
//! `fingerprint = blake3(stage name, stage config, upstream fingerprints, tool version)`.
//! Config is hashed as canonical JSON (object keys sorted), so key order in
//! TOML files does not affect the result.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::io_error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn compute<C: Serialize>(
        stage: &str,
        config: &C,
        upstream: &[Fingerprint],
        tool_version: &str,
    ) -> Self {
        let config = serde_json::to_vec(
            &serde_json::to_value(config).expect("stage config serialises to JSON"),
        )
        .expect("JSON value serialises");
        let mut upstream = upstream.to_vec();
        upstream.sort_unstable();

        let mut h = blake3::Hasher::new();
        for field in [stage.as_bytes(), &config, tool_version.as_bytes()] {
            h.update(&(field.len() as u64).to_le_bytes());
            h.update(field);
        }
        h.update(&(upstream.len() as u64).to_le_bytes());
        for fp in &upstream {
            h.update(&fp.0);
        }
        Self(*h.finalize().as_bytes())
    }

    /// Content identity of `path`: blake3 of a file's bytes, or for a
    /// directory, of every file below it (relative path and content hash)
    /// in sorted order. Entries whose names start with `.` are skipped.
    pub fn of_content(path: &Path) -> crate::Result<Self> {
        let mut h = blake3::Hasher::new();
        if std::fs::metadata(path).map_err(io_error(path))?.is_dir() {
            h.update(b"dir\0");
            let mut files = Vec::new();
            files_below(path, Path::new(""), &mut files)?;
            files.sort();
            for rel in &files {
                let name = rel.as_os_str().as_encoded_bytes();
                h.update(&(name.len() as u64).to_le_bytes());
                h.update(name);
                h.update(&Self::of_content(&path.join(rel))?.0);
            }
        } else {
            h.update(b"file\0");
            let file = std::fs::File::open(path).map_err(io_error(path))?;
            h.update_reader(file).map_err(io_error(path))?;
        }
        Ok(Self(*h.finalize().as_bytes()))
    }

    /// Short form used in cache directory names.
    pub fn short(&self) -> String {
        self.to_string()[..16].to_owned()
    }
}

/// Files below `root/rel`, relative to `root`, skipping `.` entries.
fn files_below(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> crate::Result<()> {
    let dir = root.join(rel);
    for entry in std::fs::read_dir(&dir).map_err(io_error(&dir))? {
        let name = entry.map_err(io_error(&dir))?.file_name();
        if name.as_encoded_bytes().starts_with(b".") {
            continue;
        }
        let rel = rel.join(&name);
        let path = root.join(&rel);
        if std::fs::metadata(&path).map_err(io_error(&path))?.is_dir() {
            files_below(root, &rel, out)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

impl FromStr for Fingerprint {
    type Err = String;

    /// Parse the 64-digit hex form written by `Display`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("not a 64-digit hex fingerprint: {s:?}");
        if s.len() != 64 || !s.is_ascii() {
            return Err(invalid());
        }
        let mut bytes = [0u8; 32];
        for (byte, pair) in bytes.iter_mut().zip(s.as_bytes().chunks(2)) {
            let pair = std::str::from_utf8(pair).map_err(|_| invalid())?;
            *byte = u8::from_str_radix(pair, 16).map_err(|_| invalid())?;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for Fingerprint {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;

    use crate::Error;

    fn content(path: &Path) -> Fingerprint {
        Fingerprint::of_content(path).unwrap()
    }

    #[test]
    fn file_content_identity_ignores_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"));
        fs::write(&a, b"frames").unwrap();
        fs::write(&b, b"frames").unwrap();
        assert_eq!(content(&a), content(&b));
        fs::write(&b, b"framez").unwrap();
        assert_ne!(content(&a), content(&b));
    }

    #[test]
    fn directory_identity_covers_names_nesting_and_content_but_not_dotfiles() {
        let root = tempfile::tempdir().unwrap();
        let tree = |name: &str, files: &[(&str, &str)]| {
            let dir = root.path().join(name);
            for (rel, body) in files {
                let path = dir.join(rel);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, body).unwrap();
            }
            content(&dir)
        };
        let base = tree("base", &[("1.jpg", "x"), ("cam/2.jpg", "y")]);
        assert_eq!(
            base,
            tree("reordered", &[("cam/2.jpg", "y"), ("1.jpg", "x")])
        );
        let with_dotfile = [("1.jpg", "x"), ("cam/2.jpg", "y"), (".DS_Store", "z")];
        assert_eq!(base, tree("dotfile", &with_dotfile));
        assert_ne!(base, tree("renamed", &[("1.jpg", "x"), ("cam/3.jpg", "y")]));
        assert_ne!(base, tree("moved", &[("1.jpg", "x"), ("2.jpg", "y")]));
        assert_ne!(base, tree("edited", &[("1.jpg", "x"), ("cam/2.jpg", "Y")]));
    }

    #[test]
    fn a_directory_differs_from_its_only_file() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("d")).unwrap();
        fs::write(root.path().join("d/f"), b"x").unwrap();
        assert_ne!(
            content(&root.path().join("d")),
            content(&root.path().join("d/f"))
        );
    }

    #[test]
    fn missing_content_path_is_an_io_error() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("nope.mp4");
        let err = Fingerprint::of_content(&missing).unwrap_err();
        assert!(
            matches!(&err, Error::Io { path, .. } if *path == missing),
            "{err}"
        );
    }

    fn fp(cfg: &impl Serialize, up: &[Fingerprint], ver: &str) -> Fingerprint {
        Fingerprint::compute("sfm", cfg, up, ver)
    }

    #[test]
    fn deterministic() {
        let cfg = serde_json::json!({"a": 1});
        assert_eq!(fp(&cfg, &[], "1"), fp(&cfg, &[], "1"));
    }

    #[test]
    fn independent_of_key_order() {
        let a: HashMap<_, _> = [("x", 1), ("y", 2)].into();
        let b: HashMap<_, _> = [("y", 2), ("x", 1)].into();
        assert_eq!(fp(&a, &[], "1"), fp(&b, &[], "1"));
    }

    #[test]
    fn independent_of_upstream_order() {
        let cfg = serde_json::json!({});
        let (u1, u2) = (fp(&1, &[], "a"), fp(&2, &[], "a"));
        assert_eq!(fp(&cfg, &[u1, u2], "1"), fp(&cfg, &[u2, u1], "1"));
    }

    #[test]
    fn sensitive_to_every_input() {
        let cfg = serde_json::json!({"a": 1});
        let base = fp(&cfg, &[], "1");
        assert_ne!(base, Fingerprint::compute("dense", &cfg, &[], "1"));
        assert_ne!(base, fp(&serde_json::json!({"a": 2}), &[], "1"));
        assert_ne!(base, fp(&cfg, &[base], "1"));
        assert_ne!(base, fp(&cfg, &[], "2"));
    }

    #[test]
    fn field_boundaries_are_unambiguous() {
        // Without length prefixes both pairs would hash the same bytes.
        assert_ne!(
            Fingerprint::compute("s", &1, &[], "23"),
            Fingerprint::compute("s", &12, &[], "3"),
        );
        assert_ne!(
            Fingerprint::compute("s1", &2, &[], "3"),
            Fingerprint::compute("s", &12, &[], "3"),
        );
    }

    #[test]
    fn parses_its_display_form_and_serialises_as_a_hex_string() {
        let a = fp(&0, &[], "1");
        assert_eq!(a.to_string().parse::<Fingerprint>(), Ok(a));
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, format!("\"{a}\""));
        assert_eq!(serde_json::from_str::<Fingerprint>(&json).unwrap(), a);
    }

    #[test]
    fn rejects_malformed_hex() {
        let s = fp(&0, &[], "1").to_string();
        for bad in [
            &s[..62],
            &format!("{s}00"),
            &format!("zz{}", &s[2..]),
            &format!("é{}", &s[2..]),
        ] {
            assert!(bad.parse::<Fingerprint>().is_err(), "{bad}");
        }
    }

    #[test]
    fn display_is_hex() {
        let s = fp(&0, &[], "1").to_string();
        assert_eq!(s.len(), 64);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(fp(&0, &[], "1").short(), s[..16]);
    }
}
