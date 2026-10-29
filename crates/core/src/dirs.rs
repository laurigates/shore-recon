//! Per-user directories for an installed `shoreline-recon` (ADR-0017).
//!
//! The functions take the environment as a lookup so they can be tested
//! without touching the process environment; [`config_dir`] and
//! [`cache_dir`] read the real one.

use std::ffi::OsString;
use std::path::PathBuf;

const APP: &str = "shoreline-recon";

/// Environment lookup, `std::env::var_os` in production.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// User config directory: `SHORELINE_CONFIG_DIR`, else
/// `$XDG_CONFIG_HOME/shoreline-recon`, else `~/.config/shoreline-recon`
/// (macOS included, as for most command-line tools).
pub fn config_dir_from(env: Env) -> Option<PathBuf> {
    if let Some(dir) = env("SHORELINE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Some(dir.into());
    }
    xdg(env, "XDG_CONFIG_HOME").or_else(|| home(env).map(|h| h.join(".config").join(APP)))
}

/// User cache directory, where the quick mode keeps its stage cache:
/// `$XDG_CACHE_HOME/shoreline-recon`, else `~/Library/Caches/shoreline-recon`
/// on macOS and `~/.cache/shoreline-recon` elsewhere.
pub fn cache_dir_from(env: Env, os: &str) -> Option<PathBuf> {
    xdg(env, "XDG_CACHE_HOME").or_else(|| {
        let home = home(env)?;
        Some(match os {
            "macos" => home.join("Library/Caches").join(APP),
            _ => home.join(".cache").join(APP),
        })
    })
}

pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(&|k| std::env::var_os(k))
}

pub fn cache_dir() -> Option<PathBuf> {
    cache_dir_from(&|k| std::env::var_os(k), std::env::consts::OS)
}

/// An XDG base directory joined with the app name; relative values are
/// ignored, as the XDG spec requires.
fn xdg(env: Env, key: &str) -> Option<PathBuf> {
    env(key)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join(APP))
}

fn home(env: Env) -> Option<PathBuf> {
    env("HOME").map(PathBuf::from).filter(|p| p.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: BTreeMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn config_dir_prefers_the_override_then_xdg_then_home() {
        let all = env(&[
            ("SHORELINE_CONFIG_DIR", "/o"),
            ("XDG_CONFIG_HOME", "/x"),
            ("HOME", "/h"),
        ]);
        assert_eq!(config_dir_from(&all), Some("/o".into()));
        let xdg = env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")]);
        assert_eq!(config_dir_from(&xdg), Some("/x/shoreline-recon".into()));
        let home = env(&[("HOME", "/h")]);
        assert_eq!(
            config_dir_from(&home),
            Some("/h/.config/shoreline-recon".into())
        );
    }

    #[test]
    fn relative_xdg_and_empty_override_are_ignored() {
        let e = env(&[
            ("SHORELINE_CONFIG_DIR", ""),
            ("XDG_CONFIG_HOME", "rel"),
            ("HOME", "/h"),
        ]);
        assert_eq!(
            config_dir_from(&e),
            Some("/h/.config/shoreline-recon".into())
        );
    }

    #[test]
    fn cache_dir_differs_by_os() {
        let e = env(&[("HOME", "/h")]);
        assert_eq!(
            cache_dir_from(&e, "macos"),
            Some("/h/Library/Caches/shoreline-recon".into())
        );
        assert_eq!(
            cache_dir_from(&e, "linux"),
            Some("/h/.cache/shoreline-recon".into())
        );
        let xdg = env(&[("XDG_CACHE_HOME", "/c"), ("HOME", "/h")]);
        assert_eq!(
            cache_dir_from(&xdg, "macos"),
            Some("/c/shoreline-recon".into())
        );
    }

    #[test]
    fn no_home_gives_none() {
        let e = env(&[]);
        assert_eq!(config_dir_from(&e), None);
        assert_eq!(cache_dir_from(&e, "linux"), None);
    }
}
