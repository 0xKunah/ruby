//! Markdown understanding for live preview: styles, line kinds and concealable markup.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::{BitOr, BitOrAssign, Range};
use std::rc::Rc;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};

/// Inline style flags of a text range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Styles(pub u16);

impl Styles {
	pub const NONE: Styles = Styles(0);
	pub const BOLD: Styles = Styles(1);
	pub const ITALIC: Styles = Styles(1 << 1);
	pub const STRIKE: Styles = Styles(1 << 2);
	pub const HIGHLIGHT: Styles = Styles(1 << 3);
	pub const CODE: Styles = Styles(1 << 4);
	pub const LINK: Styles = Styles(1 << 5);
	/// Syntax characters (`**`, `#`, backticks...), drawn dimmed when visible.
	pub const MARKUP: Styles = Styles(1 << 6);
	/// Text of a checked task item.
	pub const DONE: Styles = Styles(1 << 7);

	pub fn contains(self, other: Styles) -> bool {
		other.0 != 0 && self.0 & other.0 == other.0
	}
}

impl BitOr for Styles {
	type Output = Styles;
	fn bitor(self, rhs: Styles) -> Styles {
		Styles(self.0 | rhs.0)
	}
}

impl BitOrAssign for Styles {
	fn bitor_assign(&mut self, rhs: Styles) {
		self.0 |= rhs.0;
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineKind {
	Text,
	Heading(u8),
	Quote,
	ListItem,
	CodeFence,
	Code,
	Rule,
	Blank,
	Table,
	Callout,
	Frontmatter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
	None,
	Left,
	Center,
	Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
	pub align: Vec<Align>,
	/// Source ranges of the cells (trimmed), row 0 being the header.
	pub rows: Vec<Vec<Range<usize>>>,
	/// Line index of the `|---|` line.
	pub rule_line: usize,
}

/// A part of the note drawn as a widget in live preview while no selection is inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
	Code {
		lang: String,
	},
	Table(Table),
	Callout {
		kind: String,
		title: String,
	},
	Image {
		target: String,
		width: Option<u32>,
	},
	/// `![[target]]` of a file that is not an image: a drawing when the target resolves to
	/// one. `size` is the number after `|`.
	Embed {
		target: String,
		size: Option<u32>,
	},
	Frontmatter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
	pub kind: BlockKind,
	/// Line indices.
	pub lines: Range<usize>,
	pub source: Range<usize>,
}

/// A task item's box: `marker` is the `[ ]` or `[x]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
	pub marker: Range<usize>,
	pub checked: bool,
}

/// What a task's `- [ ] ` shows in live preview: room for the drawn checkbox.
pub const TASK_GAP: &str = "\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}";

/// A block shows its raw Markdown while any selection intersects or touches it.
pub fn block_revealed(block: &Block, selections: &[crate::editor_state::Selection]) -> bool {
	selections.iter().any(|s| {
		let r = s.range();
		r.start <= block.source.end && r.end >= block.source.start
	})
}

/// Decodes `%XX` escapes (URL-encoded Markdown link paths); invalid ones are kept as is.
pub(crate) fn percent_decode(path: &str) -> String {
	let bytes = path.as_bytes();
	let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
	let mut out = Vec::with_capacity(bytes.len());
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] == b'%'
			&& let (Some(h), Some(l)) = (
				bytes.get(i + 1).copied().and_then(hex),
				bytes.get(i + 2).copied().and_then(hex),
			) {
			out.push(h * 16 + l);
			i += 3;
		} else {
			out.push(bytes[i]);
			i += 1;
		}
	}
	String::from_utf8(out).unwrap_or_else(|_| path.to_string())
}

/// How many `>` quote markers precede the content of `line` (a fence line inside quotes).
pub fn quote_depth(line: &str) -> usize {
	line.chars()
		.take_while(|c| matches!(c, '>' | ' ' | '\t'))
		.filter(|c| *c == '>')
		.count()
}

/// Byte length of up to `depth` quote markers (`>` and one following space each) at the
/// start of `line`: what precedes the code on a code line inside quotes.
pub fn quote_prefix_len(line: &str, depth: usize) -> usize {
	let mut len = 0;
	for _ in 0..depth {
		let rest = &line[len..];
		let indent = rest.len() - rest.trim_start_matches(' ').len();
		if indent > 3 || !rest[indent..].starts_with('>') {
			break;
		}
		len += indent + 1;
		if line[len..].starts_with(' ') {
			len += 1;
		}
	}
	len
}

const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"];

/// `![[target]]`, `![[target|300]]` or `![alt](path)` alone on a line, with an image extension.
fn image_embed(line: &str) -> Option<(String, Option<u32>)> {
	let line = line.trim();
	let (target, width) =
		if let Some(inner) = line.strip_prefix("![[").and_then(|l| l.strip_suffix("]]")) {
			let (target, size) = inner
				.split_once('|')
				.map_or((inner, None), |(t, s)| (t, Some(s)));
			(
				target.trim().to_string(),
				size.and_then(|s| s.trim().parse().ok()),
			)
		} else {
			let rest = line.strip_prefix("![")?;
			let (_, path) = rest.split_once("](")?;
			let path = path.strip_suffix(')')?.trim();
			let path = match path.strip_prefix('<').and_then(|p| p.split_once('>')) {
				Some((inner, _)) => inner,
				// `path "title"`: the title is not part of the path.
				None => path.split_once(" \"").map_or(path, |(p, _)| p).trim_end(),
			};
			(percent_decode(path), None)
		};
	let ext = target.rsplit_once('.')?.1.to_ascii_lowercase();
	(!target.contains(']') && IMAGE_EXTENSIONS.contains(&ext.as_str())).then_some((target, width))
}

/// `![[target]]` or `![[target|300]]` alone on a line, where the target can be a drawing:
/// it ends in `.excalidraw` or has no extension, and points to no heading or block.
fn file_embed(line: &str) -> Option<(String, Option<u32>)> {
	let inner = line.trim().strip_prefix("![[")?.strip_suffix("]]")?;
	let (target, size) = inner
		.split_once('|')
		.map_or((inner, None), |(t, s)| (t, Some(s)));
	let target = target.trim();
	let name = target.rsplit(['/', '\\']).next().unwrap_or(target);
	let candidate = match name.rsplit_once('.') {
		Some((_, ext)) => ext.eq_ignore_ascii_case("excalidraw"),
		None => true,
	};
	(candidate && !target.is_empty() && !target.contains(['#', '^', '[', ']']))
		.then(|| (target.to_string(), size.and_then(|s| s.trim().parse().ok())))
}

/// Markup hidden in live preview (or shown as `replacement`) unless a selection reveals it:
/// inline elements when a selection touches `reveal`, line prefixes (`line_scope`) when a
/// selection is on the same line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Concealed {
	pub range: Range<usize>,
	pub replacement: Option<&'static str>,
	pub reveal: Range<usize>,
	pub line_scope: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decorations {
	/// One entry per logical line (`text.split('\n')`).
	pub line_kinds: Vec<LineKind>,
	/// Styled ranges; they may overlap (bold inside a link).
	pub styles: Vec<(Range<usize>, Styles)>,
	/// Sorted by `range.start`.
	pub concealed: Vec<Concealed>,
	/// Byte offset where each line starts.
	pub line_starts: Vec<usize>,
	/// The indices into `styles` of the ranges touching each line, one line after the
	/// other; `line_style_ends[line]` is where the indices of `line` end.
	line_styles: Vec<u32>,
	line_style_ends: Vec<u32>,
	/// For each line, the section it was parsed in and its line index there: the same for
	/// as long as the section does not change (empty when the note was parsed in one go).
	pub line_ids: Vec<(u64, u32)>,
	/// Widget blocks (code, tables, callouts, images, frontmatter), sorted by source start.
	pub blocks: Vec<Block>,
	pub tasks: Vec<Task>,
}

impl Decorations {
	/// Indices into `styles` of the ranges touching `line`.
	pub fn styles_of_line(&self, line: usize) -> &[u32] {
		let Some(&end) = self.line_style_ends.get(line) else {
			return &[];
		};
		let start = match line {
			0 => 0,
			_ => self.line_style_ends[line - 1],
		};
		&self.line_styles[start as usize..end as usize]
	}
}

pub const BULLET: &str = "\u{2022} ";

#[derive(Clone, Copy, PartialEq)]
enum OpenKind {
	Inline(Styles),
	Heading,
	Table,
	Other,
}

struct Open {
	kind: OpenKind,
	range: Range<usize>,
	first_child: Option<usize>,
	last_child_end: usize,
}

fn note_child(stack: &mut [Open], range: &Range<usize>) {
	if let Some(parent) = stack.last_mut() {
		parent.first_child.get_or_insert(range.start);
		parent.last_child_end = parent.last_child_end.max(range.end);
	}
}

fn heading_number(level: HeadingLevel) -> u8 {
	match level {
		HeadingLevel::H1 => 1,
		HeadingLevel::H2 => 2,
		HeadingLevel::H3 => 3,
		HeadingLevel::H4 => 4,
		HeadingLevel::H5 => 5,
		HeadingLevel::H6 => 6,
	}
}

struct Builder<'a> {
	text: &'a str,
	line_starts: Vec<usize>,
	decorations: Decorations,
	code: Vec<Range<usize>>,
}

impl Builder<'_> {
	fn line_of(&self, offset: usize) -> usize {
		self.line_starts
			.partition_point(|&s| s <= offset)
			.saturating_sub(1)
	}

	/// Content range of a line, without its `\n`.
	fn line_range(&self, line: usize) -> Range<usize> {
		let start = self.line_starts[line];
		let end = self
			.line_starts
			.get(line + 1)
			.map_or(self.text.len(), |&s| s - 1);
		start..end
	}

	fn lines_in(&self, range: &Range<usize>) -> Range<usize> {
		let first = self.line_of(range.start);
		let last = self.line_of(range.end.saturating_sub(1).max(range.start));
		first..last + 1
	}

	fn conceal(&mut self, range: Range<usize>, reveal: Range<usize>, line_scope: bool) {
		if range.is_empty() {
			return;
		}
		self.decorations
			.styles
			.push((range.clone(), Styles::MARKUP));
		self.decorations.concealed.push(Concealed {
			range,
			replacement: None,
			reveal,
			line_scope,
		});
	}

	fn close_inline(&mut self, open: &Open, style: Styles) {
		let Some(first) = open.first_child else {
			return;
		};
		let inner = first..open.last_child_end;
		if inner.is_empty() {
			return;
		}
		self.decorations.styles.push((inner.clone(), style));
		self.conceal(open.range.start..inner.start, open.range.clone(), false);
		self.conceal(inner.end..open.range.end, open.range.clone(), false);
	}

	fn close_heading(&mut self, open: &Open) {
		let line = self.line_of(open.range.start);
		let line_range = self.line_range(line);
		if let Some(first) = open.first_child
			&& first <= line_range.end
		{
			self.conceal(line_range.start..first, line_range, true);
		}
	}

	/// `lang` is `Some` for a fenced block (its info string's first word, possibly empty).
	fn code_block(&mut self, range: &Range<usize>, lang: Option<String>) {
		self.code.push(range.clone());
		let lines = self.lines_in(range);
		for line in lines.clone() {
			self.decorations.line_kinds[line] = LineKind::Code;
		}
		if let Some(lang) = lang {
			self.decorations.line_kinds[lines.start] = LineKind::CodeFence;
			let last = lines.end - 1;
			let closing = self.text[self.line_range(last)]
				.trim_start_matches(|c: char| c == '>' || c.is_whitespace());
			if last > lines.start && (closing.starts_with("```") || closing.starts_with("~~~")) {
				self.decorations.line_kinds[last] = LineKind::CodeFence;
			}
			self.push_block(BlockKind::Code { lang }, lines);
		}
	}

	fn push_block(&mut self, kind: BlockKind, lines: Range<usize>) {
		let source = self.line_starts[lines.start]..self.line_range(lines.end - 1).end;
		self.decorations.blocks.push(Block {
			kind,
			lines,
			source,
		});
	}

	fn frontmatter(&mut self, range: &Range<usize>) {
		self.code.push(range.clone());
		let lines = self.lines_in(range);
		for line in lines.clone() {
			self.decorations.line_kinds[line] = LineKind::Frontmatter;
		}
		self.push_block(BlockKind::Frontmatter, lines);
	}

	fn quote(&mut self, range: &Range<usize>) {
		let lines = self.lines_in(range);
		let callout = self.callout_header(lines.start);
		if let Some((kind, title, _)) = &callout {
			let block = BlockKind::Callout {
				kind: kind.clone(),
				title: title.clone(),
			};
			self.push_block(block, lines.clone());
		}
		for line in lines {
			let line_range = self.line_range(line);
			let content = &self.text[line_range.clone()];
			let indent = content.len() - content.trim_start().len();
			if !content[indent..].starts_with('>') {
				continue;
			}
			let mut end = line_range.start + indent + 1;
			if self.text[end..].starts_with(' ') {
				end += 1;
			}
			if let Some((_, _, title_start)) = callout
				.as_ref()
				.filter(|_| line == self.line_of(range.start))
			{
				end = *title_start;
			}
			if matches!(
				self.decorations.line_kinds[line],
				LineKind::Text | LineKind::Quote
			) {
				self.decorations.line_kinds[line] = if callout.is_some() {
					LineKind::Callout
				} else {
					LineKind::Quote
				};
			}
			self.conceal(line_range.start..end, line_range, true);
		}
	}

	/// `> [!kind] Title` on a quote's first line: kind (lowercase), title (or the capitalized
	/// kind) and where the title starts.
	fn callout_header(&self, line: usize) -> Option<(String, String, usize)> {
		let line_range = self.line_range(line);
		let content = &self.text[line_range.clone()];
		let after_gt = content.trim_start().strip_prefix('>')?.trim_start();
		let inner = after_gt.strip_prefix("[!")?;
		let (kind, rest) = inner.split_once(']')?;
		if kind.is_empty() || kind.contains(char::is_whitespace) {
			return None;
		}
		let rest = rest.strip_prefix(['+', '-']).unwrap_or(rest);
		let title = rest.trim();
		let title_start = line_range.end - rest.trim_start().len();
		let kind = kind.to_lowercase();
		let title = if title.is_empty() {
			let mut c = kind.chars();
			c.next()
				.map(|f| f.to_uppercase().chain(c).collect())
				.unwrap_or_default()
		} else {
			title.to_string()
		};
		Some((kind, title, title_start))
	}

	/// `[ ]` / `[x]` of a task item: its bullet and box become room for a drawn checkbox.
	fn task(&mut self, marker: &Range<usize>, checked: bool) {
		self.decorations.tasks.push(Task {
			marker: marker.clone(),
			checked,
		});
		let line = self.line_of(marker.start);
		let line_range = self.line_range(line);
		let mut end = marker.end;
		if self.text[end..].starts_with(' ') {
			end += 1;
		}
		if let Some(bullet) = self.decorations.concealed.iter_mut().rev().find(|c| {
			c.replacement == Some(BULLET)
				&& c.range.start >= line_range.start
				&& c.range.end <= marker.start
		}) {
			bullet.range.end = end;
			bullet.reveal = bullet.range.clone();
			bullet.replacement = Some(TASK_GAP);
		}
		if checked && end < line_range.end {
			self.decorations
				.styles
				.push((end..line_range.end, Styles::DONE));
		}
	}

	fn finish_table(
		&mut self,
		range: &Range<usize>,
		align: Vec<Align>,
		rows: Vec<Vec<Range<usize>>>,
	) {
		let lines = self.lines_in(range);
		for line in lines.clone() {
			self.decorations.line_kinds[line] = LineKind::Table;
		}
		let table = Table {
			align,
			rows,
			rule_line: lines.start + 1,
		};
		self.push_block(BlockKind::Table(table), lines);
	}

	/// Cell source range without surrounding spaces and pipes.
	fn trim_cell(&self, range: &Range<usize>) -> Range<usize> {
		let s = &self.text[range.clone()];
		let start = range.start + (s.len() - s.trim_start_matches([' ', '\t', '|']).len());
		let end = range.end - (s.len() - s.trim_end_matches([' ', '\t', '|']).len());
		start..end.max(start)
	}

	fn image_lines(&mut self) {
		for line in 0..self.line_starts.len() {
			if self.decorations.line_kinds[line] != LineKind::Text {
				continue;
			}
			let content = &self.text[self.line_range(line)];
			if let Some((target, width)) = image_embed(content) {
				self.push_block(BlockKind::Image { target, width }, line..line + 1);
			} else if let Some((target, size)) = file_embed(content) {
				self.push_block(BlockKind::Embed { target, size }, line..line + 1);
			}
		}
	}

	fn item(&mut self, range: &Range<usize>) {
		let line = self.line_of(range.start);
		if matches!(
			self.decorations.line_kinds[line],
			LineKind::Text | LineKind::Blank
		) {
			self.decorations.line_kinds[line] = LineKind::ListItem;
		}
		// A nested item's range starts before its indentation.
		let rest = &self.text[range.start..];
		let indent = rest.len() - rest.trim_start_matches([' ', '\t']).len();
		let rest = &rest[indent..];
		if rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ") {
			let start = range.start + indent;
			let marker = start..start + 2;
			self.decorations
				.styles
				.push((marker.clone(), Styles::MARKUP));
			self.decorations.concealed.push(Concealed {
				range: marker.clone(),
				replacement: Some(BULLET),
				reveal: marker,
				line_scope: false,
			});
		}
	}

	fn inline_code(&mut self, range: &Range<usize>) {
		self.code.push(range.clone());
		let s = &self.text[range.clone()];
		let lead = s.len() - s.trim_start_matches('`').len();
		let trail = s.len() - s.trim_end_matches('`').len();
		if lead + trail >= s.len() {
			return;
		}
		let inner = range.start + lead..range.end - trail;
		self.decorations.styles.push((inner, Styles::CODE));
		self.conceal(range.start..range.start + lead, range.clone(), false);
		self.conceal(range.end - trail..range.end, range.clone(), false);
	}

	fn rule(&mut self, range: &Range<usize>) {
		let line = self.line_of(range.start);
		let line_range = self.line_range(line);
		self.decorations.line_kinds[line] = LineKind::Rule;
		self.conceal(line_range.clone(), line_range, true);
	}

	/// `==text==` (not supported by the parser), outside code.
	fn highlights(&mut self) {
		for line in 0..self.line_starts.len() {
			if matches!(
				self.decorations.line_kinds[line],
				LineKind::Code | LineKind::CodeFence
			) {
				continue;
			}
			let line_range = self.line_range(line);
			let content = &self.text[line_range.clone()];
			let mut from = 0;
			while let Some(open) = content[from..].find("==").map(|i| from + i) {
				let Some(close) = content[open + 2..].find("==").map(|i| open + 2 + i) else {
					break;
				};
				let inner = &content[open + 2..close];
				let whole = line_range.start + open..line_range.start + close + 2;
				let in_code = self
					.code
					.iter()
					.any(|c| c.start < whole.end && whole.start < c.end);
				if !inner.is_empty() && inner.trim() == inner && !in_code {
					let inner_range = whole.start + 2..whole.end - 2;
					self.decorations
						.styles
						.push((inner_range, Styles::HIGHLIGHT));
					self.conceal(whole.start..whole.start + 2, whole.clone(), false);
					self.conceal(whole.end - 2..whole.end, whole.clone(), false);
					from = close + 2;
				} else {
					from = open + 2;
				}
			}
		}
	}
}

/// The note with every indentation tab expanded to 4 spaces, for the parser: pulldown-cmark does
/// not nest tab-indented list items (Obsidian indents nested lists with tabs). `tabs` holds, for
/// each expanded tab, its offset in the expanded text and in the original text.
struct Expanded {
	text: String,
	tabs: Vec<(usize, usize)>,
}

impl Expanded {
	fn new(text: &str) -> Option<Expanded> {
		if !text
			.split('\n')
			.any(|l| l.trim_start_matches(' ').starts_with('\t'))
		{
			return None;
		}
		let mut out = String::with_capacity(text.len() + 64);
		let mut tabs = Vec::new();
		for (i, line) in text.split('\n').enumerate() {
			if i > 0 {
				out.push('\n');
			}
			let line_start = line.as_ptr() as usize - text.as_ptr() as usize;
			let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
			for (j, c) in line[..indent].char_indices() {
				if c == '\t' {
					tabs.push((out.len(), line_start + j));
					out.push_str("    ");
				} else {
					out.push(c);
				}
			}
			out.push_str(&line[indent..]);
		}
		Some(Expanded { text: out, tabs })
	}

	/// Original offset of an expanded-text offset (inside an expanded tab: the tab's start, or
	/// just after it).
	fn to_original(&self, t: usize) -> usize {
		let before = self.tabs.partition_point(|&(ts, _)| ts + 4 <= t);
		if let Some(&(ts, o)) = self.tabs.get(before)
			&& ts <= t
		{
			return if t == ts { o } else { o + 1 };
		}
		t - 3 * before
	}
}

/// The last top-level block of a section, for what it says about the line that follows.
enum LastBlock {
	Other,
	Fenced(Range<usize>),
	Html,
}

/// Whether the fenced code block at `range` has its closing fence.
fn fence_closed(text: &str, range: &Range<usize>) -> bool {
	let block = text[range.clone()].trim_end_matches('\n');
	let Some((_, last)) = block.rsplit_once('\n') else {
		return false;
	};
	let opener = block.trim_start_matches(' ');
	let Some(fence) = opener.chars().next() else {
		return false;
	};
	let count = |s: &str| s.chars().take_while(|c| *c == fence).count();
	let closer = last.trim_start_matches(' ').trim_end();
	last.len() - last.trim_start_matches(' ').len() <= 3
		&& closer.chars().all(|c| c == fence)
		&& count(closer) >= count(opener).max(3)
}

/// A part of a note parsed on its own.
struct Section {
	decorations: Decorations,
	len: usize,
	/// The section ends inside a block that a heading line cannot close (an unclosed fence,
	/// raw HTML, frontmatter still open): the next heading line may belong to it.
	open_end: bool,
	/// It defines link references, which any other section may use.
	defines_references: bool,
}

/// Offsets of the lines that look like a heading (`#` to `######` at the very start of a
/// line, then a space or the end of the line), the first line excepted: where a note can be
/// cut into sections.
fn heading_starts(text: &str) -> Vec<usize> {
	let bytes = text.as_bytes();
	let mut out = Vec::new();
	let mut from = 0;
	while let Some(found) = text[from..].find("\n#") {
		let at = from + found + 1;
		let hashes = bytes[at..].iter().take_while(|b| **b == b'#').count();
		if hashes <= 6
			&& matches!(
				bytes.get(at + hashes),
				None | Some(b' ' | b'\t' | b'\n' | b'\r')
			) {
			out.push(at);
		}
		from = at;
	}
	out
}

/// Decorates a note section by section and remembers the sections: after an edit only the
/// section that changed is parsed again, which keeps typing fast in a very long note.
///
/// A section starts at a heading line. Such a line closes every list and quote, so sections
/// can be parsed on their own, with three exceptions that are detected and handled: a block
/// still open at the end of a section (it is then parsed together with the next ones), link
/// reference definitions (the note is then parsed in one go), and frontmatter.
#[derive(Default)]
pub struct Decorator {
	/// Sections by the hash of their text, with the call they were last used in.
	sections: HashMap<u64, (u64, Rc<Section>)>,
	generation: u64,
}

impl Decorator {
	fn section(&mut self, text: &str, first: bool) -> (u64, Rc<Section>) {
		let mut hasher = DefaultHasher::new();
		(first, text).hash(&mut hasher);
		let key = hasher.finish();
		if let Some((used, section)) = self.sections.get_mut(&key)
			&& section.len == text.len()
		{
			*used = self.generation;
			return (key, section.clone());
		}
		let section = Rc::new(decorate_section(text, first));
		self.sections
			.insert(key, (self.generation, section.clone()));
		(key, section)
	}

	pub fn decorate(&mut self, text: &str) -> Decorations {
		self.generation += 1;
		let cuts = heading_starts(text);
		let mut parts: Vec<(usize, u64, Rc<Section>)> = Vec::new();
		let mut start = 0;
		// Index in `cuts` of the first cut after `start`.
		let mut next = 0;
		let mut whole = cuts.is_empty();
		while !whole && start < text.len() {
			let mut end_ix = next;
			let mut step = 1;
			loop {
				let end = cuts.get(end_ix).copied().unwrap_or(text.len());
				let (key, section) = self.section(&text[start..end], start == 0);
				if section.defines_references {
					whole = true;
					break;
				}
				if section.open_end && end < text.len() {
					// Not a real heading: try further, in growing steps.
					end_ix = (end_ix + step).min(cuts.len());
					step *= 2;
					continue;
				}
				parts.push((start, key, section));
				start = end;
				next = end_ix + 1;
				break;
			}
		}
		let generation = self.generation;
		self.sections.retain(|_, (used, _)| *used == generation);
		if whole {
			let (key, section) = self.section(text, true);
			parts = vec![(0, key, section)];
		}
		assemble(text.len(), &parts)
	}
}

/// Puts the decorations of consecutive sections together, at their place in the note.
fn assemble(len: usize, parts: &[(usize, u64, Rc<Section>)]) -> Decorations {
	let mut out = Decorations::default();
	let shift = |r: &Range<usize>, by: usize| r.start + by..r.end + by;
	for (offset, key, section) in parts {
		let d = &section.decorations;
		let offset = *offset;
		// A section that is not the last ends with a line break: what follows it is the
		// first line of the next section, not an empty last line of this one.
		let last = offset + section.len >= len;
		let lines = d.line_kinds.len() - usize::from(!last);
		let line0 = out.line_kinds.len();
		let style0 = out.styles.len() as u32;
		out.line_kinds.extend_from_slice(&d.line_kinds[..lines]);
		out.line_starts
			.extend(d.line_starts[..lines].iter().map(|s| s + offset));
		out.line_ids.extend((0..lines as u32).map(|i| (*key, i)));
		for line in 0..lines {
			out.line_styles
				.extend(d.styles_of_line(line).iter().map(|i| i + style0));
			out.line_style_ends.push(out.line_styles.len() as u32);
		}
		out.styles
			.extend(d.styles.iter().map(|(r, s)| (shift(r, offset), *s)));
		out.concealed.extend(d.concealed.iter().map(|c| Concealed {
			range: shift(&c.range, offset),
			replacement: c.replacement,
			reveal: shift(&c.reveal, offset),
			line_scope: c.line_scope,
		}));
		out.tasks.extend(d.tasks.iter().map(|t| Task {
			marker: shift(&t.marker, offset),
			checked: t.checked,
		}));
		out.blocks.extend(d.blocks.iter().map(|b| {
			Block {
				kind: match &b.kind {
					BlockKind::Table(table) => BlockKind::Table(Table {
						align: table.align.clone(),
						rows: table
							.rows
							.iter()
							.map(|row| row.iter().map(|cell| shift(cell, offset)).collect())
							.collect(),
						rule_line: table.rule_line + line0,
					}),
					kind => kind.clone(),
				},
				lines: b.lines.start + line0..b.lines.end + line0,
				source: shift(&b.source, offset),
			}
		}));
	}
	out
}

/// Parses a note into what live preview needs: line kinds, styles and concealable markup.
pub fn decorate(text: &str) -> Decorations {
	decorate_section(text, true).decorations
}

/// Parses a note, or a section of it (`first`: it is the start of the note, where
/// frontmatter can be).
fn decorate_section(text: &str, first: bool) -> Section {
	let mut line_starts = vec![0];
	line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
	let line_kinds = text
		.split('\n')
		.map(|l| {
			if l.trim().is_empty() {
				LineKind::Blank
			} else {
				LineKind::Text
			}
		})
		.collect();
	let mut b = Builder {
		text,
		line_starts,
		decorations: Decorations {
			line_kinds,
			..Decorations::default()
		},
		code: Vec::new(),
	};
	let mut options = Options::ENABLE_STRIKETHROUGH
		| Options::ENABLE_TASKLISTS
		| Options::ENABLE_WIKILINKS
		| Options::ENABLE_GFM
		| Options::ENABLE_TABLES;
	// Frontmatter exists only at the very start of a note; elsewhere `---` is a rule.
	let may_have_frontmatter = first && text.starts_with("---");
	let mut has_frontmatter = false;
	let mut last_block = LastBlock::Other;
	if may_have_frontmatter {
		options |= Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
	}
	let mut stack: Vec<Open> = Vec::new();
	let mut table_align: Vec<Align> = Vec::new();
	let mut table_rows: Vec<Vec<Range<usize>>> = Vec::new();
	let expanded = Expanded::new(text);
	let parse_text = expanded.as_ref().map_or(text, |e| e.text.as_str());
	let mut events = Parser::new_ext(parse_text, options).into_offset_iter();
	for (event, range) in events.by_ref() {
		let range = match &expanded {
			Some(e) => e.to_original(range.start)..e.to_original(range.end),
			None => range,
		};
		if stack.is_empty() {
			last_block = match &event {
				Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_))) => {
					LastBlock::Fenced(range.clone())
				}
				Event::Start(Tag::HtmlBlock) => LastBlock::Html,
				Event::Start(_) | Event::Rule => LastBlock::Other,
				_ => last_block,
			};
		}
		match event {
			Event::Start(tag) => {
				note_child(&mut stack, &range);
				let kind = match tag {
					Tag::Heading { level, .. } => {
						for line in b.lines_in(&range) {
							b.decorations.line_kinds[line] =
								LineKind::Heading(heading_number(level));
						}
						OpenKind::Heading
					}
					Tag::BlockQuote(_) => {
						b.quote(&range);
						OpenKind::Other
					}
					Tag::CodeBlock(kind) => {
						let lang = match kind {
							CodeBlockKind::Fenced(info) => {
								Some(info.split_whitespace().next().unwrap_or("").to_string())
							}
							CodeBlockKind::Indented => None,
						};
						b.code_block(&range, lang);
						OpenKind::Other
					}
					Tag::MetadataBlock(_) => {
						if range.start == 0 {
							b.frontmatter(&range);
							has_frontmatter = true;
						}
						OpenKind::Other
					}
					Tag::Table(aligns) => {
						table_align = aligns
							.iter()
							.map(|a| match a {
								Alignment::None => Align::None,
								Alignment::Left => Align::Left,
								Alignment::Center => Align::Center,
								Alignment::Right => Align::Right,
							})
							.collect();
						table_rows.clear();
						OpenKind::Table
					}
					Tag::TableHead | Tag::TableRow => {
						table_rows.push(Vec::new());
						OpenKind::Other
					}
					Tag::TableCell => {
						let cell = b.trim_cell(&range);
						if let Some(row) = table_rows.last_mut() {
							row.push(cell);
						}
						OpenKind::Other
					}
					Tag::Item => {
						b.item(&range);
						OpenKind::Other
					}
					Tag::Strong => OpenKind::Inline(Styles::BOLD),
					Tag::Emphasis => OpenKind::Inline(Styles::ITALIC),
					Tag::Strikethrough => OpenKind::Inline(Styles::STRIKE),
					Tag::Link { .. } => OpenKind::Inline(Styles::LINK),
					_ => OpenKind::Other,
				};
				stack.push(Open {
					kind,
					range,
					first_child: None,
					last_child_end: 0,
				});
			}
			Event::End(_) => {
				if let Some(open) = stack.pop() {
					match open.kind {
						OpenKind::Inline(style) => b.close_inline(&open, style),
						OpenKind::Heading => b.close_heading(&open),
						OpenKind::Table => b.finish_table(
							&open.range,
							std::mem::take(&mut table_align),
							std::mem::take(&mut table_rows),
						),
						OpenKind::Other => {}
					}
				}
			}
			Event::Code(_) => {
				note_child(&mut stack, &range);
				b.inline_code(&range);
			}
			Event::TaskListMarker(checked) => {
				note_child(&mut stack, &range);
				b.decorations.styles.push((range.clone(), Styles::MARKUP));
				b.task(&range, checked);
			}
			Event::Rule => b.rule(&range),
			_ => note_child(&mut stack, &range),
		}
	}
	b.image_lines();
	b.decorations.blocks.sort_by_key(|blk| blk.source.start);
	b.highlights();
	b.decorations
		.concealed
		.sort_by_key(|c| (c.range.start, c.range.end));
	b.decorations.concealed.dedup();
	let mut line_styles = vec![Vec::new(); b.line_starts.len()];
	for (i, (range, _)) in b.decorations.styles.iter().enumerate() {
		for line in b.lines_in(range) {
			line_styles[line].push(i as u32);
		}
	}
	for indices in line_styles {
		b.decorations.line_styles.extend(indices);
		b.decorations
			.line_style_ends
			.push(b.decorations.line_styles.len() as u32);
	}
	b.decorations.line_starts = b.line_starts;
	// `---` on the first line may still find its closing line further down.
	let open_end = (may_have_frontmatter && !has_frontmatter)
		|| match &last_block {
			LastBlock::Fenced(range) => !fence_closed(text, range),
			LastBlock::Html => true,
			LastBlock::Other => false,
		};
	Section {
		decorations: b.decorations,
		len: text.len(),
		open_end,
		defines_references: events.reference_definitions().iter().next().is_some(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn hidden<'a>(t: &'a str, d: &Decorations) -> Vec<&'a str> {
		d.concealed.iter().map(|c| &t[c.range.clone()]).collect()
	}

	fn styled<'a>(t: &'a str, d: &Decorations, s: Styles) -> Vec<&'a str> {
		d.styles
			.iter()
			.filter(|(_, st)| st.contains(s))
			.map(|(r, _)| &t[r.clone()])
			.collect()
	}

	#[test]
	fn bold_markers_concealed_text_styled() {
		let t = "a **été** b";
		let d = decorate(t);
		assert_eq!(hidden(t, &d), ["**", "**"]);
		assert_eq!(styled(t, &d, Styles::BOLD), ["été"]);
		assert!(
			d.concealed
				.iter()
				.all(|c| &t[c.reveal.clone()] == "**été**")
		);
	}

	#[test]
	fn heading_prefix_is_line_scoped() {
		let t = "## Titre\nx";
		let d = decorate(t);
		assert_eq!(d.line_kinds, [LineKind::Heading(2), LineKind::Text]);
		assert_eq!(hidden(t, &d), ["## "]);
		assert!(d.concealed[0].line_scope);
	}

	#[test]
	fn bullet_is_replaced() {
		let t = "- item";
		let d = decorate(t);
		assert_eq!(d.line_kinds, [LineKind::ListItem]);
		assert_eq!(d.concealed[0].range, 0..2);
		assert_eq!(d.concealed[0].replacement, Some("\u{2022} "));
	}

	#[test]
	fn wikilink_shows_alias() {
		let t = "[[Note|Alias]]";
		let d = decorate(t);
		assert_eq!(hidden(t, &d), ["[[Note|", "]]"]);
		assert_eq!(styled(t, &d, Styles::LINK), ["Alias"]);
	}

	#[test]
	fn link_shows_text() {
		let t = "[site](https://x.y)";
		let d = decorate(t);
		assert_eq!(hidden(t, &d), ["[", "](https://x.y)"]);
		assert_eq!(styled(t, &d, Styles::LINK), ["site"]);
	}

	#[test]
	fn highlight_and_strike() {
		let t = "==hi== ~~no~~";
		let d = decorate(t);
		assert_eq!(hidden(t, &d), ["==", "==", "~~", "~~"]);
		assert_eq!(styled(t, &d, Styles::HIGHLIGHT), ["hi"]);
		assert_eq!(styled(t, &d, Styles::STRIKE), ["no"]);
	}

	#[test]
	fn quote_prefix_is_line_scoped() {
		let t = "> cite";
		let d = decorate(t);
		assert_eq!(d.line_kinds, [LineKind::Quote]);
		assert_eq!(hidden(t, &d), ["> "]);
		assert!(d.concealed[0].line_scope);
	}

	#[test]
	fn nothing_styled_inside_code() {
		let t = "```\n**x** ==y==\n```\n`**y**`";
		let d = decorate(t);
		assert_eq!(
			d.line_kinds[..3],
			[LineKind::CodeFence, LineKind::Code, LineKind::CodeFence]
		);
		assert!(styled(t, &d, Styles::BOLD).is_empty());
		assert!(styled(t, &d, Styles::HIGHLIGHT).is_empty());
		assert_eq!(hidden(t, &d), ["`", "`"]);
		assert_eq!(styled(t, &d, Styles::CODE), ["**y**"]);
	}

	#[test]
	fn blank_and_rule_lines() {
		let t = "a\n\n---\n";
		let d = decorate(t);
		assert_eq!(
			d.line_kinds[..3],
			[LineKind::Text, LineKind::Blank, LineKind::Rule]
		);
	}

	#[test]
	fn tab_indented_nested_items_are_list_items() {
		let t = "- p\n\t- a\n\t\t* b";
		let d = decorate(t);
		assert_eq!(
			d.line_kinds,
			[LineKind::ListItem, LineKind::ListItem, LineKind::ListItem]
		);
		assert_eq!(hidden(t, &d), ["- ", "- ", "* "]);
		assert_eq!(d.concealed[1].range, 5..7);
	}

	fn cell<'a>(t: &'a str, r: &std::ops::Range<usize>) -> &'a str {
		&t[r.clone()]
	}

	#[test]
	fn code_block_has_its_language() {
		let d = decorate("```python\nx\n```");
		assert_eq!(d.blocks.len(), 1);
		assert_eq!(
			d.blocks[0].kind,
			BlockKind::Code {
				lang: "python".into()
			}
		);
		assert_eq!(d.blocks[0].lines, 0..3);
	}

	#[test]
	fn table_cells_and_alignment() {
		let t = "| a | b |\n|:--|--:|\n| \u{e9} | 2 |";
		let d = decorate(t);
		let BlockKind::Table(table) = &d.blocks[0].kind else {
			panic!("not a table: {:?}", d.blocks);
		};
		assert_eq!(table.align, [Align::Left, Align::Right]);
		assert_eq!(table.rule_line, 1);
		let cells: Vec<Vec<&str>> = table
			.rows
			.iter()
			.map(|r| r.iter().map(|c| cell(t, c)).collect())
			.collect();
		assert_eq!(cells, [vec!["a", "b"], vec!["\u{e9}", "2"]]);
		assert_eq!(d.line_kinds, [LineKind::Table; 3]);
	}

	#[test]
	fn callout_title_and_markers() {
		let t = "> [!warning] Attention\n> body";
		let d = decorate(t);
		assert_eq!(
			d.blocks[0].kind,
			BlockKind::Callout {
				kind: "warning".into(),
				title: "Attention".into()
			}
		);
		assert_eq!(hidden(t, &d), ["> [!warning] ", "> "]);
		assert_eq!(d.line_kinds, [LineKind::Callout, LineKind::Callout]);
		let d = decorate("> [!tip]");
		assert_eq!(
			d.blocks[0].kind,
			BlockKind::Callout {
				kind: "tip".into(),
				title: "Tip".into()
			}
		);
	}

	#[test]
	fn tasks_are_concealed_and_checked_text_is_done() {
		let t = "- [ ] a\n- [x] b";
		let d = decorate(t);
		assert_eq!(d.tasks.len(), 2);
		assert_eq!(
			(d.tasks[0].marker.clone(), d.tasks[0].checked),
			(2..5, false)
		);
		assert_eq!(
			(d.tasks[1].marker.clone(), d.tasks[1].checked),
			(10..13, true)
		);
		assert_eq!(hidden(t, &d), ["- [ ] ", "- [x] "]);
		assert!(d.concealed.iter().all(|c| c.replacement == Some(TASK_GAP)));
		assert_eq!(styled(t, &d, Styles::DONE), ["b"]);
	}

	#[test]
	fn image_embed_lines_are_blocks() {
		let d = decorate("![[img.png|300]]\n![[Note]]\n![alt](pics/a.jpg)");
		let kinds: Vec<_> = d.blocks.iter().map(|b| b.kind.clone()).collect();
		assert_eq!(
			kinds,
			[
				BlockKind::Image {
					target: "img.png".into(),
					width: Some(300)
				},
				BlockKind::Embed {
					target: "Note".into(),
					size: None
				},
				BlockKind::Image {
					target: "pics/a.jpg".into(),
					width: None
				},
			]
		);
		assert_eq!(d.blocks[2].lines, 2..3);
	}

	#[test]
	fn frontmatter_block() {
		let d = decorate("---\na: 1\n---\nx");
		assert_eq!(d.blocks[0].kind, BlockKind::Frontmatter);
		assert_eq!(d.line_kinds[..3], [LineKind::Frontmatter; 3]);
		assert_eq!(d.line_kinds[3], LineKind::Text);
	}

	#[test]
	fn frontmatter_only_at_the_document_start() {
		for t in ["a\n\n---\nb\n\n---", "# T\n\n---\nSection two\n---\n\nbody"] {
			let d = decorate(t);
			assert!(
				d.blocks.iter().all(|b| b.kind != BlockKind::Frontmatter),
				"{t:?}"
			);
			assert!(!d.line_kinds.contains(&LineKind::Frontmatter), "{t:?}");
		}
	}

	#[test]
	fn closing_fence_inside_a_quote_is_a_fence() {
		let d = decorate("> ```py\n> x = 1\n> ```\nz");
		assert_eq!(d.line_kinds[0], LineKind::CodeFence);
		assert_eq!(d.line_kinds[1], LineKind::Code);
		assert_eq!(d.line_kinds[2], LineKind::CodeFence);
	}

	#[test]
	fn quote_prefixes_of_code_lines() {
		assert_eq!(quote_depth("> > ```py"), 2);
		assert_eq!(quote_depth("```py"), 0);
		assert_eq!(quote_prefix_len("> x = 1", 1), 2);
		assert_eq!(quote_prefix_len(">  x", 1), 2);
		assert_eq!(quote_prefix_len("> > x", 2), 4);
		assert_eq!(quote_prefix_len("> > x", 1), 2);
		assert_eq!(quote_prefix_len("x", 1), 0);
		assert_eq!(quote_prefix_len("> x", 0), 0);
	}

	#[test]
	fn markdown_image_paths_are_decoded() {
		let target = |t: &str| match &decorate(t).blocks[..] {
			[
				Block {
					kind: BlockKind::Image { target, .. },
					..
				},
			] => target.clone(),
			other => panic!("{t:?}: {other:?}"),
		};
		assert_eq!(target("![](Pasted%20image%201.png)"), "Pasted image 1.png");
		assert_eq!(target("![a](<my pic.png>)"), "my pic.png");
		assert_eq!(target("![a](p.png \"title\")"), "p.png");
		assert_eq!(target("![a](%C3%A9t%C3%A9.png)"), "\u{e9}t\u{e9}.png");
		assert_eq!(target("![a](100%.png)"), "100%.png");
	}

	#[test]
	fn blocks_reveal_when_a_caret_touches_them() {
		let t = "a\n```\nx\n```\nb";
		let d = decorate(t);
		let code = &d.blocks[0];
		assert!(!block_revealed(
			code,
			&[crate::editor_state::Selection::caret(0)]
		));
		assert!(block_revealed(
			code,
			&[crate::editor_state::Selection::caret(6)]
		));
	}

	#[test]
	fn drawing_embed_lines_are_blocks() {
		let d = decorate(
			"![[Plan]]
![[a/Plan.excalidraw|300]]
![[Note#Part]]
![[x.pdf]]
text ![[Plan]]",
		);
		let kinds: Vec<_> = d.blocks.iter().map(|b| b.kind.clone()).collect();
		assert_eq!(
			kinds,
			[
				BlockKind::Embed {
					target: "Plan".into(),
					size: None
				},
				BlockKind::Embed {
					target: "a/Plan.excalidraw".into(),
					size: Some(300)
				},
			]
		);
		assert_eq!(d.blocks[1].lines, 1..2);
	}

	/// Notes that exercise what could differ when sections are parsed on their own.
	fn tricky_notes() -> Vec<String> {
		let plain = "# One\n\ntext **bold** and [[link]]\n\n## Two\n- a\n- [x] b\n\t- nested\n\n# Three\n> [!tip] T\n> body\n";
		vec![
			String::new(),
			"no heading at all\n\n- item".to_string(),
			plain.to_string(),
			// No final newline, and a heading on the very last line.
			"# A\nx\n# B".to_string(),
			// `#` lines inside a fence are code, also when the fence never closes.
			"# Script\n```bash\n# comment\necho 1\n# another\n```\n# After\ntext".to_string(),
			"# Script\n```bash\n# comment\n# another\n\n# still code\n".to_string(),
			"# A\n~~~\n# x\n```\n# y\n~~~\n# B\n".to_string(),
			// A fence inside a list item or a quote ends with it.
			"# A\n- item\n  ```\n  code\n# B\n```\n# C\n```\n# D\n".to_string(),
			"# A\n> ```\n> # quoted code\n# B\ntext\n".to_string(),
			// Frontmatter, closed and not.
			"---\ntitle: x\n# not a heading\n---\n# Real\ntext\n".to_string(),
			"---\ntitle: x\n# heading after a rule\ntext\n".to_string(),
			// Raw HTML keeps the lines that follow it.
			"# A\n<div>\n# in html\n\n# B\n<!--\n# hidden\n\n# still hidden\n-->\n# C\n".to_string(),
			// A reference defined in one section is used in another.
			"# A\nsee [the site][s]\n# B\n[s]: https://example.org\n".to_string(),
			// Tables, rules, highlights and tasks next to section starts.
			"# A\n| a | b |\n|---|:-:|\n| 1 | 2 |\n# B\n---\n==hi== ~~no~~\n#tag is not a heading\n#\n####### seven\n".to_string(),
			"text first\n# A\n![[img.png]]\n![[Plan]]\n# B\n```rust\nlet a = 1;\n```\n".to_string(),
		]
	}

	fn assert_same(text: &str, mut chunked: Decorations, context: &str) {
		assert_eq!(
			chunked.line_ids.len(),
			chunked.line_kinds.len(),
			"{context}"
		);
		chunked.line_ids.clear();
		assert_eq!(chunked, decorate(text), "{context}: {text:?}");
	}

	#[test]
	fn sections_parsed_alone_give_the_same_decorations() {
		for text in tricky_notes() {
			assert_same(&text, Decorator::default().decorate(&text), "fresh");
		}
	}

	#[test]
	fn decorator_reuses_sections_across_edits() {
		let mut decorator = Decorator::default();
		let notes = tricky_notes();
		// One decorator sees every note in turn, then each note with a character typed at
		// every line start and line end: stale sections must never leak into a result.
		for text in &notes {
			assert_same(text, decorator.decorate(text), "sequence");
		}
		for text in &notes {
			let mut cuts: Vec<usize> = text.match_indices('\n').map(|(i, _)| i).collect();
			cuts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
			cuts.push(0);
			for at in cuts {
				for typed in ["x", "#", "`", "\n"] {
					let mut edited = text.clone();
					edited.insert_str(at, typed);
					assert_same(&edited, decorator.decorate(&edited), "edit");
				}
			}
		}
	}

	#[test]
	fn unchanged_sections_keep_their_line_ids() {
		let mut decorator = Decorator::default();
		let before = decorator.decorate("# A\none\n# B\ntwo\n# C\nthree");
		let after = decorator.decorate("# A\none\n# B\ntwo!\n# C\nthree");
		assert_eq!(before.line_ids[..2], after.line_ids[..2]);
		assert_ne!(before.line_ids[2], after.line_ids[2], "the edited section");
		assert_eq!(before.line_ids[4..], after.line_ids[4..]);
		assert_eq!(after.line_ids[3].1, 1, "line index inside its section");
		// The same text at the start of the note and elsewhere is not the same section.
		let twice = decorator.decorate("# A\none\n# A\none\n");
		assert_ne!(twice.line_ids[0].0, twice.line_ids[2].0);
	}

	#[test]
	fn heading_lines_are_the_only_section_starts() {
		let text = "# a\n#b\n## c\n ## d\n####### e\n#\n#\tf\nx # g\n";
		let starts: Vec<&str> = heading_starts(text)
			.into_iter()
			.map(|at| text[at..].lines().next().unwrap())
			.collect();
		assert_eq!(starts, ["## c", "#", "#\tf"]);
	}
}
