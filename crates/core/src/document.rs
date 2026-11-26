//! An open note on disk: load, save, and reaction to external changes.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::save::{atomic_write, conflict_path};
use crate::text_file::{FileFormat, LineEnding, decode, encode};

pub const AUTOSAVE_DELAY_MS: u64 = 500;

#[derive(Debug, PartialEq)]
pub enum ExternalOutcome {
	Unchanged,
	Reloaded(String),
	Conflict { copy: PathBuf },
	Closed,
	KeptDeleted,
}

#[derive(Debug)]
pub struct OpenDocument {
	pub path: PathBuf,
	pub format: FileFormat,
	pub read_only: bool,
	/// Hash of the bytes on disk at the last load, save or reload.
	last_synced: u64,
	/// Hash of the editor text at that same moment (an edit that restores it is not a change).
	synced_text: u64,
	dirty: bool,
}

fn hash_bytes(bytes: &[u8]) -> u64 {
	let mut h = DefaultHasher::new();
	bytes.hash(&mut h);
	h.finish()
}

/// Decodes bytes; non-UTF-8 content is shown lossily and flagged read-only.
fn decode_or_lossy(bytes: &[u8]) -> (String, FileFormat, bool) {
	match decode(bytes) {
		Ok((text, format)) => (text, format, false),
		Err(_) => (
			String::from_utf8_lossy(bytes).into_owned(),
			FileFormat {
				line_ending: LineEnding::Lf,
				bom: false,
			},
			true,
		),
	}
}

impl OpenDocument {
	pub fn load(path: &Path) -> std::io::Result<(OpenDocument, String)> {
		let bytes = std::fs::read(path)?;
		let (text, format, read_only) = decode_or_lossy(&bytes);
		let doc = OpenDocument {
			path: path.to_path_buf(),
			format,
			read_only,
			last_synced: hash_bytes(&bytes),
			synced_text: hash_bytes(text.as_bytes()),
			dirty: false,
		};
		Ok((doc, text))
	}

	pub fn mark_dirty(&mut self) {
		self.dirty = true;
	}

	pub fn is_dirty(&self) -> bool {
		self.dirty
	}

	pub fn set_path(&mut self, path: PathBuf) {
		self.path = path;
	}

	/// Forgets unsaved edits (the file was deleted on purpose).
	pub fn discard_changes(&mut self) {
		self.dirty = false;
	}

	/// Writes `text` when it differs from what was last synced; never touches read-only or clean
	/// documents. If the file changed on disk since then (and the watcher has not said so yet), the
	/// disk version is first kept as a conflict copy named with `conflict_tag`, whose path is returned.
	pub fn save(&mut self, text: &str, conflict_tag: &str) -> std::io::Result<Option<PathBuf>> {
		if !self.dirty || self.read_only {
			return Ok(None);
		}
		if hash_bytes(text.as_bytes()) == self.synced_text {
			self.dirty = false;
			return Ok(None);
		}
		let copy = match std::fs::read(&self.path) {
			Ok(disk) if hash_bytes(&disk) != self.last_synced => {
				if !self.read_only && decode(&disk).is_ok_and(|(t, _)| t == text) {
					// The disk already holds this text: nothing to save, nothing to keep.
					self.last_synced = hash_bytes(&disk);
					self.synced_text = hash_bytes(text.as_bytes());
					self.dirty = false;
					return Ok(None);
				}
				let copy = conflict_path(&self.path, conflict_tag);
				atomic_write(&copy, &disk)?;
				Some(copy)
			}
			Ok(_) => None,
			Err(e) if e.kind() == ErrorKind::NotFound => None,
			Err(e) => return Err(e),
		};
		self.write(text)?;
		Ok(copy)
	}

	fn write(&mut self, text: &str) -> std::io::Result<()> {
		let bytes = encode(text, self.format);
		atomic_write(&self.path, &bytes)?;
		self.last_synced = hash_bytes(&bytes);
		self.synced_text = hash_bytes(text.as_bytes());
		self.dirty = false;
		Ok(())
	}

	/// Reacts to a change of the file on disk. `conflict_tag` names a conflict copy if one is needed.
	pub fn on_disk_changed(
		&mut self,
		editor_text: &str,
		conflict_tag: &str,
	) -> std::io::Result<ExternalOutcome> {
		let bytes = match std::fs::read(&self.path) {
			Ok(b) => b,
			Err(e) if e.kind() == ErrorKind::NotFound => {
				return Ok(if self.dirty {
					ExternalOutcome::KeptDeleted
				} else {
					ExternalOutcome::Closed
				});
			}
			Err(e) => return Err(e),
		};
		let disk_hash = hash_bytes(&bytes);
		if disk_hash == self.last_synced {
			return Ok(ExternalOutcome::Unchanged);
		}
		if !self.dirty {
			let (text, format, read_only) = decode_or_lossy(&bytes);
			self.format = format;
			self.read_only = read_only;
			self.last_synced = disk_hash;
			self.synced_text = hash_bytes(text.as_bytes());
			return Ok(ExternalOutcome::Reloaded(text));
		}
		if decode(&bytes).is_ok_and(|(t, _)| t == editor_text) {
			self.last_synced = disk_hash;
			self.synced_text = hash_bytes(editor_text.as_bytes());
			self.dirty = false;
			return Ok(ExternalOutcome::Unchanged);
		}
		let copy = conflict_path(&self.path, conflict_tag);
		atomic_write(&copy, &bytes)?;
		// That disk version is kept now: later events about it must not copy it again.
		self.last_synced = disk_hash;
		// If this write fails the note stays dirty; the next save retries and reports the error.
		self.write(editor_text).ok();
		Ok(ExternalOutcome::Conflict { copy })
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::path::PathBuf;

	fn setup(bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
		let dir = tempfile::tempdir().unwrap();
		let p = dir.path().join("n.md");
		std::fs::write(&p, bytes).unwrap();
		(dir, p)
	}

	#[test]
	fn save_preserves_crlf() {
		let (_d, p) = setup(b"a\r\nb\r\n");
		let (mut doc, text) = OpenDocument::load(&p).unwrap();
		assert_eq!(text, "a\nb\n");
		doc.mark_dirty();
		doc.save("a\nb\nc\n", "t").unwrap();
		assert_eq!(std::fs::read(&p).unwrap(), b"a\r\nb\r\nc\r\n");
	}

	#[test]
	fn save_without_edit_leaves_file_untouched() {
		let (_d, p) = setup(b"a\r\nb\n");
		let (mut doc, text) = OpenDocument::load(&p).unwrap();
		doc.save(&text, "t").unwrap();
		assert_eq!(std::fs::read(&p).unwrap(), b"a\r\nb\n");
	}

	#[test]
	fn own_write_is_unchanged() {
		let (_d, p) = setup(b"a");
		let (mut doc, _) = OpenDocument::load(&p).unwrap();
		doc.mark_dirty();
		doc.save("b", "t").unwrap();
		assert_eq!(
			doc.on_disk_changed("b", "conflit 2026-11-09 1430").unwrap(),
			ExternalOutcome::Unchanged
		);
	}

	#[test]
	fn clean_doc_reloads() {
		let (_d, p) = setup(b"a");
		let (mut doc, _) = OpenDocument::load(&p).unwrap();
		std::fs::write(&p, b"theirs").unwrap();
		assert_eq!(
			doc.on_disk_changed("a", "conflit 2026-11-09 1430").unwrap(),
			ExternalOutcome::Reloaded("theirs".into())
		);
	}

	#[test]
	fn dirty_doc_conflict_keeps_both() {
		let (d, p) = setup(b"a");
		let (mut doc, _) = OpenDocument::load(&p).unwrap();
		doc.mark_dirty();
		std::fs::write(&p, b"theirs").unwrap();
		let copy = d.path().join("n (conflit 2026-11-09 1430).md");
		assert_eq!(
			doc.on_disk_changed("mine", "conflit 2026-11-09 1430")
				.unwrap(),
			ExternalOutcome::Conflict { copy: copy.clone() }
		);
		assert_eq!(std::fs::read(&copy).unwrap(), b"theirs");
		assert_eq!(std::fs::read(&p).unwrap(), b"mine");
	}

	#[test]
	fn deletion_closes_clean_and_keeps_dirty() {
		let (_d, p) = setup(b"a");
		let (mut clean, _) = OpenDocument::load(&p).unwrap();
		let (mut dirty, _) = OpenDocument::load(&p).unwrap();
		dirty.mark_dirty();
		std::fs::remove_file(&p).unwrap();
		assert_eq!(
			clean.on_disk_changed("a", "s").unwrap(),
			ExternalOutcome::Closed
		);
		assert_eq!(
			dirty.on_disk_changed("x", "s").unwrap(),
			ExternalOutcome::KeptDeleted
		);
		dirty.save("x", "t").unwrap();
		assert_eq!(std::fs::read(&p).unwrap(), b"x");
	}

	#[test]
	fn non_utf8_is_read_only_and_never_saved() {
		let (_d, p) = setup(b"caf\xE9");
		let (mut doc, _) = OpenDocument::load(&p).unwrap();
		assert!(doc.read_only);
		doc.mark_dirty();
		doc.save("changed", "t").unwrap();
		assert_eq!(std::fs::read(&p).unwrap(), b"caf\xE9");
	}

	#[test]
	fn save_keeps_an_unreported_external_change() {
		let (d, p) = setup(b"base");
		let (mut doc, _) = OpenDocument::load(&p).unwrap();
		doc.mark_dirty();
		std::fs::write(&p, b"theirs").unwrap();
		let copy = doc.save("mine", "conflit s").unwrap();
		assert_eq!(copy, Some(d.path().join("n (conflit s).md")));
		assert_eq!(
			std::fs::read(d.path().join("n (conflit s).md")).unwrap(),
			b"theirs"
		);
		assert_eq!(std::fs::read(&p).unwrap(), b"mine");
		assert_eq!(
			doc.on_disk_changed("mine", "x").unwrap(),
			ExternalOutcome::Unchanged
		);
	}

	#[test]
	fn saving_unchanged_text_never_rewrites() {
		let (_d, p) = setup(b"a\r\nb\nc");
		let (mut doc, text) = OpenDocument::load(&p).unwrap();
		doc.mark_dirty();
		assert_eq!(doc.save(&text, "t").unwrap(), None);
		assert_eq!(std::fs::read(&p).unwrap(), b"a\r\nb\nc");
		assert!(!doc.is_dirty());
	}

	fn set_read_only(path: &std::path::Path, read_only: bool) {
		let mut perms = std::fs::metadata(path).unwrap().permissions();
		perms.set_readonly(read_only);
		std::fs::set_permissions(path, perms).unwrap();
	}

	#[test]
	fn identical_external_change_is_not_a_conflict() {
		let (d, p) = setup(b"a");
		let (mut doc, _) = OpenDocument::load(&p).unwrap();
		doc.mark_dirty();
		std::fs::write(&p, b"same").unwrap();
		assert_eq!(
			doc.on_disk_changed("same", "c").unwrap(),
			ExternalOutcome::Unchanged
		);
		assert!(!doc.is_dirty());
		let (mut doc2, _) = OpenDocument::load(&p).unwrap();
		doc2.mark_dirty();
		std::fs::write(&p, b"other").unwrap();
		assert_eq!(doc2.save("other", "c").unwrap(), None);
		assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
	}

	#[test]
	fn failed_write_after_a_conflict_copy_does_not_copy_twice() {
		let (d, p) = setup(b"a");
		let (mut doc, _) = OpenDocument::load(&p).unwrap();
		doc.mark_dirty();
		std::fs::write(&p, b"theirs").unwrap();
		set_read_only(&p, true);
		let first = doc.on_disk_changed("mine", "c1");
		let second = doc.on_disk_changed("mine", "c2");
		let dirty = doc.is_dirty();
		set_read_only(&p, false);
		assert!(
			matches!(first, Ok(ExternalOutcome::Conflict { .. })),
			"{first:?}"
		);
		assert_eq!(second.unwrap(), ExternalOutcome::Unchanged);
		assert!(dirty);
		assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 2);
	}
}
