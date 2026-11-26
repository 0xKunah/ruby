//! Vault folder scan and the flattened rows shown by the file tree.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
	Folder,
	Note,
	Drawing,
	Image,
	Other,
}

#[derive(Debug, Clone)]
pub struct Node {
	pub rel_path: PathBuf,
	pub name: String,
	pub kind: NodeKind,
	pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TreeRow {
	pub rel_path: PathBuf,
	pub name: String,
	pub kind: NodeKind,
	pub depth: usize,
	pub expanded: bool,
}

const IMAGE_EXTS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"];

pub fn kind_for_path(path: &Path) -> NodeKind {
	let ext = path
		.extension()
		.map(|e| e.to_string_lossy().to_lowercase())
		.unwrap_or_default();
	match ext.as_str() {
		"md" => NodeKind::Note,
		"excalidraw" => NodeKind::Drawing,
		e if IMAGE_EXTS.contains(&e) => NodeKind::Image,
		_ => NodeKind::Other,
	}
}

pub fn display_name(file_name: &str, kind: NodeKind) -> String {
	let suffix = match kind {
		NodeKind::Note => ".md",
		NodeKind::Drawing => ".excalidraw",
		_ => return file_name.to_string(),
	};
	let cut = file_name.len().saturating_sub(suffix.len());
	if file_name.is_char_boundary(cut) && file_name[cut..].eq_ignore_ascii_case(suffix) {
		file_name[..cut].to_string()
	} else {
		file_name.to_string()
	}
}

/// Lowercase without accents: `Été` -> `ete`, so accented names sort with their base letter.
fn fold(s: &str) -> String {
	s.nfd()
		.filter(|c| !is_combining_mark(*c))
		.flat_map(char::to_lowercase)
		.collect()
}

/// Case- and accent-insensitive comparison where runs of ASCII digits compare by numeric value.
/// Names equal after folding are ordered by their lowercase form (`ete` before `été`).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
	natural_cmp_folded(&fold(a), &fold(b)).then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
}

fn natural_cmp_folded(a: &str, b: &str) -> Ordering {
	let (mut ia, mut ib) = (a.chars().peekable(), b.chars().peekable());
	loop {
		match (ia.peek().copied(), ib.peek().copied()) {
			(None, None) => return Ordering::Equal,
			(None, Some(_)) => return Ordering::Less,
			(Some(_), None) => return Ordering::Greater,
			(Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
				let na = take_digits(&mut ia);
				let nb = take_digits(&mut ib);
				let ord = na
					.trim_start_matches('0')
					.len()
					.cmp(&nb.trim_start_matches('0').len())
					.then_with(|| na.trim_start_matches('0').cmp(nb.trim_start_matches('0')));
				if ord != Ordering::Equal {
					return ord;
				}
			}
			(Some(ca), Some(cb)) => {
				if ca != cb {
					return ca.cmp(&cb);
				}
				ia.next();
				ib.next();
			}
		}
	}
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
	let mut s = String::new();
	while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
		s.push(c);
		it.next();
	}
	s
}

/// Recursively lists the vault: dot-entries skipped, folders first, natural order by display name.
pub fn scan_vault(root: &Path) -> std::io::Result<Vec<Node>> {
	scan_dir(root, Path::new(""))
}

fn scan_dir(root: &Path, rel: &Path) -> std::io::Result<Vec<Node>> {
	let mut folders = Vec::new();
	let mut files = Vec::new();
	for entry in std::fs::read_dir(root.join(rel))? {
		// An entry that cannot be read (locked, deleted during the scan) is skipped, not fatal.
		let Ok(entry) = entry else { continue };
		let file_name = entry.file_name().to_string_lossy().into_owned();
		if file_name.starts_with('.') {
			continue;
		}
		let rel_path = rel.join(&file_name);
		let Ok(file_type) = entry.file_type() else {
			continue;
		};
		if file_type.is_dir() {
			let children = scan_dir(root, &rel_path).unwrap_or_default();
			folders.push(Node {
				rel_path,
				name: file_name,
				kind: NodeKind::Folder,
				children,
			});
		} else {
			let kind = kind_for_path(&rel_path);
			files.push(Node {
				name: display_name(&file_name, kind),
				rel_path,
				kind,
				children: Vec::new(),
			});
		}
	}
	folders.sort_by(|a, b| natural_cmp(&a.name, &b.name));
	files.sort_by(|a, b| natural_cmp(&a.name, &b.name));
	folders.extend(files);
	Ok(folders)
}

pub fn flatten_visible(nodes: &[Node], expanded: &HashSet<PathBuf>) -> Vec<TreeRow> {
	let mut rows = Vec::new();
	push_rows(nodes, expanded, 0, &mut rows);
	rows
}

fn push_rows(nodes: &[Node], expanded: &HashSet<PathBuf>, depth: usize, rows: &mut Vec<TreeRow>) {
	for node in nodes {
		let is_open = node.kind == NodeKind::Folder && expanded.contains(&node.rel_path);
		rows.push(TreeRow {
			rel_path: node.rel_path.clone(),
			name: node.name.clone(),
			kind: node.kind,
			depth,
			expanded: is_open,
		});
		if is_open {
			push_rows(&node.children, expanded, depth + 1, rows);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::HashSet;
	use std::path::PathBuf;

	#[test]
	fn natural_order() {
		use std::cmp::Ordering::*;
		assert_eq!(natural_cmp("Note 2", "Note 10"), Less);
		assert_eq!(natural_cmp("apple", "Banana"), Less);
		assert_eq!(natural_cmp("b", "B"), Equal);
	}

	#[test]
	fn scan_sorts_hides_and_names() {
		let d = tempfile::tempdir().unwrap();
		let r = d.path();
		for f in [
			"b.md",
			"a.excalidraw",
			"img.PNG",
			"Note 10.md",
			"Note 2.md",
			".hidden.md",
		] {
			std::fs::write(r.join(f), "").unwrap();
		}
		std::fs::create_dir_all(r.join("Projets")).unwrap();
		std::fs::write(r.join("Projets/x.md"), "").unwrap();
		std::fs::create_dir_all(r.join(".obsidian")).unwrap();
		let nodes = scan_vault(r).unwrap();
		let names: Vec<_> = nodes.iter().map(|n| n.name.as_str()).collect();
		assert_eq!(names, ["Projets", "a", "b", "img.PNG", "Note 2", "Note 10"]);
		assert_eq!(nodes[0].kind, NodeKind::Folder);
		assert_eq!(
			nodes[0].children[0].rel_path,
			PathBuf::from("Projets").join("x.md")
		);
		assert_eq!(nodes[1].kind, NodeKind::Drawing);
		assert_eq!(nodes[3].kind, NodeKind::Image);
	}

	#[test]
	fn flatten_respects_expanded() {
		let d = tempfile::tempdir().unwrap();
		std::fs::create_dir_all(d.path().join("F/G")).unwrap();
		std::fs::write(d.path().join("F/G/n.md"), "").unwrap();
		let nodes = scan_vault(d.path()).unwrap();
		assert_eq!(flatten_visible(&nodes, &HashSet::new()).len(), 1);
		let expanded: HashSet<PathBuf> = [PathBuf::from("F"), PathBuf::from("F").join("G")].into();
		let rows = flatten_visible(&nodes, &expanded);
		assert_eq!(rows.iter().map(|r| r.depth).collect::<Vec<_>>(), [0, 1, 2]);
		assert!(rows[0].expanded);
		assert_eq!(rows[2].name, "n");
	}

	#[test]
	fn accents_sort_with_their_base_letter() {
		use std::cmp::Ordering::*;
		assert_eq!(natural_cmp("Été", "Zèbre"), Less);
		assert_eq!(natural_cmp("école", "Ecole 2"), Less);
		assert_ne!(natural_cmp("été", "ete"), Equal);
	}

	#[cfg(windows)]
	#[test]
	fn unreadable_subfolder_does_not_empty_the_tree() {
		let d = tempfile::tempdir().unwrap();
		std::fs::write(d.path().join("a.md"), "").unwrap();
		let locked = d.path().join("locked");
		std::fs::create_dir(&locked).unwrap();
		let user = std::env::var("USERNAME").unwrap();
		let deny = |args: &[&str]| {
			let ok = std::process::Command::new("icacls")
				.arg(&locked)
				.args(args)
				.output()
				.unwrap()
				.status
				.success();
			assert!(ok, "icacls {args:?}");
		};
		deny(&["/deny", &format!("{user}:(RD)")]);
		let nodes = scan_vault(d.path());
		deny(&["/remove:d", &user]);
		let nodes = nodes.unwrap();
		assert!(nodes.iter().any(|n| n.name == "a"), "{nodes:?}");
	}
}
