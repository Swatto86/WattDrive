//! Refuse ambiguous Windows paths before planning any local or remote changes.
use std::collections::HashMap;
use wattdrive_domain::{DriveError, RelPath};

pub fn validate<'a>(paths: impl Iterator<Item = &'a RelPath>) -> Result<(), DriveError> {
    let mut names = HashMap::new();
    for path in paths {
        let mut prefix = String::new();
        for part in path.as_str().split('/') {
            let stem = part.split('.').next().unwrap_or(part).to_uppercase();
            if part.ends_with(['.', ' '])
                || part.chars().any(|c| c < ' ' || "\\:*?\"<>|".contains(c))
                || matches!(
                    stem.as_str(),
                    "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                )
                || ["COM", "LPT"].iter().any(|p| {
                    stem.strip_prefix(p).is_some_and(|n| {
                        matches!(
                            n,
                            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                        )
                    })
                })
            {
                return Err(DriveError::Other(format!("Sync paused: {path} cannot be represented safely on Windows. Rename it on iCloud or another device.")));
            }
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if let Some(previous) = names.insert(prefix.to_uppercase(), prefix.clone()) {
                if previous != prefix {
                    return Err(DriveError::Other(format!("Sync paused: {previous} and {prefix} differ only by case. Rename one before syncing on Windows.")));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(paths: &[&str]) -> Result<(), DriveError> {
        let paths: Vec<_> = paths.iter().map(|p| RelPath::new(p).unwrap()).collect();
        validate(paths.iter())
    }
    #[test]
    fn rejects_aliases_devices_streams_and_escapes() {
        for path in [
            "a/..\\outside",
            "C:relative",
            "a:stream",
            "NUL.txt",
            "COM1",
            "lpt².txt",
            "a.",
            "a ",
            "a?b",
        ] {
            assert!(check(&[path]).is_err(), "{path}");
        }
        assert!(check(&["Docs/a", "docs/b"]).is_err());
        assert!(check(&["a.txt", "A.txt"]).is_err());
    }
    #[test]
    fn accepts_normal_paths_and_identical_snapshots() {
        check(&[
            "Docs",
            "Docs/a.txt",
            "Docs/a.txt",
            "COM10.txt",
            "Résumé.txt",
        ])
        .unwrap();
    }
}
