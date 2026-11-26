//! Keyboard commands of the note editor (VS Code bindings live in `keymap.rs`).

use super::complete::CompleteKey;
use gpui_kit::*;
use ruby_core::editor_state::{Motion, Selection};

use super::NoteEditor;

actions!(
	editor,
	[
		MoveLeft,
		MoveRight,
		SelectLeft,
		SelectRight,
		MoveWordLeft,
		MoveWordRight,
		SelectWordLeft,
		SelectWordRight,
		MoveLineStart,
		MoveLineEnd,
		SelectLineStart,
		SelectLineEnd,
		MoveDocStart,
		MoveDocEnd,
		SelectDocStart,
		SelectDocEnd,
		MoveUp,
		MoveDown,
		SelectUp,
		SelectDown,
		PageUp,
		PageDown,
		SelectPageUp,
		SelectPageDown,
		Backspace,
		Delete,
		DeleteWordLeft,
		DeleteWordRight,
		Newline,
		Tab,
		SelectAll,
		Copy,
		Cut,
		Paste,
		ShiftTab,
		Undo,
		Redo,
		AddNextOccurrence,
		SelectAllOccurrences,
		CollapseSelections,
		AddCursorAbove,
		AddCursorBelow,
		MoveLinesUp,
		MoveLinesDown,
		CopyLinesUp,
		CopyLinesDown,
		DeleteLines,
		InsertLineBelow,
		InsertLineAbove,
		SelectLines,
		IndentLines,
		OutdentLines,
		Bold,
		Italic,
		InsertLink,
		ToggleLivePreview,
		Find,
		Replace,
		FindNext,
		FindPrevious
	]
);

const FULL_LINE: &str = "full_line";

impl NoteEditor {
	fn motion(&mut self, motion: Motion, extend: bool, cx: &mut Context<Self>) {
		self.preferred_x = None;
		self.state.move_by(motion, extend);
		self.after_command(cx);
	}

	fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Self)) {
		if self.doc.read_only {
			return;
		}
		self.preferred_x = None;
		self.marked_range = None;
		f(self);
		self.edited(cx);
		self.after_command(cx);
	}

	pub(super) fn after_command(&mut self, cx: &mut Context<Self>) {
		self.refresh_link_complete(false);
		self.autoscroll = Some(super::Autoscroll::Fit);
		cx.notify();
	}

	/// Moves every caret by `rows` visual rows, keeping its horizontal position.
	pub(crate) fn move_vertical(
		&mut self,
		rows: f32,
		extend: bool,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let Some(mut layout) = self.layout.clone() else {
			return;
		};
		// Several keys can arrive before the next frame (key repeat): lay out the current text.
		if layout.version != self.state.version() || layout.marked != self.marked_range {
			layout = self.layout(layout.width, window, cx);
		}
		let lh_at = |o: usize| layout.line_height_at(o);
		let len = self.state.text().len();
		let selections: Vec<Selection> = self.state.selections().to_vec();
		let keep = self
			.preferred_x
			.as_ref()
			.filter(|xs| xs.len() == selections.len())
			.cloned();
		let mut xs = Vec::with_capacity(selections.len());
		let moved = selections
			.iter()
			.enumerate()
			.map(|(i, sel)| {
				let p = layout.point_for_offset(sel.head);
				let x = keep.as_ref().map_or(p.x, |xs| xs[i]);
				xs.push(x);
				let lh = lh_at(sel.head);
				let y = p.y + lh * rows + lh / 2.;
				let head = if y < px(0.) {
					0
				} else if y >= layout.total_height {
					len
				} else {
					layout.offset_for_point(point(x, y))
				};
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
		self.state.set_selections(moved);
		self.preferred_x = Some(xs);
		self.after_command(cx);
	}

	/// Ctrl+Alt+Up / Down: add a caret one visual line above or below the primary one.
	fn add_cursor_vertical(&mut self, rows: f32, window: &mut Window, cx: &mut Context<Self>) {
		let Some(mut layout) = self.layout.clone() else {
			return;
		};
		if layout.version != self.state.version() {
			layout = self.layout(layout.width, window, cx);
		}
		let head = self.state.primary().head;
		let p = layout.point_for_offset(head);
		let lh = layout.line_height_at(head);
		let y = p.y + lh * rows + lh / 2.;
		if y < px(0.) || y >= layout.total_height {
			return;
		}
		let offset = layout.offset_for_point(point(p.x, y));
		self.state.add_selection(Selection::caret(offset));
		self.after_command(cx);
	}

	pub(crate) fn toggle_live(&mut self, cx: &mut Context<Self>) {
		self.live = !self.live;
		self.after_command(cx);
	}

	fn page_rows(&self) -> f32 {
		let height = self.scroll.bounds().size.height;
		(f32::from(height) / super::layout::LINE_HEIGHT)
			.floor()
			.max(1.)
	}

	fn copy_to_clipboard(&self, text: String, full_line: bool, cx: &mut Context<Self>) {
		let item = if full_line {
			ClipboardItem::new_string_with_metadata(text, FULL_LINE.into())
		} else {
			ClipboardItem::new_string(text)
		};
		cx.write_to_clipboard(item);
	}

	pub(super) fn register_actions(&self, d: Div, cx: &mut Context<Self>) -> Div {
		d.on_action(cx.listener(|e, _: &MoveLeft, _, cx| e.motion(Motion::Left, false, cx)))
			.on_action(cx.listener(|e, _: &MoveRight, _, cx| e.motion(Motion::Right, false, cx)))
			.on_action(cx.listener(|e, _: &SelectLeft, _, cx| e.motion(Motion::Left, true, cx)))
			.on_action(cx.listener(|e, _: &SelectRight, _, cx| e.motion(Motion::Right, true, cx)))
			.on_action(
				cx.listener(|e, _: &MoveWordLeft, _, cx| e.motion(Motion::WordLeft, false, cx)),
			)
			.on_action(
				cx.listener(|e, _: &MoveWordRight, _, cx| e.motion(Motion::WordRight, false, cx)),
			)
			.on_action(
				cx.listener(|e, _: &SelectWordLeft, _, cx| e.motion(Motion::WordLeft, true, cx)),
			)
			.on_action(
				cx.listener(|e, _: &SelectWordRight, _, cx| e.motion(Motion::WordRight, true, cx)),
			)
			.on_action(
				cx.listener(|e, _: &MoveLineStart, _, cx| e.motion(Motion::LineStart, false, cx)),
			)
			.on_action(
				cx.listener(|e, _: &MoveLineEnd, _, cx| e.motion(Motion::LineEnd, false, cx)),
			)
			.on_action(
				cx.listener(|e, _: &SelectLineStart, _, cx| e.motion(Motion::LineStart, true, cx)),
			)
			.on_action(
				cx.listener(|e, _: &SelectLineEnd, _, cx| e.motion(Motion::LineEnd, true, cx)),
			)
			.on_action(
				cx.listener(|e, _: &MoveDocStart, _, cx| e.motion(Motion::DocStart, false, cx)),
			)
			.on_action(cx.listener(|e, _: &MoveDocEnd, _, cx| e.motion(Motion::DocEnd, false, cx)))
			.on_action(
				cx.listener(|e, _: &SelectDocStart, _, cx| e.motion(Motion::DocStart, true, cx)),
			)
			.on_action(cx.listener(|e, _: &SelectDocEnd, _, cx| e.motion(Motion::DocEnd, true, cx)))
			.on_action(cx.listener(|e, _: &MoveUp, window, cx| {
				if !e.complete_key(CompleteKey::Up, cx) {
					e.move_vertical(-1., false, window, cx)
				}
			}))
			.on_action(cx.listener(|e, _: &MoveDown, window, cx| {
				if !e.complete_key(CompleteKey::Down, cx) {
					e.move_vertical(1., false, window, cx)
				}
			}))
			.on_action(
				cx.listener(|e, _: &SelectUp, window, cx| e.move_vertical(-1., true, window, cx)),
			)
			.on_action(
				cx.listener(|e, _: &SelectDown, window, cx| e.move_vertical(1., true, window, cx)),
			)
			.on_action(cx.listener(|e, _: &PageUp, window, cx| {
				e.move_vertical(-e.page_rows(), false, window, cx)
			}))
			.on_action(cx.listener(|e, _: &PageDown, window, cx| {
				e.move_vertical(e.page_rows(), false, window, cx)
			}))
			.on_action(cx.listener(|e, _: &SelectPageUp, window, cx| {
				e.move_vertical(-e.page_rows(), true, window, cx)
			}))
			.on_action(cx.listener(|e, _: &SelectPageDown, window, cx| {
				e.move_vertical(e.page_rows(), true, window, cx)
			}))
			.on_action(cx.listener(|e, _: &Backspace, _, cx| e.edit(cx, |e| e.state.backspace())))
			.on_action(cx.listener(|e, _: &Delete, _, cx| e.edit(cx, |e| e.state.delete_forward())))
			.on_action(cx.listener(|e, _: &DeleteWordLeft, _, cx| {
				e.edit(cx, |e| e.state.delete_word_left())
			}))
			.on_action(cx.listener(|e, _: &DeleteWordRight, _, cx| {
				e.edit(cx, |e| e.state.delete_word_right())
			}))
			.on_action(cx.listener(|e, _: &Newline, _, cx| {
				if !e.complete_key(CompleteKey::Accept, cx) {
					e.edit(cx, |e| e.state.newline())
				}
			}))
			.on_action(cx.listener(|e, _: &Tab, _, cx| {
				if !e.complete_key(CompleteKey::Accept, cx) {
					e.edit(cx, |e| e.state.tab(false))
				}
			}))
			.on_action(cx.listener(|e, _: &ShiftTab, _, cx| e.edit(cx, |e| e.state.tab(true))))
			.on_action(cx.listener(|e, _: &Undo, _, cx| {
				e.edit(cx, |e| {
					e.state.undo();
				})
			}))
			.on_action(cx.listener(|e, _: &Redo, _, cx| {
				e.edit(cx, |e| {
					e.state.redo();
				})
			}))
			.on_action(cx.listener(|e, _: &AddNextOccurrence, _, cx| {
				e.state.add_next_occurrence();
				e.after_command(cx);
			}))
			.on_action(cx.listener(|e, _: &SelectAllOccurrences, _, cx| {
				e.state.select_all_occurrences();
				e.after_command(cx);
			}))
			.on_action(cx.listener(|e, _: &Find, window, cx| e.open_find(false, window, cx)))
			.on_action(cx.listener(|e, _: &Replace, window, cx| e.open_find(true, window, cx)))
			.on_action(cx.listener(|e, _: &FindNext, _, cx| e.find_step(true, cx)))
			.on_action(cx.listener(|e, _: &FindPrevious, _, cx| e.find_step(false, cx)))
			.on_action(cx.listener(|e, _: &CollapseSelections, window, cx| {
				if e.complete_key(CompleteKey::Dismiss, cx) {
					return;
				}
				// Escape first drops extra carets, then closes the find bar.
				if e.find.is_some() && e.state.selections().len() == 1 {
					return e.close_find(window, cx);
				}
				e.state.collapse_to_primary();
				e.after_command(cx);
			}))
			.on_action(cx.listener(|e, _: &AddCursorAbove, window, cx| {
				e.add_cursor_vertical(-1., window, cx)
			}))
			.on_action(cx.listener(|e, _: &AddCursorBelow, window, cx| {
				e.add_cursor_vertical(1., window, cx)
			}))
			.on_action(
				cx.listener(|e, _: &MoveLinesUp, _, cx| e.edit(cx, |e| e.state.move_lines(false))),
			)
			.on_action(
				cx.listener(|e, _: &MoveLinesDown, _, cx| e.edit(cx, |e| e.state.move_lines(true))),
			)
			.on_action(
				cx.listener(|e, _: &CopyLinesUp, _, cx| e.edit(cx, |e| e.state.copy_lines(false))),
			)
			.on_action(
				cx.listener(|e, _: &CopyLinesDown, _, cx| e.edit(cx, |e| e.state.copy_lines(true))),
			)
			.on_action(
				cx.listener(|e, _: &DeleteLines, _, cx| e.edit(cx, |e| e.state.delete_lines())),
			)
			.on_action(cx.listener(|e, _: &InsertLineBelow, _, cx| {
				e.edit(cx, |e| e.state.insert_line(true))
			}))
			.on_action(cx.listener(|e, _: &InsertLineAbove, _, cx| {
				e.edit(cx, |e| e.state.insert_line(false))
			}))
			.on_action(cx.listener(|e, _: &SelectLines, _, cx| {
				e.state.select_lines();
				e.after_command(cx);
			}))
			.on_action(
				cx.listener(|e, _: &IndentLines, _, cx| {
					e.edit(cx, |e| e.state.indent_lines(false))
				}),
			)
			.on_action(
				cx.listener(|e, _: &OutdentLines, _, cx| {
					e.edit(cx, |e| e.state.indent_lines(true))
				}),
			)
			.on_action(cx.listener(|e, _: &Bold, _, cx| e.edit(cx, |e| e.state.toggle_wrap("**"))))
			.on_action(cx.listener(|e, _: &Italic, _, cx| e.edit(cx, |e| e.state.toggle_wrap("*"))))
			.on_action(
				cx.listener(|e, _: &InsertLink, _, cx| e.edit(cx, |e| e.state.insert_link())),
			)
			.on_action(cx.listener(|e, _: &ToggleLivePreview, _, cx| e.toggle_live(cx)))
			.on_action(cx.listener(|e, _: &SelectAll, _, cx| {
				e.preferred_x = None;
				e.state.select_all();
				cx.notify();
			}))
			.on_action(cx.listener(|e, _: &Copy, _, cx| {
				let (text, full) = e.state.copy();
				e.copy_to_clipboard(text, full, cx);
			}))
			.on_action(cx.listener(|e, _: &Cut, _, cx| {
				if e.doc.read_only {
					return;
				}
				e.marked_range = None;
				let (text, full) = e.state.cut();
				e.copy_to_clipboard(text, full, cx);
				e.edited(cx);
				e.after_command(cx);
			}))
			.on_action(cx.listener(|e, _: &Paste, window, cx| {
				let Some(item) = cx.read_from_clipboard() else {
					return;
				};
				if e.paste_image(&item, window, cx) {
					return;
				}
				let Some(text) = item.text() else {
					return;
				};
				let full_line = item.metadata().is_some_and(|m| m == FULL_LINE);
				let text = text.replace("\r\n", "\n");
				e.edit(cx, |e| e.state.paste_text(&text, full_line));
			}))
	}
}
