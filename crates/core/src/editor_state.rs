//! Text plus selections, and the editing operations applied to them.
//!
//! Offsets are byte offsets into the text and always sit on char boundaries.

use std::ops::Range;

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
	pub anchor: usize,
	pub head: usize,
}

impl Selection {
	pub fn caret(offset: usize) -> Self {
		Selection {
			anchor: offset,
			head: offset,
		}
	}

	pub fn range(&self) -> Range<usize> {
		self.anchor.min(self.head)..self.anchor.max(self.head)
	}

	pub fn is_empty(&self) -> bool {
		self.anchor == self.head
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
	Left,
	Right,
	WordLeft,
	WordRight,
	LineStart,
	LineEnd,
	DocStart,
	DocEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
	Space,
	Word,
	Punct,
}

/// Class of a grapheme cluster: the class of its first char (combining marks follow their base).
fn cluster_class(cluster: &str) -> CharClass {
	cluster.chars().next().map_or(CharClass::Space, class)
}

fn class(c: char) -> CharClass {
	if c.is_whitespace() {
		CharClass::Space
	} else if c.is_alphanumeric() || c == '_' {
		CharClass::Word
	} else {
		CharClass::Punct
	}
}

/// One replacement. `caret` is where the caret lands, as an offset in the old text
/// (`None` = right after the inserted text).
struct Edit {
	range: Range<usize>,
	insert: String,
	caret: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct EditorState {
	text: String,
	selections: Vec<Selection>,
	/// The selection commands act on first (the last one added); resolved by `primary()`.
	primary: Selection,
	version: u64,
	history: Vec<Transaction>,
	redo_stack: Vec<Transaction>,
	/// The last transaction may still absorb the next typing or deleting edit.
	group_open: bool,
}

/// How an edit groups with the previous one for undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
	Typing,
	Deleting,
	Other,
}

/// One replacement as recorded for undo: at byte `at` of the text after the edit, `inserted`
/// replaced `removed`.
#[derive(Debug, Clone)]
struct Change {
	at: usize,
	removed: String,
	inserted: String,
}

/// An undo step: one or more edits (each a list of changes in ascending order).
#[derive(Debug, Clone)]
struct Transaction {
	steps: Vec<Vec<Change>>,
	before: Vec<Selection>,
	after: Vec<Selection>,
	kind: EditKind,
}

const MAX_UNDO: usize = 500;

impl EditorState {
	pub fn new(text: String) -> Self {
		EditorState {
			text,
			selections: vec![Selection::caret(0)],
			primary: Selection::caret(0),
			version: 0,
			history: Vec::new(),
			redo_stack: Vec::new(),
			group_open: false,
		}
	}

	/// Reverts the last undo step; returns false when there is nothing to undo.
	pub fn undo(&mut self) -> bool {
		let Some(t) = self.history.pop() else {
			return false;
		};
		for step in t.steps.iter().rev() {
			for ch in step.iter().rev() {
				self.text
					.replace_range(ch.at..ch.at + ch.inserted.len(), &ch.removed);
			}
		}
		self.selections = t.before.clone();
		self.finish_history_move();
		self.redo_stack.push(t);
		true
	}

	/// Re-applies the last undone step; returns false when there is nothing to redo.
	pub fn redo(&mut self) -> bool {
		let Some(t) = self.redo_stack.pop() else {
			return false;
		};
		for step in &t.steps {
			for ch in step {
				self.text
					.replace_range(ch.at..ch.at + ch.removed.len(), &ch.inserted);
			}
		}
		self.selections = t.after.clone();
		self.finish_history_move();
		self.history.push(t);
		true
	}

	/// Replaces each source range with its text (ranges sorted and disjoint), then sets the
	/// given selections (in the new text). One undo step; nothing happens if the text is unchanged.
	pub(crate) fn replace_with(
		&mut self,
		mut edits: Vec<(Range<usize>, String)>,
		after: Vec<Selection>,
	) {
		edits.sort_by_key(|(r, _)| r.start);
		let mut out = String::with_capacity(self.text.len());
		let mut last = 0;
		let mut changes = Vec::with_capacity(edits.len());
		for (range, insert) in &edits {
			debug_assert!(
				range.start >= last,
				"overlapping edits reached replace_with"
			);
			let start = range.start.max(last);
			let end = range.end.max(start);
			out.push_str(&self.text[last..start]);
			changes.push(Change {
				at: out.len(),
				removed: self.text[start..end].to_string(),
				inserted: insert.clone(),
			});
			out.push_str(insert);
			last = end;
		}
		out.push_str(&self.text[last..]);
		if out == self.text {
			self.set_selections(after);
			return;
		}
		let before = self.selections.clone();
		self.text = out;
		self.version += 1;
		self.set_selections(after);
		self.record(changes, before, EditKind::Other);
	}

	/// The next edit starts a new undo step even if it would otherwise merge.
	pub fn break_undo_group(&mut self) {
		self.group_open = false;
	}

	fn finish_history_move(&mut self) {
		self.version += 1;
		self.group_open = false;
		self.normalize();
	}

	fn record(&mut self, changes: Vec<Change>, before: Vec<Selection>, kind: EditKind) {
		self.redo_stack.clear();
		let after = self.selections.clone();
		if let Some(last) = self.history.last_mut()
			&& self.group_open
			&& kind != EditKind::Other
			&& last.kind == kind
			&& last.after == before
		{
			last.steps.push(changes);
			last.after = after;
		} else {
			self.history.push(Transaction {
				steps: vec![changes],
				before,
				after,
				kind,
			});
			if self.history.len() > MAX_UNDO {
				self.history.remove(0);
			}
		}
		self.group_open = kind != EditKind::Other;
	}

	pub fn text(&self) -> &str {
		&self.text
	}

	pub fn selections(&self) -> &[Selection] {
		&self.selections
	}

	pub fn version(&self) -> u64 {
		self.version
	}

	/// Replaces the selections; the last one given becomes the primary selection.
	pub fn set_selections(&mut self, selections: Vec<Selection>) {
		if let Some(last) = selections.last() {
			self.primary = *last;
		}
		self.selections = selections;
		self.normalize();
	}

	/// The primary selection (the last one added), after merging.
	pub fn primary(&self) -> Selection {
		let p = self.primary;
		self.selections
			.iter()
			.find(|s| {
				let r = s.range();
				r.start <= p.head && p.head <= r.end
			})
			.copied()
			.unwrap_or_else(|| *self.selections.last().expect("never empty"))
	}

	fn primary_index(&self) -> usize {
		let p = self.primary();
		self.selections.iter().position(|s| *s == p).unwrap_or(0)
	}

	/// Sets selections given in the same order as the current ones, keeping the primary one.
	fn set_selections_keep_primary(&mut self, selections: Vec<Selection>) {
		let index = self.primary_index();
		let primary = selections.get(index).copied();
		self.set_selections(selections);
		if let Some(p) = primary {
			self.primary = p;
		}
	}

	/// Replaces everything (external reload); undo history is cleared.
	pub fn set_text(&mut self, text: String) {
		self.text = text;
		self.version += 1;
		self.history.clear();
		self.redo_stack.clear();
		self.group_open = false;
		self.normalize();
	}

	pub fn insert(&mut self, s: &str) {
		let kind = if s.is_empty() || s.contains('\n') {
			EditKind::Other
		} else {
			EditKind::Typing
		};
		let edits = self
			.selections
			.iter()
			.map(|sel| Edit {
				range: sel.range(),
				insert: s.to_string(),
				caret: None,
			})
			.collect();
		self.apply_kind(edits, kind);
	}

	pub fn backspace(&mut self) {
		self.delete_with(|s, head| s.prev_grapheme(head)..head);
	}

	pub fn delete_forward(&mut self) {
		self.delete_with(|s, head| head..s.next_grapheme(head));
	}

	pub fn delete_word_left(&mut self) {
		self.delete_with(|s, head| s.word_left(head)..head);
	}

	pub fn delete_word_right(&mut self) {
		self.delete_with(|s, head| head..s.word_right(head));
	}

	pub fn move_by(&mut self, motion: Motion, extend: bool) {
		let moved = self
			.selections
			.iter()
			.map(|sel| {
				if !extend && !sel.is_empty() {
					match motion {
						Motion::Left => return Selection::caret(sel.range().start),
						Motion::Right => return Selection::caret(sel.range().end),
						_ => {}
					}
				}
				let head = self.target(sel.head, motion);
				if extend {
					Selection {
						anchor: sel.anchor,
						head,
					}
				} else {
					Selection::caret(head)
				}
			})
			.collect();
		self.set_selections_keep_primary(moved);
	}

	pub fn select_all(&mut self) {
		self.set_selections(vec![Selection {
			anchor: 0,
			head: self.text.len(),
		}]);
	}

	pub fn replace_range(&mut self, range: Range<usize>, s: &str) {
		let start = self.floor_boundary(range.start);
		let end = self.floor_boundary(range.end).max(start);
		let kind = if s.contains('\n') {
			EditKind::Other
		} else {
			EditKind::Typing
		};
		self.apply_kind(
			vec![Edit {
				range: start..end,
				insert: s.to_string(),
				caret: None,
			}],
			kind,
		);
	}

	/// Selected text joined by `\n`, or (VS Code) the carets' whole lines when nothing is selected.
	pub fn copy(&self) -> (String, bool) {
		if self.selections.iter().all(Selection::is_empty) {
			let mut out = String::new();
			let mut last_line = None;
			for sel in &self.selections {
				let start = self.line_start(sel.head);
				if last_line == Some(start) {
					continue;
				}
				last_line = Some(start);
				out.push_str(&self.text[start..self.line_end(sel.head)]);
				out.push('\n');
			}
			(out, true)
		} else {
			let parts: Vec<&str> = self
				.selections
				.iter()
				.map(|s| &self.text[s.range()])
				.collect();
			(parts.join("\n"), false)
		}
	}

	pub fn cut(&mut self) -> (String, bool) {
		let (copied, full_line) = self.copy();
		if full_line {
			let mut edits: Vec<Edit> = Vec::new();
			for sel in &self.selections {
				let start = self.line_start(sel.head);
				let end = self.line_end(sel.head);
				let range = if end < self.text.len() {
					start..end + 1
				} else if start > 0 {
					start - 1..end
				} else {
					start..end
				};
				if edits.last().is_some_and(|e| e.range == range) {
					continue;
				}
				edits.push(Edit {
					range,
					insert: String::new(),
					caret: None,
				});
			}
			self.apply(edits);
		} else {
			self.insert("");
		}
		(copied, full_line)
	}

	pub fn paste(&mut self, s: &str, full_line: bool) {
		if full_line && self.selections.iter().all(Selection::is_empty) {
			let edits = self
				.selections
				.iter()
				.map(|sel| {
					let start = self.line_start(sel.head);
					Edit {
						range: start..start,
						insert: s.to_string(),
						caret: Some(sel.head),
					}
				})
				.collect();
			self.apply(edits);
		} else {
			self.insert(s);
		}
	}

	/// The run of same-class characters (word, punctuation or spaces) under `offset` (double click).
	pub fn word_range_at(&self, offset: usize) -> Range<usize> {
		let offset = self.floor_boundary(offset);
		let same = |g: &str, run: CharClass| cluster_class(g) == run && !g.contains('\n');
		let Some(first) = self.text[offset..]
			.graphemes(true)
			.next()
			.filter(|g| !g.contains('\n'))
		else {
			return offset..offset;
		};
		let run = cluster_class(first);
		let start = self.text[..offset]
			.grapheme_indices(true)
			.rev()
			.take_while(|(_, g)| same(g, run))
			.last()
			.map_or(offset, |(i, _)| i);
		let end = self.text[offset..]
			.grapheme_indices(true)
			.find(|(_, g)| !same(g, run))
			.map_or(self.text.len(), |(i, _)| offset + i);
		start..end
	}

	/// The logical line under `offset`, including its trailing newline (triple click).
	pub fn line_range_at(&self, offset: usize) -> Range<usize> {
		let offset = self.floor_boundary(offset);
		let end = self.line_end(offset);
		self.line_start(offset)..(end + 1).min(self.text.len())
	}

	/// 1-based line and 1-based column counted in chars.
	pub fn line_col(&self, offset: usize) -> (usize, usize) {
		let offset = self.floor_boundary(offset);
		let before = &self.text[..offset];
		let line = before.matches('\n').count() + 1;
		let col = before[self.line_start(offset)..].chars().count() + 1;
		(line, col)
	}

	fn delete_with(&mut self, range_for_caret: impl Fn(&Self, usize) -> Range<usize>) {
		let edits = self
			.selections
			.iter()
			.map(|sel| Edit {
				range: if sel.is_empty() {
					range_for_caret(self, sel.head)
				} else {
					sel.range()
				},
				insert: String::new(),
				caret: None,
			})
			.collect();
		self.apply_kind(edits, EditKind::Deleting);
	}

	/// Applies edits sorted by position; overlapping ranges are clipped to the previous edit's end.
	fn apply(&mut self, edits: Vec<Edit>) {
		self.apply_kind(edits, EditKind::Other);
	}

	/// Applies edits sorted by position (overlaps are clipped to the previous edit's end) and
	/// records them for undo.
	fn apply_kind(&mut self, mut edits: Vec<Edit>, kind: EditKind) {
		edits.sort_by_key(|e| e.range.start);
		if edits
			.iter()
			.all(|e| e.range.is_empty() && e.insert.is_empty())
		{
			self.set_selections(
				edits
					.iter()
					.map(|e| Selection::caret(e.range.start))
					.collect(),
			);
			return;
		}
		let mut out = String::with_capacity(
			self.text.len() + edits.iter().map(|e| e.insert.len()).sum::<usize>(),
		);
		let mut last = 0;
		let mut carets = Vec::with_capacity(edits.len());
		let mut changes = Vec::with_capacity(edits.len());
		let before = self.selections.clone();
		for e in &edits {
			let start = e.range.start.max(last);
			let end = e.range.end.max(start);
			out.push_str(&self.text[last..start]);
			changes.push(Change {
				at: out.len(),
				removed: self.text[start..end].to_string(),
				inserted: e.insert.clone(),
			});
			out.push_str(&e.insert);
			let after_insert = out.len();
			carets.push(match e.caret {
				Some(p) => after_insert + p.saturating_sub(end),
				None => after_insert,
			});
			last = end;
		}
		out.push_str(&self.text[last..]);
		self.text = out;
		self.version += 1;
		self.set_selections_keep_primary(carets.into_iter().map(Selection::caret).collect());
		self.record(changes, before, kind);
	}

	fn normalize(&mut self) {
		let mut sels: Vec<Selection> = self
			.selections
			.iter()
			.map(|s| Selection {
				anchor: self.floor_boundary(s.anchor),
				head: self.floor_boundary(s.head),
			})
			.collect();
		if sels.is_empty() {
			sels.push(Selection::caret(0));
		}
		sels.sort_by_key(|s| s.range().start);
		let mut merged: Vec<Selection> = Vec::with_capacity(sels.len());
		for s in sels {
			if let Some(prev) = merged.last_mut() {
				let (p, r) = (prev.range(), s.range());
				if r.start < p.end || p == r {
					let start = p.start.min(r.start);
					let end = p.end.max(r.end);
					*prev = if prev.head < prev.anchor {
						Selection {
							anchor: end,
							head: start,
						}
					} else {
						Selection {
							anchor: start,
							head: end,
						}
					};
					continue;
				}
			}
			merged.push(s);
		}
		self.selections = merged;
	}

	/// Moves `offset` back to the nearest grapheme cluster boundary (never inside a char or a
	/// cluster such as a flag, a keycap or a letter with a combining accent).
	fn floor_boundary(&self, offset: usize) -> usize {
		let mut o = offset.min(self.text.len());
		while !self.text.is_char_boundary(o) {
			o -= 1;
		}
		let mut cursor = GraphemeCursor::new(o, self.text.len(), true);
		match cursor.is_boundary(&self.text, 0) {
			Ok(true) | Err(_) => o,
			Ok(false) => self.prev_grapheme(o),
		}
	}

	fn prev_grapheme(&self, offset: usize) -> usize {
		let mut cursor = GraphemeCursor::new(offset, self.text.len(), true);
		match cursor.prev_boundary(&self.text, 0) {
			Ok(Some(o)) => o,
			Ok(None) => 0,
			Err(_) => self.text[..offset]
				.char_indices()
				.next_back()
				.map_or(0, |(i, _)| i),
		}
	}

	fn next_grapheme(&self, offset: usize) -> usize {
		let mut cursor = GraphemeCursor::new(offset, self.text.len(), true);
		match cursor.next_boundary(&self.text, 0) {
			Ok(Some(o)) => o,
			Ok(None) => self.text.len(),
			Err(_) => self.text[offset..]
				.chars()
				.next()
				.map_or(offset, |c| offset + c.len_utf8()),
		}
	}

	fn word_right(&self, offset: usize) -> usize {
		let mut clusters = self.text[offset..].grapheme_indices(true).peekable();
		while clusters
			.next_if(|(_, g)| cluster_class(g) == CharClass::Space)
			.is_some()
		{}
		let Some(&(_, first)) = clusters.peek() else {
			return self.text.len();
		};
		let run = cluster_class(first);
		while clusters.next_if(|(_, g)| cluster_class(g) == run).is_some() {}
		clusters.peek().map_or(self.text.len(), |(i, _)| offset + i)
	}

	fn word_left(&self, offset: usize) -> usize {
		let mut clusters = self.text[..offset].grapheme_indices(true).rev().peekable();
		while clusters
			.next_if(|(_, g)| cluster_class(g) == CharClass::Space)
			.is_some()
		{}
		let Some(&(_, first)) = clusters.peek() else {
			return 0;
		};
		let run = cluster_class(first);
		let mut pos = 0;
		while let Some((i, _)) = clusters.next_if(|(_, g)| cluster_class(g) == run) {
			pos = i;
		}
		pos
	}

	pub(crate) fn line_start(&self, offset: usize) -> usize {
		self.text[..offset].rfind('\n').map_or(0, |i| i + 1)
	}

	pub(crate) fn line_end(&self, offset: usize) -> usize {
		self.text[offset..]
			.find('\n')
			.map_or(self.text.len(), |i| offset + i)
	}

	fn target(&self, head: usize, motion: Motion) -> usize {
		match motion {
			Motion::Left => self.prev_grapheme(head),
			Motion::Right => self.next_grapheme(head),
			Motion::WordLeft => self.word_left(head),
			Motion::WordRight => self.word_right(head),
			Motion::LineStart => {
				let start = self.line_start(head);
				let indent = self.text[start..]
					.chars()
					.take_while(|c| *c == ' ' || *c == '\t')
					.map(char::len_utf8)
					.sum::<usize>();
				if head == start + indent {
					start
				} else {
					start + indent
				}
			}
			Motion::LineEnd => self.line_end(head),
			Motion::DocStart => 0,
			Motion::DocEnd => self.text.len(),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn at(text: &str, offset: usize) -> EditorState {
		let mut s = EditorState::new(text.into());
		s.set_selections(vec![Selection::caret(offset)]);
		s
	}

	#[test]
	fn insert_and_backspace_accents_and_emoji() {
		let mut s = at("", 0);
		s.insert("été 👍🏽");
		assert_eq!(s.selections(), [Selection::caret("été 👍🏽".len())]);
		s.backspace();
		assert_eq!(s.text(), "été ");
		s.move_by(Motion::Left, false);
		s.move_by(Motion::Left, false);
		assert_eq!(s.selections(), [Selection::caret("ét".len())]);
		s.backspace();
		assert_eq!(s.text(), "éé ");
	}

	#[test]
	fn word_motions() {
		let mut s = at("hello, world", 0);
		s.move_by(Motion::WordRight, false);
		assert_eq!(s.selections()[0].head, 5);
		s.move_by(Motion::WordRight, false);
		assert_eq!(s.selections()[0].head, 6);
		s.move_by(Motion::WordRight, false);
		assert_eq!(s.selections()[0].head, 12);
		s.move_by(Motion::WordLeft, false);
		assert_eq!(s.selections()[0].head, 7);
		s.delete_word_left();
		assert_eq!(s.text(), "helloworld");
	}

	#[test]
	fn smart_home() {
		let mut s = at("\t- item", 7);
		s.move_by(Motion::LineStart, false);
		assert_eq!(s.selections()[0].head, 1);
		s.move_by(Motion::LineStart, false);
		assert_eq!(s.selections()[0].head, 0);
	}

	#[test]
	fn copy_cut_paste_full_line() {
		let mut s = at("ab\ncd\n", 4);
		assert_eq!(s.copy(), ("cd\n".to_string(), true));
		let (cut, full) = s.cut();
		assert_eq!(s.text(), "ab\n");
		let mut t = at("x\ny", 3);
		t.paste(&cut, full);
		assert_eq!(t.text(), "x\ncd\ny");
	}

	#[test]
	fn multi_caret_insert() {
		let mut s = EditorState::new("ab".into());
		s.set_selections(vec![
			Selection::caret(0),
			Selection::caret(1),
			Selection::caret(2),
		]);
		s.insert("-");
		assert_eq!(s.text(), "-a-b-");
		assert_eq!(s.selections().len(), 3);
	}

	#[test]
	fn selections_merge_and_clamp() {
		let mut s = EditorState::new("été".into());
		s.set_selections(vec![
			Selection { anchor: 0, head: 3 },
			Selection {
				anchor: 2,
				head: 99,
			},
		]);
		assert_eq!(
			s.selections(),
			[Selection {
				anchor: 0,
				head: "été".len()
			}]
		);
		s.set_selections(vec![Selection::caret(1)]);
		assert!(s.text().is_char_boundary(s.selections()[0].head));
	}

	#[test]
	fn line_col_counts_chars() {
		let s = EditorState::new("ab\ncé x".into());
		assert_eq!(s.line_col("ab\ncé".len()), (2, 3));
	}

	#[test]
	fn version_bumps_on_edit_only() {
		let mut s = at("a", 1);
		let v = s.version();
		s.move_by(Motion::Left, false);
		assert_eq!(s.version(), v);
		s.insert("b");
		assert!(s.version() > v);
	}

	#[test]
	fn word_and_line_ranges() {
		let s = EditorState::new(
			"été là
suite"
				.into(),
		);
		assert_eq!(s.word_range_at(1), 0.."été".len());
		assert_eq!(s.word_range_at("été ".len()), "été ".len().."été là".len());
		assert_eq!(
			s.line_range_at(2),
			0.."été là
"
			.len()
		);
		assert_eq!(
			s.line_range_at(s.text().len()),
			"été là
"
			.len()..s.text().len()
		);
	}

	#[test]
	fn motions_and_carets_respect_grapheme_clusters() {
		let cafe = "cafe\u{301}";
		let mut s = at("cafe\u{301} x", 0);
		s.move_by(Motion::WordRight, false);
		assert_eq!(s.selections()[0].head, cafe.len());
		assert_eq!(s.word_range_at(1), 0..cafe.len());
		let mut flag = EditorState::new("\u{1F1EB}\u{1F1F7}".into());
		flag.set_selections(vec![Selection::caret("\u{1F1EB}".len())]);
		assert_eq!(flag.selections()[0].head, 0);
		let keycap = "1\u{FE0F}\u{20E3}";
		let mut k = at("1\u{FE0F}\u{20E3} a", 0);
		k.move_by(Motion::WordRight, false);
		assert_eq!(k.selections()[0].head, keycap.len());
	}

	#[test]
	fn typing_is_one_undo_step() {
		let mut s = at("", 0);
		for c in ["a", "b", "c"] {
			s.insert(c);
		}
		assert!(s.undo());
		assert_eq!(s.text(), "");
		assert_eq!(s.selections(), [Selection::caret(0)]);
		assert!(!s.undo());
	}

	#[test]
	fn caret_move_starts_a_new_undo_step() {
		let mut s = at("", 0);
		s.insert("a");
		s.insert("b");
		s.move_by(Motion::Left, false);
		s.insert("x");
		assert_eq!(s.text(), "axb");
		assert!(s.undo());
		assert_eq!(s.text(), "ab");
		assert!(s.undo());
		assert_eq!(s.text(), "");
	}

	#[test]
	fn multi_caret_undo_restores_text_and_carets() {
		let mut s = EditorState::new("ab".into());
		let carets = vec![
			Selection::caret(0),
			Selection::caret(1),
			Selection::caret(2),
		];
		s.set_selections(carets.clone());
		s.insert("-");
		assert!(s.undo());
		assert_eq!(s.text(), "ab");
		assert_eq!(s.selections(), carets.as_slice());
	}

	#[test]
	fn undo_then_redo_round_trips() {
		let mut s = at("", 0);
		s.insert("hello");
		s.insert("\n");
		s.backspace();
		s.backspace();
		assert_eq!(s.text(), "hell");
		while s.undo() {}
		assert_eq!(s.text(), "");
		while s.redo() {}
		assert_eq!(s.text(), "hell");
		assert_eq!(s.selections(), [Selection::caret(4)]);
	}

	#[test]
	fn undo_without_history_changes_nothing() {
		let mut s = at("x", 0);
		let v = s.version();
		assert!(!s.undo());
		assert!(!s.redo());
		assert_eq!(s.version(), v);
	}

	#[test]
	fn a_new_edit_clears_redo_and_break_splits_groups() {
		let mut s = at("", 0);
		s.insert("a");
		s.break_undo_group();
		s.insert("b");
		assert!(s.undo());
		assert_eq!(s.text(), "a");
		s.insert("c");
		assert!(!s.redo());
		assert_eq!(s.text(), "ac");
	}

	#[test]
	fn reload_clears_history() {
		let mut s = at("", 0);
		s.insert("a");
		s.set_text("disk".into());
		assert!(!s.undo());
		assert_eq!(s.text(), "disk");
	}
}
