//! File tree operations: create, rename, move, delete to the Recycle Bin.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::vault::{NodeKind, kind_for_path};

pub const EMPTY_DRAWING: &str = r##"{"type":"excalidraw","version":2,"source":"ruby","elements":[],"appState":{"viewBackgroundColor":"#ffffff"},"files":{}}"##;

#[derive(Debug)]
pub enum FsError {
	InvalidName,
	AlreadyExists,
	IntoItself,
	Io(std::io::Error),
}

impl From<std::io::Error> for FsError {
	fn from(e: std::io::Error) -> Self {
		FsError::Io(e)
	}
}

impl fmt::Display for FsError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			FsError::InvalidName => write!(f, "invalid name"),
			FsError::AlreadyExists => write!(f, "a file with this name already exists"),
			FsError::IntoItself => write!(f, "cannot move a folder into itself"),
			FsError::Io(e) => write!(f, "disk error: {e}"),
		}
	}
}

impl std::error::Error for FsError {}

/// Windows device names (with or without an extension), including the superscript-digit and
/// console variants.
const RESERVED: [&str; 30] = [
	"CON",
	"PRN",
	"AUX",
	"NUL",
	"CONIN$",
	"CONOUT$",
	"COM1",
	"COM2",
	"COM3",
	"COM4",
	"COM5",
	"COM6",
	"COM7",
	"COM8",
	"COM9",
	"COM\u{b9}",
	"COM\u{b2}",
	"COM\u{b3}",
	"LPT1",
	"LPT2",
	"LPT3",
	"LPT4",
	"LPT5",
	"LPT6",
	"LPT7",
	"LPT8",
	"LPT9",
	"LPT\u{b9}",
	"LPT\u{b2}",
	"LPT\u{b3}",
];

/// Rejects names Windows cannot store or that the vault would hide.
pub fn validate_name(name: &str) -> Result<(), FsError> {
	let invalid = name.trim().is_empty()
		|| name
			.chars()
			.any(|c| c.is_control() || r#"\/:*?"<>|"#.contains(c))
		|| name.starts_with('.')
		|| name.ends_with('.')
		|| name.ends_with(' ');
	let stem = name
		.split('.')
		.next()
		.unwrap_or("")
		.trim_end()
		.to_ascii_uppercase();
	if invalid || RESERVED.contains(&stem.as_str()) {
		return Err(FsError::InvalidName);
	}
	Ok(())
}

/// `stem.ext`, then `stem 1.ext`, `stem 2.ext`, ... until the path is free.
pub fn unique_path(dir: &Path, stem: &str, ext: Option<&str>) -> PathBuf {
	let name = |n: usize| {
		let base = if n == 0 {
			stem.to_string()
		} else {
			format!("{stem} {n}")
		};
		match ext {
			Some(e) => format!("{base}.{e}"),
			None => base,
		}
	};
	(0..)
		.map(|n| dir.join(name(n)))
		.find(|p| !p.exists())
		.expect("unbounded search")
}

fn create_file(dir: &Path, stem: &str, ext: &str, content: &str) -> Result<PathBuf, FsError> {
	let path = unique_path(dir, stem, Some(ext));
	let mut file = std::fs::OpenOptions::new()
		.write(true)
		.create_new(true)
		.open(&path)?;
	file.write_all(content.as_bytes())?;
	file.sync_all()?;
	Ok(path)
}

/// New empty note `stem.md` (numbered if taken).
pub fn create_note(dir: &Path, stem: &str) -> Result<PathBuf, FsError> {
	create_file(dir, stem, "md", "")
}

/// New empty drawing `stem.excalidraw` (numbered if taken).
pub fn create_drawing(dir: &Path, stem: &str) -> Result<PathBuf, FsError> {
	create_file(dir, stem, "excalidraw", EMPTY_DRAWING)
}

/// New folder `name` (numbered if taken).
pub fn create_folder(dir: &Path, name: &str) -> Result<PathBuf, FsError> {
	let path = unique_path(dir, name, None);
	std::fs::create_dir(&path)?;
	Ok(path)
}

fn same_file(a: &Path, b: &Path) -> bool {
	match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
		(Ok(a), Ok(b)) => a == b,
		_ => false,
	}
}

/// Renames in place. For notes and drawings `new_name` is the display name; the extension is kept.
pub fn rename_entry(path: &Path, new_name: &str) -> Result<PathBuf, FsError> {
	validate_name(new_name)?;
	let kind = if path.is_dir() {
		NodeKind::Folder
	} else {
		kind_for_path(path)
	};
	let file_name = match kind {
		NodeKind::Note | NodeKind::Drawing => {
			let ext = path
				.extension()
				.map(|e| e.to_string_lossy().into_owned())
				.unwrap_or_default();
			let suffix = format!(".{ext}");
			if new_name.to_lowercase().ends_with(&suffix.to_lowercase()) {
				new_name.to_string()
			} else {
				format!("{new_name}{suffix}")
			}
		}
		_ => new_name.to_string(),
	};
	let target = path.with_file_name(&file_name);
	if target == path {
		return Ok(target);
	}
	if target.exists() {
		if !same_file(path, &target) {
			return Err(FsError::AlreadyExists);
		}
		// Case-only rename on a case-insensitive file system: go through a temp name.
		let tmp = unique_path(path.parent().unwrap_or(Path::new("")), ".ruby-rename", None);
		two_step_rename(path, &tmp, &target)?;
		return Ok(target);
	}
	rename_no_replace(path, &target).map_err(already_exists)?;
	Ok(target)
}

/// Moves a file or folder into `target_dir`, keeping its name.
pub fn move_entry(path: &Path, target_dir: &Path) -> Result<PathBuf, FsError> {
	let src = std::fs::canonicalize(path)?;
	let dst_dir = std::fs::canonicalize(target_dir)?;
	if dst_dir.starts_with(&src) {
		return Err(FsError::IntoItself);
	}
	let name = path.file_name().ok_or(FsError::InvalidName)?;
	let target = target_dir.join(name);
	if same_file(path, &target) {
		return Ok(target);
	}
	if target.exists() {
		return Err(FsError::AlreadyExists);
	}
	rename_no_replace(path, &target).map_err(already_exists)?;
	Ok(target)
}

/// A target that appeared after the check is still reported as a name clash.
fn already_exists(e: std::io::Error) -> FsError {
	if e.kind() == std::io::ErrorKind::AlreadyExists {
		FsError::AlreadyExists
	} else {
		FsError::Io(e)
	}
}

/// Renames `from` to `to` in one step that fails with `AlreadyExists` if `to` exists, so a file
/// created in between is never overwritten (`std::fs::rename` replaces on Windows).
#[cfg(windows)]
pub fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
	use std::os::windows::ffi::OsStrExt;
	use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
	let wide = |p: &Path| -> Vec<u16> {
		p.as_os_str()
			.encode_wide()
			.chain(std::iter::once(0))
			.collect()
	};
	let (from, to) = (wide(from), wide(to));
	// SAFETY: both buffers are NUL-terminated UTF-16 paths that outlive the call; flags 0 = no replace.
	let ok = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) };
	if ok == 0 {
		Err(std::io::Error::last_os_error())
	} else {
		Ok(())
	}
}

#[cfg(not(windows))]
pub fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
	if to.exists() {
		return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
	}
	std::fs::rename(from, to)
}

/// `src` -> `tmp` -> `dst`; if the second step fails, `src` is put back.
fn two_step_rename(src: &Path, tmp: &Path, dst: &Path) -> std::io::Result<()> {
	rename_no_replace(src, tmp)?;
	if let Err(e) = rename_no_replace(tmp, dst) {
		rename_no_replace(tmp, src).ok();
		return Err(e);
	}
	Ok(())
}

pub fn delete_to_trash(path: &Path) -> Result<(), FsError> {
	trash::delete(path).map_err(|e| FsError::Io(std::io::Error::other(e.to_string())))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn invalid_names() {
		for bad in [
			"", "  ", "a:b", "a?b", "con", "LPT1.md", "x.", "x ", ".x", "a/b",
		] {
			assert!(
				matches!(validate_name(bad), Err(FsError::InvalidName)),
				"{bad:?}"
			);
		}
		assert!(validate_name("Été 2026").is_ok());
	}

	#[test]
	fn create_note_numbers_untitled() {
		let d = tempfile::tempdir().unwrap();
		assert_eq!(
			create_note(d.path(), "Sans titre").unwrap(),
			d.path().join("Sans titre.md")
		);
		assert_eq!(
			create_note(d.path(), "Sans titre").unwrap(),
			d.path().join("Sans titre 1.md")
		);
		assert_eq!(
			create_drawing(d.path(), "Sans titre").unwrap(),
			d.path().join("Sans titre.excalidraw")
		);
		assert_eq!(
			create_note(d.path(), "Untitled").unwrap(),
			d.path().join("Untitled.md")
		);
		assert_eq!(
			create_folder(d.path(), "New folder").unwrap(),
			d.path().join("New folder")
		);
		assert_eq!(
			std::fs::read_to_string(d.path().join("Sans titre.excalidraw")).unwrap(),
			EMPTY_DRAWING
		);
	}

	#[test]
	fn rename_keeps_extension_and_refuses_clash() {
		let d = tempfile::tempdir().unwrap();
		let a = d.path().join("a.md");
		std::fs::write(&a, "A").unwrap();
		std::fs::write(d.path().join("b.md"), "B").unwrap();
		assert!(matches!(rename_entry(&a, "b"), Err(FsError::AlreadyExists)));
		assert_eq!(std::fs::read_to_string(d.path().join("b.md")).unwrap(), "B");
		assert_eq!(rename_entry(&a, "c").unwrap(), d.path().join("c.md"));
	}

	#[test]
	fn case_only_rename_works() {
		let d = tempfile::tempdir().unwrap();
		let p = d.path().join("todo.md");
		std::fs::write(&p, "x").unwrap();
		let new = rename_entry(&p, "Todo").unwrap();
		let names: Vec<_> = std::fs::read_dir(d.path())
			.unwrap()
			.map(|e| e.unwrap().file_name().into_string().unwrap())
			.collect();
		assert_eq!(names, ["Todo.md"]);
		assert_eq!(new, d.path().join("Todo.md"));
	}

	#[test]
	fn move_rules() {
		let d = tempfile::tempdir().unwrap();
		let f = d.path().join("F");
		std::fs::create_dir_all(f.join("G")).unwrap();
		assert!(matches!(
			move_entry(&f, &f.join("G")),
			Err(FsError::IntoItself)
		));
		std::fs::write(d.path().join("n.md"), "").unwrap();
		assert_eq!(
			move_entry(&d.path().join("n.md"), &f).unwrap(),
			f.join("n.md")
		);
	}

	#[test]
	fn more_reserved_names() {
		for bad in [
			"CONIN$",
			"conout$",
			"COM\u{b9}",
			"LPT\u{b2}.md",
			"com\u{b3}",
		] {
			assert!(
				matches!(validate_name(bad), Err(FsError::InvalidName)),
				"{bad:?}"
			);
		}
	}

	#[test]
	fn rename_no_replace_never_overwrites() {
		let d = tempfile::tempdir().unwrap();
		let (a, b) = (d.path().join("a.md"), d.path().join("b.md"));
		std::fs::write(&a, "A").unwrap();
		std::fs::write(&b, "B").unwrap();
		let err = rename_no_replace(&a, &b).unwrap_err();
		assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
		assert_eq!(std::fs::read_to_string(&b).unwrap(), "B");
		assert!(a.exists());
		std::fs::create_dir(d.path().join("F")).unwrap();
		rename_no_replace(&d.path().join("F"), &d.path().join("G")).unwrap();
		assert!(d.path().join("G").is_dir());
	}

	#[test]
	fn two_step_rename_rolls_back_on_failure() {
		let d = tempfile::tempdir().unwrap();
		let a = d.path().join("a.md");
		std::fs::write(&a, "A").unwrap();
		let tmp = d.path().join(".tmp-rename");
		let dst = d.path().join("missing").join("x.md");
		assert!(two_step_rename(&a, &tmp, &dst).is_err());
		assert_eq!(std::fs::read_to_string(&a).unwrap(), "A");
		assert!(!tmp.exists());
	}
}
