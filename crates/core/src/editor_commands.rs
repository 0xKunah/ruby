//! Editing commands on top of `EditorState`: multi-cursor, line operations and Markdown-aware
//! typing (lists, wrapping, links, auto-pairs).

use std::ops::Range;

use crate::editor_state::{EditorState, Selection};

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
	a.start < b.end && b.start < a.end
}

impl EditorState {
	/// Adds a caret or selection; it becomes the primary one.
	pub fn add_selection(&mut self, selection: Selection) {
		let mut all = self.selections().to_vec();
		all.push(selection);
		self.set_selections(all);
	}

	/// The primary selection's text, or the word under an empty primary selection.
	fn primary_needle(&self) -> Option<Range<usize>> {
		let p = self.primary();
		if p.is_empty() {
			self.word_at_caret(p.head)
		} else {
			Some(p.range())
		}
	}

	/// The word under the caret, or the one ending right at it; never spaces or punctuation.
	fn word_at_caret(&self, head: usize) -> Option<Range<usize>> {
		let text = self.text();
		let is_word = |r: &Range<usize>| {
			text[r.clone()]
				.chars()
				.next()
				.is_some_and(|c| c.is_alphanumeric() || c == '_')
		};
		let under = self.word_range_at(head);
		if is_word(&under) {
			return Some(under);
		}
		let prev = text[..head].char_indices().next_back()?.0;
		let before = self.word_range_at(prev);
		(is_word(&before) && before.end == head).then_some(before)
	}

	/// Ctrl+D: select the word under an empty primary caret, otherwise add the next occurrence of
	/// the primary selection's text (wrapping around, skipping ones already selected).
	pub fn add_next_occurrence(&mut self) {
		let p = self.primary();
		let Some(range) = self.primary_needle() else {
			return;
		};
		if p.is_empty() {
			let mut all: Vec<Selection> = self
				.selections()
				.iter()
				.filter(|s| **s != p)
				.copied()
				.collect();
			all.push(Selection {
				anchor: range.start,
				head: range.end,
			});
			self.set_selections(all);
			return;
		}
		let text = self.text();
		let needle = &text[range.clone()];
		let from = range.end;
		let after = text[from..].match_indices(needle).map(|(i, _)| from + i);
		let before = text[..from].match_indices(needle).map(|(i, _)| i);
		let found = after
			.chain(before)
			.map(|at| at..at + needle.len())
			.find(|r| {
				!self
					.selections()
					.iter()
					.any(|s| overlaps(&s.range(), r) || s.range() == *r)
			});
		if let Some(r) = found {
			self.add_selection(Selection {
				anchor: r.start,
				head: r.end,
			});
		}
	}

	/// Ctrl+Shift+L: select every occurrence of the primary selection's text (or word).
	pub fn select_all_occurrences(&mut self) {
		let Some(range) = self.primary_needle() else {
			return;
		};
		let text = self.text();
		let needle = &text[range.clone()];
		let mut all: Vec<Selection> = text
			.match_indices(needle)
			.map(|(i, _)| Selection {
				anchor: i,
				head: i + needle.len(),
			})
			.filter(|s| s.range() != range)
			.collect();
		all.push(Selection {
			anchor: range.start,
			head: range.end,
		});
		self.set_selections(all);
	}

	/// Click on a task checkbox: `[ ]` <-> `[x]` at `marker` (one undo step, selections kept).
	pub fn toggle_task(&mut self, marker: Range<usize>) {
		let Some(current) = self.text().get(marker.clone()) else {
			return;
		};
		let next = match current {
			"[ ]" => "[x]",
			"[x]" | "[X]" => "[ ]",
			_ => return,
		};
		let after = self.selections().to_vec();
		self.replace_with(vec![(marker, next.to_string())], after);
	}

	/// Esc: keep only the primary selection.
	pub fn collapse_to_primary(&mut self) {
		let p = self.primary();
		self.set_selections(vec![p]);
	}

	/// Byte ranges (without the final `\n`) of the whole lines touched by the selections,
	/// merged when they overlap or touch.
	fn line_blocks(&self) -> Vec<Range<usize>> {
		let text = self.text();
		let mut blocks: Vec<Range<usize>> = Vec::new();
		for sel in self.selections() {
			let r = sel.range();
			let end = if !r.is_empty() && text[..r.end].ends_with('\n') {
				r.end - 1
			} else {
				r.end
			};
			let block = self.line_start(r.start)..self.line_end(end.max(r.start));
			match blocks.last_mut() {
				Some(last) if block.start <= last.end + 1 => last.end = last.end.max(block.end),
				_ => blocks.push(block),
			}
		}
		blocks
	}

	/// Selections moved with their block: `shift(i)` inside block `i`, plus the text length
	/// change (`deltas`) of the blocks before it.
	fn shifted_selections(
		&self,
		blocks: &[Range<usize>],
		deltas: &[isize],
		shift: impl Fn(usize) -> isize,
	) -> Vec<Selection> {
		self.selections()
			.iter()
			.map(|sel| {
				let i = blocks
					.iter()
					.position(|b| sel.range().start <= b.end)
					.unwrap_or(blocks.len() - 1);
				let by = deltas[..i].iter().sum::<isize>() + shift(i);
				let move_by = |o: usize| (o as isize + by).max(0) as usize;
				Selection {
					anchor: move_by(sel.anchor),
					head: move_by(sel.head),
				}
			})
			.collect()
	}

	/// Alt+Up / Alt+Down: swap the selected lines with the line above or below.
	pub fn move_lines(&mut self, down: bool) {
		let blocks = self.line_blocks();
		let text = self.text();
		let len = text.len();
		if (down && blocks.iter().any(|b| b.end == len))
			|| (!down && blocks.iter().any(|b| b.start == 0))
		{
			return;
		}
		let mut edits = Vec::new();
		let mut shifts = Vec::new();
		for b in &blocks {
			let block_text = &text[b.clone()];
			if down {
				let next = b.end + 1..self.line_end(b.end + 1);
				edits.push((
					b.start..next.end,
					format!("{}\n{}", &text[next.clone()], block_text),
				));
				shifts.push(next.len() as isize + 1);
			} else {
				let prev = self.line_start(b.start - 1)..b.start - 1;
				edits.push((
					prev.start..b.end,
					format!("{}\n{}", block_text, &text[prev.clone()]),
				));
				shifts.push(-(prev.len() as isize + 1));
			}
		}
		let after = self.shifted_selections(&blocks, &vec![0; blocks.len()], |i| shifts[i]);
		self.replace_with(edits, after);
	}

	/// Shift+Alt+Up / Down: duplicate the selected lines; down moves the carets to the copy.
	pub fn copy_lines(&mut self, down: bool) {
		let blocks = self.line_blocks();
		let text = self.text();
		let edits: Vec<_> = blocks
			.iter()
			.map(|b| (b.end..b.end, format!("\n{}", &text[b.clone()])))
			.collect();
		let deltas: Vec<isize> = blocks.iter().map(|b| b.len() as isize + 1).collect();
		let after = self.shifted_selections(&blocks, &deltas, |i| if down { deltas[i] } else { 0 });
		self.replace_with(edits, after);
	}

	/// Ctrl+Shift+K: delete the selected lines.
	pub fn delete_lines(&mut self) {
		let blocks = self.line_blocks();
		let len = self.text().len();
		let mut edits = Vec::new();
		let mut carets = Vec::new();
		let mut removed_before = 0;
		for b in &blocks {
			let (range, caret) = if b.end < len {
				(b.start..b.end + 1, b.start)
			} else if b.start > 0 {
				(b.start - 1..b.end, self.line_start(b.start - 1))
			} else {
				(b.clone(), 0)
			};
			carets.push(Selection::caret(caret - removed_before.min(caret)));
			removed_before += range.len();
			edits.push((range, String::new()));
		}
		self.replace_with(edits, carets);
	}

	/// Ctrl+Enter (below) / Ctrl+Shift+Enter (above): new line with the current indentation.
	pub fn insert_line(&mut self, below: bool) {
		let text = self.text();
		let mut lines: Vec<usize> = self
			.selections()
			.iter()
			.map(|s| self.line_start(s.head))
			.collect();
		lines.dedup();
		let mut edits = Vec::new();
		let mut carets = Vec::new();
		let mut added = 0;
		for start in lines {
			let end = self.line_end(start);
			let indent: String = text[start..end]
				.chars()
				.take_while(|c| *c == ' ' || *c == '\t')
				.collect();
			let (at, insert, caret) = if below {
				(end, format!("\n{indent}"), end + 1 + indent.len())
			} else {
				(start, format!("{indent}\n"), start + indent.len())
			};
			carets.push(Selection::caret(caret + added));
			added += insert.len();
			edits.push((at..at, insert));
		}
		self.replace_with(edits, carets);
	}

	/// Ctrl+L: select whole lines; repeating extends the selection by one line.
	pub fn select_lines(&mut self) {
		let len = self.text().len();
		let selections = self
			.selections()
			.iter()
			.map(|sel| {
				let r = sel.range();
				let end = if !r.is_empty() && self.text()[..r.end].ends_with('\n') {
					r.end - 1
				} else {
					r.end
				};
				let start = self.line_start(r.start);
				let mut head = (self.line_end(end.max(r.start)) + 1).min(len);
				if sel.anchor == start && sel.head == head && head < len {
					head = (self.line_end(head) + 1).min(len);
				}
				Selection {
					anchor: start,
					head,
				}
			})
			.collect();
		self.set_selections(selections);
	}

	/// Ctrl+] / Ctrl+[: add or remove one level of indentation (a tab, or up to 4 spaces) on
	/// every selected line.
	pub fn indent_lines(&mut self, outdent: bool) {
		let text = self.text();
		let mut edits: Vec<(Range<usize>, String)> = Vec::new();
		for b in self.line_blocks() {
			let mut start = b.start;
			loop {
				let line = &text[start..self.line_end(start)];
				if outdent {
					let n = if line.starts_with('\t') {
						1
					} else {
						line.chars().take(4).take_while(|c| *c == ' ').count()
					};
					if n > 0 {
						edits.push((start..start + n, String::new()));
					}
				} else {
					edits.push((start..start, "\t".to_string()));
				}
				let end = self.line_end(start);
				if end >= b.end {
					break;
				}
				start = end + 1;
			}
		}
		let after = self
			.selections()
			.iter()
			.map(|s| Selection {
				anchor: map_through(&edits, s.anchor),
				head: map_through(&edits, s.head),
			})
			.collect();
		self.replace_with(edits, after);
	}
}

/// Where `offset` lands after the edits (sorted, disjoint): text inserted at the offset pushes it
/// forward, a removed range containing it pulls it to the range start.
fn map_through(edits: &[(Range<usize>, String)], offset: usize) -> usize {
	let mut delta: isize = 0;
	for (range, insert) in edits {
		if range.start > offset {
			break;
		}
		if offset < range.end {
			return (range.start as isize + delta) as usize;
		}
		delta += insert.len() as isize - range.len() as isize;
	}
	(offset as isize + delta) as usize
}

/// A list or quote line prefix: indentation, then the marker as typed.
struct LinePrefix {
	indent: String,
	/// `- `, `* `, `+ `, `3. `, `3) `, `> `, including a task box (`- [x] `).
	marker: String,
	/// What Enter puts on the next line after the indentation.
	next: String,
}

fn line_prefix(line: &str) -> Option<LinePrefix> {
	let indent: String = line
		.chars()
		.take_while(|c| *c == ' ' || *c == '\t')
		.collect();
	let rest = &line[indent.len()..];
	let (marker, next) = if let Some(q) = rest.strip_prefix('>') {
		let space = if q.starts_with(' ') { " " } else { "" };
		(format!(">{space}"), "> ".to_string())
	} else if let Some(bullet) = rest.chars().next().filter(|c| matches!(c, '-' | '*' | '+')) {
		let after = rest.get(1..)?;
		if !after.starts_with(' ') {
			return None;
		}
		let task = after[1..].starts_with("[ ] ")
			|| after[1..].starts_with("[x] ")
			|| after[1..].starts_with("[X] ");
		if task {
			(
				format!("{bullet} {}", &after[1..5]),
				format!("{bullet} [ ] "),
			)
		} else {
			(format!("{bullet} "), format!("{bullet} "))
		}
	} else {
		let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
		let delim = rest[digits.len()..].chars().next()?;
		if digits.is_empty()
			|| !matches!(delim, '.' | ')')
			|| !rest[digits.len() + 1..].starts_with(' ')
		{
			return None;
		}
		let n: u64 = digits.parse().ok()?;
		(format!("{digits}{delim} "), format!("{}{delim} ", n + 1))
	};
	Some(LinePrefix {
		indent,
		marker,
		next,
	})
}

/// Whether the line starting at `line_start` is inside a fenced code block (``` or ~~~).
fn in_fenced_code(text: &str, line_start: usize) -> bool {
	let mut fence: Option<char> = None;
	for line in text[..line_start].split('\n') {
		let trimmed = line.trim_start();
		if line.len() - trimmed.len() > 3 {
			continue;
		}
		let marker = if trimmed.starts_with("```") {
			'`'
		} else if trimmed.starts_with("~~~") {
			'~'
		} else {
			continue;
		};
		fence = match fence {
			None => Some(marker),
			Some(open) if open == marker => None,
			other => other,
		};
	}
	fence.is_some()
}

fn leading_whitespace(line: &str) -> &str {
	&line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

fn is_url(s: &str) -> bool {
	(s.starts_with("http://") || s.starts_with("https://")) && !s.contains(char::is_whitespace)
}

impl EditorState {
	/// One replacement per selection: `f` returns the source range, its new text, and the new
	/// selection as offsets from the range start in the new text; `None` leaves that selection.
	fn edit_each(
		&mut self,
		f: impl Fn(&EditorState, Selection) -> Option<(Range<usize>, String, usize, usize)>,
	) {
		let mut edits: Vec<(Range<usize>, String)> = Vec::new();
		let mut after = Vec::new();
		let mut delta: isize = 0;
		for sel in self.selections().to_vec() {
			// An edit overlapping the previous one (two carets on one emptied item) is dropped;
			// its selection just follows the text.
			let edit = f(self, sel)
				.filter(|(range, ..)| edits.last().is_none_or(|(prev, _)| range.start >= prev.end));
			match edit {
				Some((range, insert, anchor, head)) => {
					let start = (range.start as isize + delta) as usize;
					after.push(Selection {
						anchor: start + anchor,
						head: start + head,
					});
					delta += insert.len() as isize - range.len() as isize;
					edits.push((range, insert));
				}
				None => after.push(Selection {
					anchor: (sel.anchor as isize + delta).max(0) as usize,
					head: (sel.head as isize + delta).max(0) as usize,
				}),
			}
		}
		self.replace_with(edits, after);
	}

	/// Enter: continue lists, tasks and quotes; on an empty item, remove its marker instead.
	pub fn newline(&mut self) {
		if self.selections().iter().any(|s| !s.is_empty()) {
			self.insert("\n");
			return;
		}
		self.edit_each(|s, sel| {
			let start = s.line_start(sel.head);
			let end = s.line_end(sel.head);
			let line = &s.text()[start..end];
			if in_fenced_code(s.text(), start) {
				// Code: keep the indentation, no list continuation.
				let insert = format!("\n{}", leading_whitespace(line));
				let len = insert.len();
				return Some((sel.head..sel.head, insert, len, len));
			}
			let prefix =
				line_prefix(line).filter(|p| sel.head >= start + p.indent.len() + p.marker.len());
			Some(match prefix {
				Some(p) if line[p.indent.len() + p.marker.len()..].trim().is_empty() => {
					(start..end, String::new(), 0, 0)
				}
				Some(p) => {
					let insert = format!("\n{}{}", p.indent, p.next);
					let len = insert.len();
					(sel.head..sel.head, insert, len, len)
				}
				None => (sel.head..sel.head, "\n".to_string(), 1, 1),
			})
		});
	}

	/// Tab / Shift+Tab: indent or outdent list items and multi-line selections; elsewhere Tab
	/// inserts a tab character.
	pub fn tab(&mut self, outdent: bool) {
		let text = self.text();
		let structural = self.selections().iter().any(|sel| {
			let r = sel.range();
			let start = self.line_start(r.start);
			text[r.clone()].contains('\n')
				|| (!in_fenced_code(text, start)
					&& line_prefix(&text[start..self.line_end(r.start)]).is_some())
		});
		if outdent || structural {
			self.indent_lines(outdent);
		} else {
			self.insert("\t");
		}
	}

	/// Ctrl+B (`**`) / Ctrl+I (`*`): wrap each selection, or unwrap it if already wrapped; an
	/// empty selection gets an empty pair with the caret inside.
	pub fn toggle_wrap(&mut self, marker: &str) {
		let m = marker.len();
		self.edit_each(|s, sel| {
			let r = sel.range();
			let text = s.text();
			let inner = &text[r.clone()];
			if r.is_empty() {
				return Some((r, format!("{marker}{marker}"), m, m));
			}
			// Asterisk markers count runs: `*` (italic) only unwraps an odd run, so Ctrl+I on
			// a bold word (`**x**`) adds italic instead of removing the bold.
			let stars = marker.chars().all(|c| c == '*');
			let runs_allow = |before: usize, after: usize| {
				before >= m && after >= m && (m != 1 || (before % 2 == 1 && after % 2 == 1))
			};
			let lead = |s: &str| s.chars().take_while(|c| *c == '*').count();
			let trail = |s: &str| s.chars().rev().take_while(|c| *c == '*').count();
			let around = if stars {
				runs_allow(trail(&text[..r.start]), lead(&text[r.end..]))
			} else {
				r.start >= m
					&& text[..r.start].ends_with(marker)
					&& text[r.end..].starts_with(marker)
			};
			if around {
				return Some((r.start - m..r.end + m, inner.to_string(), 0, inner.len()));
			}
			let inside = if stars {
				inner.len() >= 2 * m && runs_allow(lead(inner), trail(inner))
			} else {
				inner.len() >= 2 * m && inner.starts_with(marker) && inner.ends_with(marker)
			};
			if inside {
				let unwrapped = inner[m..inner.len() - m].to_string();
				let len = unwrapped.len();
				return Some((r, unwrapped, 0, len));
			}
			Some((r, format!("{marker}{inner}{marker}"), m, m + inner.len()))
		});
	}

	/// Ctrl+K: `[selection]()` with the caret in the parentheses, or `[]()` with it in the brackets.
	pub fn insert_link(&mut self) {
		self.edit_each(|s, sel| {
			let r = sel.range();
			let inner = &s.text()[r.clone()];
			let caret = if inner.is_empty() { 1 } else { inner.len() + 3 };
			Some((r.clone(), format!("[{inner}]()"), caret, caret))
		});
	}

	/// A typed character: auto-pairs `(`, `[`, backtick; steps over a closing char already
	/// there; with a selection, `* _ = ~ ( [` and backtick wrap it.
	pub fn type_char(&mut self, c: char) {
		let pair = |c: char| match c {
			'(' => Some(("(", ")")),
			'[' => Some(("[", "]")),
			'`' => Some(("`", "`")),
			_ => None,
		};
		let wrap = |c: char| match c {
			'*' => Some(("*", "*")),
			'_' => Some(("_", "_")),
			'=' => Some(("==", "==")),
			'~' => Some(("~~", "~~")),
			other => pair(other),
		};
		let all_empty = self.selections().iter().all(Selection::is_empty);
		let text = self.text();
		if all_empty
			&& matches!(c, ')' | ']' | '`')
			&& self
				.selections()
				.iter()
				.all(|s| text[s.head..].starts_with(c))
		{
			self.move_by(crate::editor_state::Motion::Right, false);
			return;
		}
		// No pairing right after a backtick: typing ``` must give a code fence.
		let after_backtick = c == '`'
			&& self
				.selections()
				.iter()
				.any(|s| text[..s.head].ends_with('`'));
		if all_empty
			&& !after_backtick
			&& let Some((open, close)) = pair(c)
		{
			self.edit_each(|_, sel| {
				Some((
					sel.head..sel.head,
					format!("{open}{close}"),
					open.len(),
					open.len(),
				))
			});
			return;
		}
		if !all_empty && let Some((open, close)) = wrap(c) {
			self.edit_each(|s, sel| {
				let r = sel.range();
				let inner = &s.text()[r.clone()];
				Some((
					r,
					format!("{open}{inner}{close}"),
					open.len(),
					open.len() + inner.len(),
				))
			});
			return;
		}
		let mut buf = [0u8; 4];
		self.insert(c.encode_utf8(&mut buf));
	}

	/// Paste: a URL over a one-line selection becomes a Markdown link; otherwise a normal paste.
	pub fn paste_text(&mut self, s: &str, full_line: bool) {
		let single = self.selections().len() == 1;
		let p = self.primary();
		let selected = &self.text()[p.range()];
		if single && !p.is_empty() && !selected.contains('\n') && is_url(s.trim()) {
			let link = format!("[{selected}]({})", s.trim());
			let len = link.len();
			self.edit_each(|_, sel| Some((sel.range(), link.clone(), len, len)));
			return;
		}
		self.paste(s, full_line);
	}
}

#[cfg(test)]
mod tests {
	use crate::editor_state::{EditorState, Selection};

	fn sel(a: usize, h: usize) -> Selection {
		Selection { anchor: a, head: h }
	}

	#[test]
	fn ctrl_d_selects_word_then_next_occurrences() {
		let mut s = EditorState::new("foo bar foo".into());
		s.add_next_occurrence();
		assert_eq!(s.selections(), [sel(0, 3)]);
		s.add_next_occurrence();
		assert_eq!(s.selections(), [sel(0, 3), sel(8, 11)]);
		let v = s.version();
		s.add_next_occurrence();
		assert_eq!(s.selections().len(), 2);
		assert_eq!(s.version(), v);
		s.insert("x");
		assert_eq!(s.text(), "x bar x");
	}

	#[test]
	fn select_all_occurrences_and_collapse() {
		let mut s = EditorState::new("ab ab ab".into());
		s.set_selections(vec![Selection::caret(4)]);
		s.select_all_occurrences();
		assert_eq!(s.selections(), [sel(0, 2), sel(3, 5), sel(6, 8)]);
		assert_eq!(s.primary(), sel(3, 5));
		s.collapse_to_primary();
		assert_eq!(s.selections(), [sel(3, 5)]);
	}

	#[test]
	fn added_cursor_becomes_primary() {
		let mut s = EditorState::new("a\nb\nc".into());
		s.set_selections(vec![Selection::caret(4)]);
		s.add_selection(Selection::caret(0));
		assert_eq!(s.selections().len(), 2);
		assert_eq!(s.primary(), Selection::caret(0));
		s.insert("-");
		assert_eq!(s.primary(), Selection::caret(1));
	}

	fn at(text: &str, offset: usize) -> EditorState {
		let mut s = EditorState::new(text.into());
		s.set_selections(vec![Selection::caret(offset)]);
		s
	}

	#[test]
	fn move_lines_up_and_down() {
		let mut s = at("a\nb\nc", 2);
		s.move_lines(false);
		assert_eq!(s.text(), "b\na\nc");
		assert_eq!(s.selections(), [Selection::caret(0)]);
		s.move_lines(true);
		assert_eq!(s.text(), "a\nb\nc");
		assert_eq!(s.selections(), [Selection::caret(2)]);
		assert!(s.undo());
		assert_eq!(s.text(), "b\na\nc");
	}

	#[test]
	fn moving_past_the_edge_is_a_no_op() {
		let mut s = at("a\nb", 2);
		let v = s.version();
		s.move_lines(true);
		assert_eq!(s.text(), "a\nb");
		assert_eq!(s.version(), v);
	}

	#[test]
	fn carets_on_one_line_move_it_once() {
		let mut s = EditorState::new("a\nbc\nd".into());
		s.set_selections(vec![Selection::caret(2), Selection::caret(3)]);
		s.move_lines(false);
		assert_eq!(s.text(), "bc\na\nd");
	}

	#[test]
	fn copy_lines_down_moves_the_caret_to_the_copy() {
		let mut s = at("a\nb", 0);
		s.copy_lines(true);
		assert_eq!(s.text(), "a\na\nb");
		assert_eq!(s.selections(), [Selection::caret(2)]);
		let mut s = at("a\nb", 0);
		s.copy_lines(false);
		assert_eq!(s.text(), "a\na\nb");
		assert_eq!(s.selections(), [Selection::caret(0)]);
	}

	#[test]
	fn delete_lines() {
		let mut s = at("x", 0);
		s.delete_lines();
		assert_eq!(s.text(), "");
		let mut s = at("a\nb\nc", 2);
		s.delete_lines();
		assert_eq!(s.text(), "a\nc");
		let mut s = at("a\nb", 2);
		s.delete_lines();
		assert_eq!(s.text(), "a");
	}

	#[test]
	fn insert_line_keeps_indentation() {
		let mut s = at("\t- a", 4);
		s.insert_line(true);
		assert_eq!(s.text(), "\t- a\n\t");
		assert_eq!(s.selections(), [Selection::caret(6)]);
		let mut s = at("\tx", 2);
		s.insert_line(false);
		assert_eq!(s.text(), "\t\n\tx");
		assert_eq!(s.selections(), [Selection::caret(1)]);
	}

	#[test]
	fn select_lines_extends_on_repeat() {
		let mut s = at("a\nb\nc", 0);
		s.select_lines();
		assert_eq!(s.selections(), [Selection { anchor: 0, head: 2 }]);
		s.select_lines();
		assert_eq!(s.selections(), [Selection { anchor: 0, head: 4 }]);
	}

	#[test]
	fn indent_then_outdent_round_trips() {
		let mut s = EditorState::new("a\nb".into());
		s.select_all();
		s.indent_lines(false);
		assert_eq!(s.text(), "\ta\n\tb");
		s.indent_lines(true);
		assert_eq!(s.text(), "a\nb");
		let mut s = at("    x", 5);
		s.indent_lines(true);
		assert_eq!(s.text(), "x");
		assert_eq!(s.selections(), [Selection::caret(1)]);
	}

	fn after_enter(text: &str) -> String {
		let mut s = at(text, text.len());
		s.newline();
		s.text().to_string()
	}

	#[test]
	fn enter_continues_lists_and_quotes() {
		assert_eq!(after_enter("\t- a"), "\t- a\n\t- ");
		assert_eq!(after_enter("3. x"), "3. x\n4. ");
		assert_eq!(after_enter("- [x] t"), "- [x] t\n- [ ] ");
		assert_eq!(after_enter("> q"), "> q\n> ");
		assert_eq!(after_enter("abc"), "abc\n");
	}

	#[test]
	fn enter_on_an_empty_item_ends_the_list() {
		assert_eq!(after_enter("a\n- "), "a\n");
		let mut s = at("- ", 2);
		s.newline();
		assert_eq!(s.selections(), [Selection::caret(0)]);
	}

	#[test]
	fn tab_indents_list_items_and_inserts_elsewhere() {
		let mut s = at("- a", 3);
		s.tab(false);
		assert_eq!(s.text(), "\t- a");
		s.tab(true);
		assert_eq!(s.text(), "- a");
		let mut s = at("ab", 1);
		s.tab(false);
		assert_eq!(s.text(), "a\tb");
	}

	#[test]
	fn toggle_wrap_round_trips() {
		let mut s = EditorState::new("x".into());
		s.select_all();
		s.toggle_wrap("**");
		assert_eq!(s.text(), "**x**");
		assert_eq!(s.selections(), [Selection { anchor: 2, head: 3 }]);
		s.toggle_wrap("**");
		assert_eq!(s.text(), "x");
		let mut s = at("", 0);
		s.toggle_wrap("*");
		assert_eq!(s.text(), "**");
		assert_eq!(s.selections(), [Selection::caret(1)]);
	}

	#[test]
	fn links() {
		let mut s = EditorState::new("site".into());
		s.select_all();
		s.insert_link();
		assert_eq!(s.text(), "[site]()");
		assert_eq!(s.selections(), [Selection::caret(7)]);
		let mut s = at("", 0);
		s.insert_link();
		assert_eq!(s.text(), "[]()");
		assert_eq!(s.selections(), [Selection::caret(1)]);
		let mut s = EditorState::new("site".into());
		s.select_all();
		s.paste_text("https://x.y", false);
		assert_eq!(s.text(), "[site](https://x.y)");
		let mut s = EditorState::new("site".into());
		s.select_all();
		s.paste_text("not a url", false);
		assert_eq!(s.text(), "not a url");
	}

	#[test]
	fn auto_pairs_and_wrapping_chars() {
		let mut s = at("", 0);
		s.type_char('(');
		assert_eq!(s.text(), "()");
		assert_eq!(s.selections(), [Selection::caret(1)]);
		s.type_char('a');
		s.type_char(')');
		assert_eq!(s.text(), "(a)");
		assert_eq!(s.selections(), [Selection::caret(3)]);
		let mut s = EditorState::new("x".into());
		s.select_all();
		s.type_char('=');
		assert_eq!(s.text(), "==x==");
		let mut s = at("", 0);
		s.type_char('[');
		s.type_char('[');
		assert_eq!(s.text(), "[[]]");
		assert_eq!(s.selections(), [Selection::caret(2)]);
	}

	#[test]
	fn enter_with_two_carets_on_an_emptied_item_does_not_panic() {
		let mut s = EditorState::new("- foo foo".into());
		s.set_selections(vec![Selection::caret(2)]);
		s.add_next_occurrence();
		s.add_next_occurrence();
		s.backspace();
		assert_eq!(s.text(), "-  ");
		s.newline();
		assert_eq!(s.text(), "");
	}

	#[test]
	fn enter_with_a_caret_inside_and_after_the_marker_does_not_panic() {
		let mut s = EditorState::new("- ".into());
		s.set_selections(vec![Selection::caret(1), Selection::caret(2)]);
		s.newline();
	}

	#[test]
	fn wrap_toggle_on_adjacent_wrapped_selections_does_not_panic() {
		let mut s = EditorState::new("*a*b*".into());
		s.set_selections(vec![
			Selection { anchor: 1, head: 2 },
			Selection { anchor: 3, head: 4 },
		]);
		s.toggle_wrap("*");
		let mut s = EditorState::new("****\n****".into());
		s.set_selections(vec![
			Selection { anchor: 2, head: 2 },
			Selection { anchor: 2, head: 7 },
		]);
		s.toggle_wrap("**");
	}

	#[test]
	fn typing_a_code_fence() {
		let mut s = EditorState::new(String::new());
		for c in "```python".chars() {
			s.type_char(c);
		}
		assert_eq!(s.text(), "```python");
	}

	#[test]
	fn enter_and_tab_in_fenced_code_are_plain() {
		let t = "```yaml\n- name: x";
		let mut s = at(t, t.len());
		s.newline();
		assert_eq!(s.text(), "```yaml\n- name: x\n");
		let t = "```python\n    x = 1";
		let mut s = at(t, t.len());
		s.newline();
		assert_eq!(s.text(), "```python\n    x = 1\n    ");
		let t = "```\n- a";
		let mut s = at(t, t.len());
		s.tab(false);
		assert_eq!(s.text(), "```\n- a\t");
	}

	#[test]
	fn ctrl_d_right_after_a_word_selects_it() {
		let mut s = EditorState::new("foo bar foo".into());
		s.set_selections(vec![Selection::caret(3)]);
		s.add_next_occurrence();
		assert_eq!(s.selections(), [Selection { anchor: 0, head: 3 }]);
		let mut s = at("foo foo", 7);
		s.add_next_occurrence();
		assert_eq!(s.selections(), [Selection { anchor: 4, head: 7 }]);
		let mut s = at("a  b", 2);
		let v = s.selections().to_vec();
		s.add_next_occurrence();
		assert_eq!(s.selections(), v.as_slice());
	}

	#[test]
	fn italic_on_a_bold_word_keeps_the_bold() {
		let mut s = EditorState::new("**x**".into());
		s.set_selections(vec![Selection { anchor: 2, head: 3 }]);
		s.toggle_wrap("*");
		assert_eq!(s.text(), "***x***");
		s.toggle_wrap("*");
		assert_eq!(s.text(), "**x**");
	}

	#[test]
	fn toggle_task_both_ways_and_undo() {
		let mut s = EditorState::new("- [ ] a\n- [X] b".into());
		s.set_selections(vec![Selection::caret(7)]);
		s.toggle_task(2..5);
		assert_eq!(s.text(), "- [x] a\n- [X] b");
		s.toggle_task(10..13);
		assert_eq!(s.text(), "- [x] a\n- [ ] b");
		assert_eq!(s.selections(), [Selection::caret(7)]);
		assert!(s.undo());
		assert_eq!(s.text(), "- [x] a\n- [X] b");
	}
}
