//! PLY header element counts, for stage metrics (Gaussians, points, faces).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use shoreline_core::error::io_error;
use shoreline_core::{Error, Result};

/// `element <name> <count>` lines of `path`'s header, keyed by name.
pub fn element_counts(path: &Path) -> Result<BTreeMap<String, u64>> {
    let file = std::fs::File::open(path).map_err(io_error(path))?;
    let bad = |why: &str| Error::Stage {
        stage: crate::NAME,
        message: format!("{}: {why}", path.display()),
    };
    let mut lines = BufReader::new(file).split(b'\n');
    let mut next = || -> Result<Option<String>> {
        lines
            .next()
            .transpose()
            .map_err(io_error(path))
            .map(|l| l.map(|l| String::from_utf8_lossy(&l).trim_end().to_owned()))
    };
    if next()?.as_deref() != Some("ply") {
        return Err(bad("not a PLY file"));
    }
    let mut counts = BTreeMap::new();
    while let Some(line) = next()? {
        if line == "end_header" {
            return Ok(counts);
        }
        if let ["element", name, n] = line.split_whitespace().collect::<Vec<_>>()[..] {
            let n = n
                .parse()
                .map_err(|_| bad(&format!("bad element count in {line:?}")))?;
            counts.insert(name.to_owned(), n);
        }
    }
    Err(bad("no end_header"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_come_from_the_header_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.ply");
        let mut bytes =
            b"ply\nformat binary_little_endian 1.0\nelement vertex 3\nproperty float x\n\
                          element face 1\nproperty list uchar int vertex_indices\nend_header\n"
                .to_vec();
        bytes.extend([0xff, 0x00, b'\n', 0x80]);
        std::fs::write(&path, bytes).unwrap();
        let counts = element_counts(&path).unwrap();
        assert_eq!(
            counts,
            [("face".to_owned(), 1), ("vertex".to_owned(), 3)].into()
        );
    }

    #[test]
    fn non_ply_or_unterminated_headers_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.ply");
        std::fs::write(&path, "obj\n").unwrap();
        assert!(element_counts(&path).is_err());
        std::fs::write(&path, "ply\nelement vertex 3\n").unwrap();
        assert!(element_counts(&path).is_err());
    }
}
