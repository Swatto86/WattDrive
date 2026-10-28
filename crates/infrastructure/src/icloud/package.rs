//! Validate package ZIP structure and each entry's CRC without extracting it.
//! Keeping the original archive preserves Apple metadata and symbolic links.
use std::io::{self, Read};
use std::path::Path;
use wattdrive_domain::DriveError;

pub fn validate(path: &Path) -> Result<(), DriveError> {
    let file = std::fs::File::open(path)?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut archive = zip::ZipArchive::new(file)?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let expected = entry.size();
            let limit = expected
                .checked_add(1)
                .ok_or("package entry size overflow")?;
            let actual = io::copy(&mut (&mut entry).take(limit), &mut io::sink())?;
            if actual != expected {
                return Err("package entry size mismatch".into());
            }
        }
        Ok(())
    })();
    result.map_err(|_| {
        DriveError::Other(
            "Invalid or incomplete iCloud package ZIP; original preserved, retry sync".into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn validates_crc_and_rejects_truncated_archives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        writer
            .start_file(
                "Project/file.txt",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"synthetic package content").unwrap();
        writer.finish().unwrap();
        assert!(validate(&path).is_ok());
        let original = std::fs::read(&path).unwrap();
        let mut corrupt = original.clone();
        let offset = corrupt
            .windows(9)
            .position(|bytes| bytes == b"synthetic")
            .unwrap();
        corrupt[offset] ^= 1;
        std::fs::write(&path, &corrupt).unwrap();
        assert!(validate(&path).is_err());
        std::fs::write(&path, &original[..original.len() / 2]).unwrap();
        assert!(validate(&path).is_err());
    }
}
