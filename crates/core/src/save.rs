//! Crash-safe file writes and conflict copy naming.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Writes `bytes` to a temp file next to `path`, flushes it to disk, then renames it over `path`.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
	let file_name = path.file_name().ok_or_else(|| {
		std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name")
	})?;
	let mut tmp_name = std::ffi::OsString::from(".");
	tmp_name.push(file_name);
	tmp_name.push(".ruby-tmp");
	let tmp = path.with_file_name(tmp_name);
	let result = (|| {
		let mut file = std::fs::File::create(&tmp)?;
		file.write_all(bytes)?;
		file.sync_all()?;
		drop(file);
		std::fs::rename(&tmp, path)
	})();
	if result.is_err() {
		let _ = std::fs::remove_file(&tmp);
	}
	result
}

/// `Note.md` + `conflict 2026-11-09 1430` -> `Note (conflict 2026-11-09 1430).md`, numbered when taken.
pub fn conflict_path(original: &Path, tag: &str) -> PathBuf {
	let stem = original
		.file_stem()
		.map(|s| s.to_string_lossy().into_owned())
		.unwrap_or_default();
	let ext = original
		.extension()
		.map(|e| format!(".{}", e.to_string_lossy()))
		.unwrap_or_default();
	let base = format!("{stem} ({tag})");
	let mut candidate = original.with_file_name(format!("{base}{ext}"));
	let mut n = 2;
	while candidate.exists() {
		candidate = original.with_file_name(format!("{base} {n}{ext}"));
		n += 1;
	}
	candidate
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn atomic_write_creates_and_replaces() {
		let dir = tempfile::tempdir().unwrap();
		let p = dir.path().join("n.md");
		atomic_write(&p, b"one").unwrap();
		atomic_write(&p, b"two").unwrap();
		assert_eq!(std::fs::read(&p).unwrap(), b"two");
		let names: Vec<_> = std::fs::read_dir(dir.path())
			.unwrap()
			.map(|e| e.unwrap().file_name())
			.collect();
		assert_eq!(names.len(), 1, "temp file left behind: {names:?}");
	}

	#[test]
	fn atomic_write_missing_parent_errors() {
		let dir = tempfile::tempdir().unwrap();
		assert!(atomic_write(&dir.path().join("nope/n.md"), b"x").is_err());
	}

	#[test]
	fn conflict_path_numbers_when_taken() {
		let dir = tempfile::tempdir().unwrap();
		let original = dir.path().join("Note.md");
		let first = conflict_path(&original, "conflit 2026-11-09 1430");
		assert_eq!(first, dir.path().join("Note (conflit 2026-11-09 1430).md"));
		std::fs::write(&first, "x").unwrap();
		assert_eq!(
			conflict_path(&original, "conflit 2026-11-09 1430"),
			dir.path().join("Note (conflit 2026-11-09 1430) 2.md")
		);
	}
}
