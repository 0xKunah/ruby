//! Links between notes: finding them in a note's text, resolving their target in the vault
//! and rewriting them when a file is renamed or moved.
//!
//! Vault paths are relative to the vault root. They are compared with `/` separators,
//! ignoring case.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::markdown::percent_decode;
use crate::vault::{Node, NodeKind, natural_cmp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
	/// `[[target]]`
	Wiki,
	/// `[text](path)` to a file.
	Markdown,
	/// `[text](https://...)`
	Url,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
	/// The whole link, with its leading `!` for an embed.
	pub range: Range<usize>,
	/// The file part of the link: no `#heading`, no `|alias`, no `<>`, no title.
	pub target: Range<usize>,
	pub embed: bool,
	pub kind: LinkKind,
}

/// What a `Ctrl+click` opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
	/// A vault file, as written in the link (decoded).
	File(String),
	Url(String),
}

/// A path with `/` separators.
pub fn slash(path: &Path) -> String {
	path.to_string_lossy().replace('\\', "/")
}

fn same_path(a: &Path, b: &Path) -> bool {
	slash(a).to_lowercase() == slash(b).to_lowercase()
}

/// Relative paths of every file of the vault tree.
pub fn vault_files(nodes: &[Node]) -> Vec<PathBuf> {
	let mut out = Vec::new();
	fn walk(nodes: &[Node], out: &mut Vec<PathBuf>) {
		for node in nodes {
			if node.kind == NodeKind::Folder {
				walk(&node.children, out);
			} else {
				out.push(node.rel_path.clone());
			}
		}
	}
	walk(nodes, &mut out);
	out
}

/// The fence a line opens or closes: its character and length, and what follows it.
fn fence_of(line: &str) -> Option<(char, usize, &str)> {
	let content = line.trim_start_matches(|c: char| c == '>' || c.is_whitespace());
	let c = content.chars().next().filter(|c| matches!(c, '`' | '~'))?;
	let len = content.chars().take_while(|x| *x == c).count();
	(len >= 3).then(|| (c, len, &content[len..]))
}

/// Links to files in `text` (wikilinks and Markdown links), outside fenced code and `$$`
/// math blocks.
pub fn parse_links(text: &str) -> Vec<Link> {
	let mut out = Vec::new();
	// The open fence (a closing one has the same character and is at least as long).
	let mut fence: Option<(char, usize)> = None;
	let mut math = false;
	let mut start = 0;
	for line in text.split('\n') {
		match (fence, fence_of(line)) {
			(Some((c, len)), Some((c2, len2, rest)))
				if c == c2 && len2 >= len && rest.trim().is_empty() =>
			{
				fence = None;
			}
			(Some(_), _) => {}
			// A backtick fence has no backtick in its info string (else it is inline code).
			(None, Some((c, len, rest))) if !math && !(c == '`' && rest.contains('`')) => {
				fence = Some((c, len));
			}
			(None, _) if line.trim() == "$$" => math = !math,
			(None, _) if !math => scan_line(line, start, &mut out),
			(None, _) => {}
		}
		start += line.len() + 1;
	}
	out.retain(|l| l.kind != LinkKind::Url);
	out
}

/// Trims `range` of `line` and shifts it by `base`.
fn trimmed(line: &str, range: Range<usize>, base: usize) -> Range<usize> {
	let s = &line[range.clone()];
	let lead = s.len() - s.trim_start().len();
	base + range.start + lead..base + range.start + lead + s.trim().len()
}

fn scan_line(line: &str, base: usize, out: &mut Vec<Link>) {
	let bytes = line.as_bytes();
	let mut i = 0;
	while i < bytes.len() {
		match bytes[i] {
			b'`' => {
				let run = bytes[i..].iter().take_while(|b| **b == b'`').count();
				let fence = &line[i..i + run];
				// A closed code span is skipped whole.
				i += run + line[i + run..].find(fence).map_or(0, |close| close + run);
			}
			b'[' => {
				let embed = i > 0 && bytes[i - 1] == b'!';
				let link_start = base + i - usize::from(embed);
				if bytes.get(i + 1) == Some(&b'[') {
					let Some(close) = line[i + 2..].find("]]").map(|c| i + 2 + c) else {
						i += 2;
						continue;
					};
					let inner = &line[i + 2..close];
					let mut end = inner.find(['|', '#']).unwrap_or(inner.len());
					// `\\|`: the alias pipe escaped inside a table.
					if inner[end..].starts_with('|') && inner[..end].ends_with('\\') {
						end -= 1;
					}
					let target = trimmed(line, i + 2..i + 2 + end, base);
					if !target.is_empty() {
						out.push(Link {
							range: link_start..base + close + 2,
							target,
							embed,
							kind: LinkKind::Wiki,
						});
					}
					i = close + 2;
				} else if let Some((link, end)) = markdown_link(line, i, base) {
					out.push(Link {
						range: link_start..base + end,
						embed,
						..link
					});
					i = end;
				} else {
					i += 1;
				}
			}
			_ => i += 1,
		}
	}
}

/// The `[text](dest)` starting at `open`, and where it ends in `line`.
fn markdown_link(line: &str, open: usize, base: usize) -> Option<(Link, usize)> {
	let close = open + 1 + line[open + 1..].find(']')?;
	if line[open + 1..close].contains('[') || !line[close + 1..].starts_with('(') {
		return None;
	}
	let dest_start = close + 2;
	let lead = line[dest_start..].len() - line[dest_start..].trim_start().len();
	let path_start = dest_start + lead;
	let (path, end) = if line[path_start..].starts_with('<') {
		let gt = path_start + 1 + line[path_start + 1..].find('>')?;
		let end = gt + 1 + line[gt + 1..].find(')')?;
		(path_start + 1..gt, end + 1)
	} else {
		let end = dest_start + line[dest_start..].find(')')?;
		let dest = &line[path_start..end];
		// `path "title"`: the path stops at the first space.
		let len = dest.find(char::is_whitespace).unwrap_or(dest.len());
		(path_start..path_start + len, end + 1)
	};
	let text = &line[path.clone()];
	let is_url = text.contains("://") || text.starts_with("mailto:");
	let file_len = if is_url {
		text.len()
	} else {
		text.find('#').unwrap_or(text.len())
	};
	if file_len == 0 {
		return None;
	}
	let link = Link {
		range: 0..0,
		target: base + path.start..base + path.start + file_len,
		embed: false,
		kind: if is_url {
			LinkKind::Url
		} else {
			LinkKind::Markdown
		},
	};
	Some((link, end))
}

/// The link under `offset`, if any: a wikilink, a Markdown link or a bare URL.
pub fn link_at(text: &str, offset: usize) -> Option<LinkTarget> {
	let offset = offset.min(text.len());
	let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
	let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
	let line = &text[start..end];
	let at = offset - start;
	let mut links = Vec::new();
	scan_line(line, 0, &mut links);
	if let Some(link) = links
		.iter()
		.find(|l| l.range.start <= at && at < l.range.end)
	{
		let target = &line[link.target.clone()];
		return Some(match link.kind {
			LinkKind::Wiki => LinkTarget::File(target.to_string()),
			LinkKind::Markdown => LinkTarget::File(percent_decode(target)),
			LinkKind::Url => LinkTarget::Url(target.to_string()),
		});
	}
	// A bare URL: the whitespace-delimited token under the pointer.
	let token_start = line[..at].rfind(char::is_whitespace).map_or(0, |i| i + 1);
	let token_end = line[at..]
		.find(char::is_whitespace)
		.map_or(line.len(), |i| at + i);
	let token = line.get(token_start..token_end)?;
	let url_start = token.find("https://").or_else(|| token.find("http://"))?;
	let url = token[url_start..]
		.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '>', '"', '\'']);
	let range = token_start + url_start..token_start + url_start + url.len();
	(range.contains(&at)).then(|| LinkTarget::Url(url.to_string()))
}

/// `base/target` with `.` and `..` resolved, lowercase; `None` when it leaves the vault.
fn normalize(base: &str, target: &str) -> Option<String> {
	let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
	for part in target.split('/') {
		match part {
			"" | "." => {}
			".." => {
				parts.pop()?;
			}
			p => parts.push(p),
		}
	}
	Some(parts.join("/").to_lowercase())
}

/// Where the file name starts in a `/` path, and where its extension dot is (if any).
fn name_parts(path: &str) -> (usize, Option<usize>) {
	let name_start = path.rfind('/').map_or(0, |s| s + 1);
	let dot = path[name_start..]
		.rfind('.')
		.filter(|dot| *dot > 0)
		.map(|dot| name_start + dot);
	(name_start, dot)
}

/// The vault's files, indexed once to resolve many links.
pub struct FileIndex {
	files: Vec<PathBuf>,
	/// Lowercase `/` path of each file.
	lower: Vec<String>,
	by_path: HashMap<String, usize>,
	/// Files by lowercase file name, and by file name without its extension.
	by_name: HashMap<String, Vec<usize>>,
}

impl FileIndex {
	pub fn new(files: &[PathBuf]) -> Self {
		let lower: Vec<String> = files.iter().map(|f| slash(f).to_lowercase()).collect();
		let mut by_path = HashMap::with_capacity(files.len());
		let mut by_name: HashMap<String, Vec<usize>> = HashMap::with_capacity(files.len());
		for (i, path) in lower.iter().enumerate() {
			by_path.entry(path.clone()).or_insert(i);
			let (name_start, dot) = name_parts(path);
			by_name
				.entry(path[name_start..].to_string())
				.or_default()
				.push(i);
			if let Some(dot) = dot {
				by_name
					.entry(path[name_start..dot].to_string())
					.or_default()
					.push(i);
			}
		}
		FileIndex {
			files: files.to_vec(),
			lower,
			by_path,
			by_name,
		}
	}

	/// How many files have exactly this file name (any folder, any case).
	fn count_named(&self, name: &str) -> usize {
		let name = name.to_lowercase();
		self.by_name.get(&name).map_or(0, |found| {
			found
				.iter()
				.filter(|i| self.lower[**i][name_parts(&self.lower[**i]).0..] == name)
				.count()
		})
	}

	/// Resolves a link target like Obsidian: an exact path (from the note's folder or the
	/// vault root), then a file with that name anywhere, `.md` before `.excalidraw` before
	/// other files, the same folder first, then the shortest path.
	pub fn resolve(&self, from_dir: &Path, target: &str) -> Option<&PathBuf> {
		let target = target.trim().replace('\\', "/");
		let target = target.trim_matches('/');
		if target.is_empty() {
			return None;
		}
		let from = slash(from_dir).to_lowercase();
		for base in [from.as_str(), ""] {
			let Some(exact) = normalize(base, target) else {
				continue;
			};
			let found = self
				.by_path
				.get(&exact)
				.or_else(|| self.by_path.get(&format!("{exact}.md")));
			if let Some(i) = found {
				return Some(&self.files[*i]);
			}
		}
		let wanted = normalize("", target.trim_start_matches("./"))?;
		let ends_with = |path: &str| {
			path == wanted
				|| path
					.strip_suffix(wanted.as_str())
					.is_some_and(|rest| rest.ends_with('/'))
		};
		let name = &wanted[name_parts(&wanted).0..];
		let mut found: Vec<(u8, bool, usize, usize)> = Vec::new();
		for &i in self.by_name.get(name).into_iter().flatten() {
			let path = &self.lower[i];
			let (name_start, dot) = name_parts(path);
			let rank = if ends_with(path) {
				0
			} else if let Some(dot) = dot.filter(|dot| ends_with(&path[..*dot])) {
				match &path[dot + 1..] {
					"md" => 1,
					"excalidraw" => 2,
					_ => 3,
				}
			} else {
				continue;
			};
			let dir = path[..name_start].trim_end_matches('/');
			found.push((rank, dir != from, path.matches('/').count(), i));
		}
		found.sort_by(|a, b| {
			(a.0, a.1, a.2)
				.cmp(&(b.0, b.1, b.2))
				.then_with(|| natural_cmp(&self.lower[a.3], &self.lower[b.3]))
		});
		found.first().map(|f| &self.files[f.3])
	}
}

/// Resolves one link target (see `FileIndex::resolve`).
pub fn resolve_link(files: &[PathBuf], from_dir: &Path, target: &str) -> Option<PathBuf> {
	FileIndex::new(files).resolve(from_dir, target).cloned()
}

/// URL schemes a Ctrl+click may hand to the system: anything else (`file:`, app
/// protocols) could run a program.
pub fn is_safe_url(url: &str) -> bool {
	let lower = url.trim_start().to_lowercase();
	["http://", "https://", "mailto:"]
		.iter()
		.any(|scheme| lower.starts_with(scheme))
}

/// Files a link may open with their default app: documents and media, never something
/// that can run (scripts, programs, shortcuts).
pub fn is_safe_to_open(path: &Path) -> bool {
	const SAFE: [&str; 17] = [
		"pdf", "txt", "csv", "json", "docx", "xlsx", "pptx", "odt", "ods", "odp", "mp3", "wav",
		"ogg", "flac", "mp4", "webm", "mov",
	];
	path.extension()
		.is_some_and(|e| SAFE.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// The path of `to` relative to the folder `from_dir` (both relative to the vault).
fn relative(from_dir: &Path, to: &Path) -> String {
	let from = slash(from_dir);
	let to = slash(to);
	let from: Vec<&str> = from.split('/').filter(|p| !p.is_empty()).collect();
	let to: Vec<&str> = to.split('/').filter(|p| !p.is_empty()).collect();
	let dirs = to.len().saturating_sub(1);
	let common = from
		.iter()
		.zip(&to[..dirs])
		.take_while(|(a, b)| a.to_lowercase() == b.to_lowercase())
		.count();
	let mut parts: Vec<&str> = vec![".."; from.len() - common];
	parts.extend(&to[common..]);
	parts.join("/")
}

/// A path as the destination of a Markdown link: the characters that would end or split
/// it are percent-encoded (spaces too, unless the link uses `<>`).
fn encode_destination(path: &str, in_angles: bool) -> String {
	let mut out = String::with_capacity(path.len());
	for c in path.chars() {
		match c {
			'%' => out.push_str("%25"),
			'#' => out.push_str("%23"),
			'<' => out.push_str("%3C"),
			'>' => out.push_str("%3E"),
			' ' if !in_angles => out.push_str("%20"),
			'(' if !in_angles => out.push_str("%28"),
			')' if !in_angles => out.push_str("%29"),
			c => out.push(c),
		}
	}
	out
}

/// Characters a wikilink target cannot contain (they start a heading, an alias or end it).
const NOT_IN_WIKILINKS: [char; 5] = ['#', '|', '[', ']', '^'];

/// One or several files moved (a rename, a folder move): rewrites the links of any note so
/// that each keeps pointing at the same file.
pub struct MovePlan {
	/// New path of each moved file, by its lowercase `/` path before the move.
	moved: HashMap<String, PathBuf>,
	before: FileIndex,
	after: FileIndex,
}

impl MovePlan {
	/// `files` is the vault before the move.
	pub fn new(moves: &[(PathBuf, PathBuf)], files: &[PathBuf]) -> Self {
		let moved: HashMap<String, PathBuf> = moves
			.iter()
			.map(|(old, new)| (slash(old).to_lowercase(), new.clone()))
			.collect();
		let after: Vec<PathBuf> = files
			.iter()
			.map(|f| moved.get(&slash(f).to_lowercase()).unwrap_or(f).clone())
			.collect();
		MovePlan {
			before: FileIndex::new(files),
			after: FileIndex::new(&after),
			moved,
		}
	}

	/// Where `path` is after the move.
	fn now(&self, path: &Path) -> PathBuf {
		self.moved
			.get(&slash(path).to_lowercase())
			.cloned()
			.unwrap_or_else(|| path.to_path_buf())
	}

	/// How a wikilink written as `raw` names the file now at `dest`.
	fn wiki_target(&self, raw: &str, dest: &Path, dir_after: &Path) -> String {
		let keeps_md = raw.to_lowercase().ends_with(".md");
		let strip = |s: String| {
			let cut = s.len().saturating_sub(3);
			if !keeps_md && s.is_char_boundary(cut) && s[cut..].eq_ignore_ascii_case(".md") {
				s[..cut].to_string()
			} else {
				s
			}
		};
		let full = strip(slash(dest));
		if raw.contains(['/', '\\']) {
			return full;
		}
		let name = dest
			.file_name()
			.map(|n| n.to_string_lossy().into_owned())
			.unwrap_or_default();
		let unique = self.after.count_named(&name) == 1;
		let bare = strip(name);
		// A bare name is kept only when it cannot point to another file.
		if unique
			&& self
				.after
				.resolve(dir_after, &bare)
				.is_some_and(|p| same_path(p, dest))
		{
			bare
		} else {
			full
		}
	}

	/// The links of `note` (its path before the move) rewritten for the move: the new text
	/// and the number of links changed, or `None` when no link is affected.
	pub fn rewrite(&self, text: &str, note: &Path) -> Option<(String, usize)> {
		let links = parse_links(text);
		if links.is_empty() {
			return None;
		}
		let note_after = self.now(note);
		let dir_before = note.parent().unwrap_or(Path::new(""));
		let dir_after = note_after.parent().unwrap_or(Path::new(""));
		let mut edits: Vec<(Range<usize>, String)> = Vec::new();
		for link in links {
			let raw = &text[link.target.clone()];
			let markdown = link.kind == LinkKind::Markdown;
			let decoded = if markdown {
				percent_decode(raw)
			} else {
				raw.to_string()
			};
			let Some(resolved) = self.before.resolve(dir_before, &decoded) else {
				continue;
			};
			let dest = self.now(resolved);
			if markdown {
				// Only a move of the target or of the note changes a relative path.
				if same_path(&dest, resolved) && same_path(&note_after, note) {
					continue;
				}
				let rel = relative(dir_after, &dest);
				// Still the same relative path (nothing moved, or both moved together).
				if rel.to_lowercase() == decoded.replace('\\', "/").to_lowercase() {
					continue;
				}
				let in_angles = text[..link.target.start].ends_with('<');
				let replacement = encode_destination(&rel, in_angles);
				if replacement != raw {
					edits.push((link.target.clone(), replacement));
				}
				continue;
			}
			let still_right = self
				.after
				.resolve(dir_after, raw)
				.is_some_and(|p| same_path(p, &dest));
			if still_right && same_path(&dest, resolved) {
				continue;
			}
			let target = self.wiki_target(raw, &dest, dir_after);
			if target.contains(NOT_IN_WIKILINKS) {
				edits.push((
					link.range.clone(),
					self.as_markdown(text, &link, &dest, dir_after),
				));
			} else if target != raw {
				edits.push((link.target.clone(), target));
			}
		}
		if edits.is_empty() {
			return None;
		}
		let mut out = text.to_string();
		for (range, replacement) in edits.iter().rev() {
			out.replace_range(range.clone(), replacement);
		}
		Some((out, edits.len()))
	}

	/// The wikilink `link` as a Markdown link to `dest`: for file names a wikilink cannot
	/// hold. Its alias (or the file name) becomes the text, its heading is kept.
	fn as_markdown(&self, text: &str, link: &Link, dest: &Path, dir_after: &Path) -> String {
		// What follows the target inside the brackets: `#heading`, `|alias`.
		let rest = &text[link.target.end..link.range.end - 2];
		let (heading, alias) = match rest.find('|') {
			Some(pipe) => (&rest[..pipe], Some(&rest[pipe + 1..])),
			None => (rest, None),
		};
		let heading = heading.trim().trim_end_matches('\\');
		let stem = dest
			.file_stem()
			.map(|s| s.to_string_lossy().into_owned())
			.unwrap_or_default();
		let label = alias
			.map(str::trim)
			.filter(|a| !a.is_empty())
			.map_or(stem, str::to_string)
			.replace('[', "(")
			.replace(']', ")");
		let path = encode_destination(&relative(dir_after, dest), false);
		let heading = match heading.strip_prefix('#') {
			Some(h) => format!("#{}", h.trim().replace(' ', "%20")),
			None => String::new(),
		};
		let embed = if link.embed { "!" } else { "" };
		format!("{embed}[{label}]({path}{heading})")
	}
}

/// Rewrites the links of the note `note` (its path before the change) after `old` became
/// `new`. `files` is the vault before the change. Returns the new text and the number of
/// links changed, or `None` when no link is affected.
pub fn rewrite_links(
	text: &str,
	note: &Path,
	old: &Path,
	new: &Path,
	files: &[PathBuf],
) -> Option<(String, usize)> {
	MovePlan::new(&[(old.to_path_buf(), new.to_path_buf())], files).rewrite(text, note)
}

/// Applies several moves at once (a folder rename or move) to the links of `note`.
pub fn rewrite_for_moves(
	text: &str,
	note: &Path,
	moves: &[(PathBuf, PathBuf)],
	files: &[PathBuf],
) -> Option<(String, usize)> {
	MovePlan::new(moves, files).rewrite(text, note)
}

#[cfg(test)]
mod tests {
	use std::path::{Path, PathBuf};

	use super::{LinkKind, LinkTarget, link_at, parse_links, resolve_link, rewrite_links};

	fn files(paths: &[&str]) -> Vec<PathBuf> {
		paths.iter().map(PathBuf::from).collect()
	}

	fn targets(text: &str) -> Vec<&str> {
		parse_links(text)
			.iter()
			.map(|l| &text[l.target.clone()])
			.collect()
	}

	#[test]
	fn wikilinks_with_alias_heading_and_embed() {
		let t = "a [[Note]] b [[dir/Other#H|alias]] ![[img.png|300]] [[#local]] [[ spaced ]]";
		assert_eq!(targets(t), ["Note", "dir/Other", "img.png", "spaced"]);
		let links = parse_links(t);
		assert_eq!(&t[links[0].range.clone()], "[[Note]]");
		assert_eq!(&t[links[2].range.clone()], "![[img.png|300]]");
		assert!(links[2].embed && !links[0].embed);
		assert!(links.iter().all(|l| l.kind == LinkKind::Wiki));
	}

	#[test]
	fn markdown_links_to_files_only() {
		let t = "[a](b.md) [u](https://x.y/z) ![i](<my pic.png>) [t](c%20d.md#h \"T\") [m](mailto:a@b) [h](#top)";
		assert_eq!(targets(t), ["b.md", "my pic.png", "c%20d.md"]);
		let links = parse_links(t);
		assert!(links.iter().all(|l| l.kind == LinkKind::Markdown));
		assert_eq!(&t[links[1].range.clone()], "![i](<my pic.png>)");
	}

	#[test]
	fn links_in_code_are_ignored() {
		let t = "`[[a]]` [[b]]\n```\n[[c]]\n```\n[[d]]\n\u{e9} [[\u{e9}t\u{e9}]]";
		assert_eq!(targets(t), ["b", "d", "\u{e9}t\u{e9}"]);
	}

	#[test]
	fn link_under_the_pointer() {
		let t = "x [[A b|c]] [d](e%20f.md) [g](https://h.i/j) see http://k.l/m, ok";
		let at = |needle: &str| link_at(t, t.find(needle).unwrap() + 1);
		assert_eq!(at("[[A"), Some(LinkTarget::File("A b".into())));
		assert_eq!(at("|c"), Some(LinkTarget::File("A b".into())));
		assert_eq!(at("[d]"), Some(LinkTarget::File("e f.md".into())));
		assert_eq!(at("[g]"), Some(LinkTarget::Url("https://h.i/j".into())));
		assert_eq!(at("http://k"), Some(LinkTarget::Url("http://k.l/m".into())));
		assert_eq!(at("x "), None);
		assert_eq!(at("ok"), None);
	}

	#[test]
	fn resolution_order() {
		let f = files(&[
			"a/Note.md",
			"Note.md",
			"b/c/Note.md",
			"b/Note.excalidraw",
			"b/Other.excalidraw",
			"pics/img.png",
			"b/\u{c9}t\u{e9}.md",
		]);
		let r = |from: &str, target: &str| resolve_link(&f, Path::new(from), target);
		// Same folder first, then the shortest path.
		assert_eq!(r("a", "Note"), Some("a/Note.md".into()));
		assert_eq!(r("zzz", "note"), Some("Note.md".into()));
		assert_eq!(r("b/c", "Note"), Some("b/c/Note.md".into()));
		// `.md` wins over a drawing in the same folder.
		assert_eq!(r("b", "Note"), Some("Note.md".into()));
		assert_eq!(r("", "Other"), Some("b/Other.excalidraw".into()));
		// Exact paths, with or without the extension, from the root or the note's folder.
		assert_eq!(r("", "b/c/Note"), Some("b/c/Note.md".into()));
		assert_eq!(r("", "b/c/note.MD"), Some("b/c/Note.md".into()));
		assert_eq!(r("b", "c/Note"), Some("b/c/Note.md".into()));
		assert_eq!(r("b/c", "../../a/Note.md"), Some("a/Note.md".into()));
		// A partial path matches the end of a longer one.
		assert_eq!(r("", "c/Note"), Some("b/c/Note.md".into()));
		assert_eq!(r("", "img.png"), Some("pics/img.png".into()));
		assert_eq!(r("", "\u{e9}t\u{e9}"), Some("b/\u{c9}t\u{e9}.md".into()));
		assert_eq!(r("", "missing"), None);
		assert_eq!(r("", ""), None);
	}

	#[test]
	fn rename_rewrites_wikilinks_keeping_alias_and_heading() {
		let f = files(&["Old.md", "Index.md", "dir/Other.md"]);
		let t = "[[Old]] [[Old#H|alias]] ![[Old]] [[Other]] [[old.md]]";
		let (out, n) = rewrite_links(
			t,
			Path::new("Index.md"),
			Path::new("Old.md"),
			Path::new("New name.md"),
			&f,
		)
		.unwrap();
		assert_eq!(
			out,
			"[[New name]] [[New name#H|alias]] ![[New name]] [[Other]] [[New name.md]]"
		);
		assert_eq!(n, 4);
	}

	#[test]
	fn move_keeps_bare_names_when_unambiguous() {
		let f = files(&["Old.md", "Index.md", "dir/Old.md"]);
		// `[[Old]]` from the root resolves to Old.md; after the move a bare name would
		// resolve to dir/Old.md, so the full path is written.
		let (out, _) = rewrite_links(
			"[[Old]]",
			Path::new("Index.md"),
			Path::new("Old.md"),
			Path::new("arch/Old.md"),
			&f,
		)
		.unwrap();
		assert_eq!(out, "[[arch/Old]]");
		let f = files(&["Old.md", "Index.md"]);
		assert_eq!(
			rewrite_links(
				"[[Old]]",
				Path::new("Index.md"),
				Path::new("Old.md"),
				Path::new("arch/Old.md"),
				&f
			),
			None
		);
		let (out, _) = rewrite_links(
			"[[Old]] [[./Old]]",
			Path::new("Index.md"),
			Path::new("Old.md"),
			Path::new("arch/Older.md"),
			&f,
		)
		.unwrap();
		assert_eq!(out, "[[Older]] [[arch/Older]]");
	}

	#[test]
	fn markdown_links_stay_relative() {
		let f = files(&["notes/Old.md", "notes/Index.md", "pics/a b.png"]);
		let t = "[x](Old.md#h) [y](<Old.md>) ![p](../pics/a%20b.png)";
		let (out, n) = rewrite_links(
			t,
			Path::new("notes/Index.md"),
			Path::new("notes/Old.md"),
			Path::new("arch/New one.md"),
			&f,
		)
		.unwrap();
		assert_eq!(
			out,
			"[x](../arch/New%20one.md#h) [y](<../arch/New one.md>) ![p](../pics/a%20b.png)"
		);
		assert_eq!(n, 2);
	}

	#[test]
	fn moving_a_note_fixes_its_own_relative_links() {
		let f = files(&["notes/Me.md", "notes/Sib.md", "pics/a.png"]);
		let t = "[s](Sib.md) ![p](../pics/a.png) [[Sib]]";
		let (out, n) = rewrite_links(
			t,
			Path::new("notes/Me.md"),
			Path::new("notes/Me.md"),
			Path::new("Me.md"),
			&f,
		)
		.unwrap();
		assert_eq!(out, "[s](notes/Sib.md) ![p](pics/a.png) [[Sib]]");
		assert_eq!(n, 2);
		// A plain rename in place changes nothing in the note itself.
		assert_eq!(
			rewrite_links(
				t,
				Path::new("notes/Me.md"),
				Path::new("notes/Me.md"),
				Path::new("notes/You.md"),
				&f
			),
			None
		);
	}

	#[test]
	fn folder_move_rewrites_once_and_ignores_internal_links() {
		let f = files(&["dir/A.md", "dir/B.md", "Index.md"]);
		let moves = vec![
			(PathBuf::from("dir/A.md"), PathBuf::from("arch/A.md")),
			(PathBuf::from("dir/B.md"), PathBuf::from("arch/B.md")),
		];
		let (out, n) = super::rewrite_for_moves(
			"[[dir/B]] [[A]] [a](dir/A.md)",
			Path::new("Index.md"),
			&moves,
			&f,
		)
		.unwrap();
		assert_eq!(out, "[[arch/B]] [[A]] [a](arch/A.md)");
		assert_eq!(n, 2);
		// Links between files that moved together still work: nothing to change.
		assert_eq!(
			super::rewrite_for_moves("[x](B.md) [[B]]", Path::new("dir/A.md"), &moves, &f),
			None
		);
	}

	fn rename(text: &str, note: &str, old: &str, new: &str, f: &[PathBuf]) -> Option<String> {
		rewrite_links(text, Path::new(note), Path::new(old), Path::new(new), f).map(|r| r.0)
	}

	#[test]
	fn names_a_link_cannot_hold_become_encoded_markdown_links() {
		let f = files(&["Old.md", "Index.md"]);
		let out = rename(
			"[[Old]] [x](Old.md) ![[Old#H|al]]",
			"Index.md",
			"Old.md",
			"C# notes.md",
			&f,
		)
		.unwrap();
		assert_eq!(
			out,
			"[C# notes](C%23%20notes.md) [x](C%23%20notes.md) ![al](C%23%20notes.md#H)"
		);
		// Every rewritten link resolves to the renamed file.
		let after = files(&["C# notes.md", "Index.md"]);
		let links = parse_links(&out);
		assert_eq!(links.len(), 3);
		for link in links {
			let target = crate::markdown::percent_decode(&out[link.target]);
			assert_eq!(
				resolve_link(&after, Path::new(""), &target),
				Some("C# notes.md".into())
			);
		}
		assert_eq!(
			rename("[x](Old.md)", "Index.md", "Old.md", "a) b.md", &f).unwrap(),
			"[x](a%29%20b.md)"
		);
		assert_eq!(
			rename("[x](Old.md)", "Index.md", "Old.md", "50%20.md", &f).unwrap(),
			"[x](50%2520.md)"
		);
		assert_eq!(
			rename("[[Old]]", "Index.md", "Old.md", "x [y].md", &f).unwrap(),
			"[x (y)](x%20[y].md)"
		);
		assert_eq!(targets("[x (y)](x%20[y].md)"), ["x%20[y].md"]);
	}

	#[test]
	fn a_moved_note_keeps_pointing_at_the_same_files() {
		let f = files(&["notes/Me.md", "notes/Sib.md", "a/Sib.md", "Top.md"]);
		assert_eq!(
			rename(
				"[[Sib]] [[../Top]]",
				"notes/Me.md",
				"notes/Me.md",
				"deep/er/Me.md",
				&f
			)
			.unwrap(),
			"[[notes/Sib]] [[Top]]"
		);
	}

	#[test]
	fn a_move_does_not_retarget_links_to_other_files() {
		let f = files(&["other/A.md", "zzz/A.md", "Index.md"]);
		assert_eq!(
			resolve_link(&f, Path::new(""), "A"),
			Some("other/A.md".into())
		);
		let moves = vec![(PathBuf::from("zzz/A.md"), PathBuf::from("dir/A.md"))];
		let (out, _) =
			super::rewrite_for_moves("[[A]]", Path::new("Index.md"), &moves, &f).unwrap();
		assert_eq!(out, "[[other/A]]");
	}

	#[test]
	fn escaped_alias_pipe_in_a_table_is_kept() {
		let f = files(&["Old.md", "Index.md"]);
		assert_eq!(targets("| [[Old\\|alias]] |"), ["Old"]);
		assert_eq!(
			rename("| [[Old\\|alias]] |", "Index.md", "Old.md", "New.md", &f).unwrap(),
			"| [[New\\|alias]] |"
		);
	}

	#[test]
	fn fences_and_math_blocks_hide_links() {
		assert_eq!(targets("```\n~~~\n[[a]]\n```\n[[b]]"), ["b"]);
		assert_eq!(targets("```js```\n[[c]]"), ["c"]);
		assert_eq!(targets("````\n```\n[[a]]\n````\n[[d]]"), ["d"]);
		assert_eq!(targets("$$\n[[m]]\n$$\n[[e]]"), ["e"]);
	}

	#[test]
	fn only_web_links_and_documents_are_opened() {
		assert!(super::is_safe_url("https://example.com/a"));
		assert!(super::is_safe_url("HTTP://example.com"));
		assert!(super::is_safe_url("mailto:a@b.c"));
		for url in [
			"file:///C:/Windows/System32/calc.exe",
			"ms-settings://x",
			"search-ms://q",
			"javascript://x",
		] {
			assert!(!super::is_safe_url(url), "{url}");
		}
		assert!(super::is_safe_to_open(Path::new("docs/Paper.PDF")));
		for path in [
			"tools/readme.bat",
			"run.exe",
			"a.lnk",
			"x.ps1",
			"noext",
			"s.url",
		] {
			assert!(!super::is_safe_to_open(Path::new(path)), "{path}");
		}
	}

	#[test]
	fn a_folder_move_in_a_large_vault_is_fast() {
		let mut f: Vec<PathBuf> = (0..3000)
			.map(|i| PathBuf::from(format!("notes/n{i}.md")))
			.collect();
		f.extend((0..300).map(|i| PathBuf::from(format!("dir/m{i}.md"))));
		let moves: Vec<_> = (0..300)
			.map(|i| {
				(
					PathBuf::from(format!("dir/m{i}.md")),
					PathBuf::from(format!("arch/m{i}.md")),
				)
			})
			.collect();
		let text: String = (0..40)
			.map(|i| format!("[[dir/m{i}]] [[n{i}]]\n"))
			.collect();
		let started = std::time::Instant::now();
		let plan = super::MovePlan::new(&moves, &f);
		for _ in 0..50 {
			let (out, n) = plan.rewrite(&text, Path::new("notes/n0.md")).unwrap();
			assert_eq!(n, 40);
			assert!(out.starts_with("[[arch/m0]] [[n0]]"));
		}
		// 50 notes of 80 links: about 6 s per note before the files were indexed.
		assert!(
			started.elapsed() < std::time::Duration::from_secs(2),
			"{:?}",
			started.elapsed()
		);
	}
}
