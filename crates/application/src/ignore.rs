//! Names that never sync: editor lock/swap files, half-written downloads,
//! desktop metadata, and WattDrive's own partial-transfer files.

/// Prefix of the temp file a download is written to before its atomic rename.
pub const PARTIAL_PREFIX: &str = ".wattdrive-part-";

pub fn is_ignored_name(name: &str) -> bool {
    const EXACT: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini", ".directory"];
    const PREFIXES: &[&str] = &[".~lock.", ".#", PARTIAL_PREFIX, ".wattdrive"];
    const SUFFIXES: &[&str] = &[
        ".tmp",
        ".temp",
        ".part",
        ".crdownload",
        ".swp",
        ".swx",
        "~",
        ".icloud",
    ];
    EXACT.contains(&name)
        || PREFIXES.iter().any(|p| name.starts_with(p))
        || SUFFIXES.iter().any(|s| name.ends_with(s))
        || (name.starts_with('.') && name.ends_with(".swp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_junk_is_ignored_and_real_files_are_not() {
        for junk in [
            ".DS_Store",
            "Thumbs.db",
            ".~lock.report.odt#",
            ".#notes.txt",
            ".wattdrive-part-abc",
            "download.crdownload",
            "essay.docx.tmp",
            "notes.txt~",
            ".todo.md.swp",
            ".Report.pdf.icloud",
        ] {
            assert!(is_ignored_name(junk), "{junk} should be ignored");
        }
        for real in [
            "report.odt",
            ".bashrc",
            "todo.md",
            "Photo (conflict).jpg",
            "a.tmp.txt",
        ] {
            assert!(!is_ignored_name(real), "{real} should sync");
        }
    }
}
