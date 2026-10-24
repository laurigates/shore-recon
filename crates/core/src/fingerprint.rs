//! Content-addressed stage fingerprints.
//!
//! `fingerprint = blake3(stage name, stage config, upstream fingerprints, tool version)`.
//! Config is hashed as canonical JSON (object keys sorted), so key order in
//! TOML files does not affect the result.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
