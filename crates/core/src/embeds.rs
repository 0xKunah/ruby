//! Files embedded in notes: resolving `![[target]]` and naming pasted images.

use std::path::{Path, PathBuf};

use crate::fs_ops::unique_path;
use crate::vault::natural_cmp;

/// Finds the file an embed points to: relative to the note's folder, then to the vault root,
/// then any vault file with that name (case-insensitive; first in natural path order).
pub fn resolve_embed(vault: &Path, note_dir: &Path, target: &str) -> Option<PathBuf> {
	let target = target.trim();
	if target.is_empty() {
		return None;
	}
	for base in [note_dir, vault] {
		let candidate = base.join(target);
		if candidate.is_file() {
			return Some(candidate);
		}
	}
	let name = Path::new(target)
		.file_name()?
		.to_string_lossy()
		.to_lowercase();
	let mut found = Vec::new();
	find_by_name(vault, &name, &mut found);
	found.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
	found.into_iter().next()
}

/// Finds the drawing an embed points to; `.excalidraw` may be left out of the target.
pub fn resolve_drawing(vault: &Path, note_dir: &Path, target: &str) -> Option<PathBuf> {
	let target = target.trim();
	if target.is_empty() {
		return None;
	}
	let cut = target.len().saturating_sub(".excalidraw".len());
	let has_extension =
		target.is_char_boundary(cut) && target[cut..].eq_ignore_ascii_case(".excalidraw");
	if has_extension {
		resolve_embed(vault, note_dir, target)
	} else if Path::new(target).extension().is_none() {
		resolve_embed(vault, note_dir, &format!("{target}.excalidraw"))
	} else {
		None
	}
}

fn find_by_name(dir: &Path, name: &str, found: &mut Vec<PathBuf>) {
	let Ok(entries) = std::fs::read_dir(dir) else {
		return;
	};
	for entry in entries.flatten() {
		let file_name = entry.file_name().to_string_lossy().into_owned();
		if file_name.starts_with('.') {
			continue;
		}
		let path = entry.path();
		match entry.file_type() {
			Ok(t) if t.is_dir() => find_by_name(&path, name, found),
			Ok(_) if file_name.to_lowercase() == name => found.push(path),
			_ => {}
		}
	}
}

/// `Pasted image <stamp>.<ext>` in `dir`, numbered if the name is taken.
pub fn pasted_image_path(dir: &Path, stamp: &str, ext: &str) -> PathBuf {
	unique_path(dir, &format!("Pasted image {stamp}"), Some(ext))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn resolution_order() {
		let d = tempfile::tempdir().unwrap();
		let v = d.path();
		std::fs::create_dir_all(v.join("notes")).unwrap();
		std::fs::create_dir_all(v.join("attachments/deep")).unwrap();
		std::fs::write(v.join("img.png"), "root").unwrap();
		std::fs::write(v.join("notes/img.png"), "local").unwrap();
		std::fs::write(v.join("attachments/deep/Photo.JPG"), "nested").unwrap();
		assert_eq!(
			resolve_embed(v, &v.join("notes"), "img.png"),
			Some(v.join("notes").join("img.png"))
		);
		assert_eq!(resolve_embed(v, v, "img.png"), Some(v.join("img.png")));
		assert_eq!(
			resolve_embed(v, &v.join("notes"), "photo.jpg"),
			Some(v.join("attachments").join("deep").join("Photo.JPG"))
		);
		assert_eq!(
			resolve_embed(v, v, "attachments/deep/Photo.JPG"),
			Some(v.join("attachments/deep/Photo.JPG"))
		);
		assert_eq!(resolve_embed(v, v, "missing.png"), None);
	}

	#[test]
	fn pasted_images_never_overwrite() {
		let d = tempfile::tempdir().unwrap();
		let first = pasted_image_path(d.path(), "20261110143000", "png");
		assert_eq!(first, d.path().join("Pasted image 20261110143000.png"));
		std::fs::write(&first, "x").unwrap();
		assert_eq!(
			pasted_image_path(d.path(), "20261110143000", "png"),
			d.path().join("Pasted image 20261110143000 1.png")
		);
	}

	#[test]
	fn drawings_resolve_with_or_without_extension() {
		let d = tempfile::tempdir().unwrap();
		let v = d.path();
		std::fs::create_dir_all(v.join("sub")).unwrap();
		std::fs::write(v.join("sub/Plan.excalidraw"), "{}").unwrap();
		std::fs::write(v.join("Note.md"), "n").unwrap();
		let plan = Some(v.join("sub").join("Plan.excalidraw"));
		assert_eq!(resolve_drawing(v, v, "Plan"), plan);
		assert_eq!(resolve_drawing(v, v, "plan.excalidraw"), plan);
		assert_eq!(
			resolve_drawing(v, v, "sub/Plan"),
			Some(v.join("sub/Plan.excalidraw"))
		);
		assert_eq!(
			resolve_drawing(v, v, "Note"),
			None,
			"a note is not a drawing"
		);
		assert_eq!(resolve_drawing(v, v, "Note.md"), None);
		assert_eq!(resolve_drawing(v, v, ""), None);
	}
}
