//! Content-addressed stage fingerprints.
//!
//! `fingerprint = blake3(stage name, stage config, upstream fingerprints, tool version)`.
//! Config is hashed as canonical JSON (object keys sorted), so key order in
//! TOML files does not affect the result.

use std::fmt;

use serde::Serialize;

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

    /// Short form used in cache directory names.
    pub fn short(&self) -> String {
        self.to_string()[..16].to_owned()
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
        assert_ne!(
            Fingerprint::compute("ab", &"", &[], "c"),
            Fingerprint::compute("a", &"", &[], "bc"),
        );
    }

    #[test]
    fn display_is_hex() {
        let s = fp(&0, &[], "1").to_string();
        assert_eq!(s.len(), 64);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(fp(&0, &[], "1").short(), s[..16]);
    }
}
