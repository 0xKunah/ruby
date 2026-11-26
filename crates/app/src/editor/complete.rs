//! `[[` autocomplete: a list of the vault's notes, drawings and images under the caret.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ruby_core::editor_state::Selection;
use ruby_core::fuzzy::rank;
use ruby_core::links::{slash, vault_files};
use ruby_core::vault::{NodeKind, kind_for_path, scan_vault};

use super::NoteEditor;
use crate::theme::palette;

/// Rows shown at most.
const MAX_ROWS: usize = 8;

pub(crate) struct LinkComplete {
	/// Where the typed name starts (right after `[[`).
	start: usize,
	/// Label shown and matched, and the link target it inserts.
	files: Rc<Vec<(String, String)>>,
	/// Indexes in `files` of the rows shown.
	rows: Vec<usize>,
	selected: usize,
	/// Escape was pressed: the list stays closed for this link.
	dismissed: bool,
}

impl LinkComplete {
	#[cfg(test)]
	pub(crate) fn labels(&self) -> Vec<&str> {
		self.rows
			.iter()
			.map(|i| self.files[*i].0.as_str())
			.collect()
	}
}

pub(crate) enum CompleteKey {
	Up,
	Down,
	Accept,
	Dismiss,
}

/// Every linkable file of the vault: its label and the shortest target that names it.
fn linkable_files(vault: &std::path::Path) -> Vec<(String, String)> {
	let files: Vec<_> = vault_files(&scan_vault(vault).unwrap_or_default())
		.into_iter()
		.filter(|f| {
			matches!(
				kind_for_path(f),
				NodeKind::Note | NodeKind::Drawing | NodeKind::Image
			)
		})
		.collect();
	let name_of = |path: &str| path.rsplit('/').next().unwrap_or(path).to_lowercase();
	let labels: Vec<String> = files
		.iter()
		.map(|f| {
			let path = slash(f);
			path.strip_suffix(".md").unwrap_or(&path).to_string()
		})
		.collect();
	let mut names: HashMap<String, usize> = HashMap::new();
	for label in &labels {
		*names.entry(name_of(label)).or_default() += 1;
	}
	labels
		.into_iter()
		.map(|label| {
			// A name used once is enough; otherwise the path tells the files apart.
			let target = if names[&name_of(&label)] == 1 {
				label.rsplit('/').next().unwrap_or(&label).to_string()
			} else {
				label.clone()
			};
			(label, target)
		})
		.collect()
}

impl NoteEditor {
	/// Where the name being typed after `[[` starts, when the caret is in one.
	fn link_query_start(&self) -> Option<usize> {
		let [selection] = self.state.selections() else {
			return None;
		};
		if !selection.is_empty() {
			return None;
		}
		let text = self.state.text();
		let head = selection.head;
		let line_start = text[..head].rfind('\n').map_or(0, |i| i + 1);
		let open = line_start + text[line_start..head].rfind("[[")?;
		let query = &text[open + 2..head];
		(!query.contains(['[', ']', '|', '#'])).then_some(open + 2)
	}

	/// Opens, updates or closes the list after a change. It opens only when the text
	/// changed (`may_open`), not when the caret merely moves into a link.
	pub(crate) fn refresh_link_complete(&mut self, may_open: bool) {
		let start = self
			.link_query_start()
			.filter(|start| match &self.link_complete {
				Some(open) => open.start == *start,
				None => may_open && !self.doc.read_only,
			});
		let Some(start) = start else {
			self.link_complete = None;
			return;
		};
		let files = match &self.link_complete {
			Some(open) => open.files.clone(),
			None => Rc::new(linkable_files(&self.vault_root())),
		};
		let query = &self.state.text()[start..self.primary_head()];
		let rows: Vec<usize> = rank(query, files.iter().map(|f| f.0.as_str()), MAX_ROWS)
			.into_iter()
			.map(|(i, _)| i)
			.collect();
		// Kept even with no match or after Escape: the vault is scanned once per link.
		let dismissed = self
			.link_complete
			.as_ref()
			.is_some_and(|open| open.dismissed);
		self.link_complete = Some(LinkComplete {
			start,
			files,
			rows,
			selected: 0,
			dismissed,
		});
	}

	/// The list, when it is showing.
	pub(crate) fn link_list(&self) -> Option<&LinkComplete> {
		self.link_complete
			.as_ref()
			.filter(|open| !open.dismissed && !open.rows.is_empty())
	}

	/// Handles a key while the list is open; `false` when it is closed.
	pub(crate) fn complete_key(&mut self, key: CompleteKey, cx: &mut Context<Self>) -> bool {
		if self.link_list().is_none() {
			return false;
		}
		let Some(open) = &mut self.link_complete else {
			return false;
		};
		let len = open.rows.len();
		match key {
			CompleteKey::Up => open.selected = (open.selected + len - 1) % len,
			CompleteKey::Down => open.selected = (open.selected + 1) % len,
			CompleteKey::Dismiss => open.dismissed = true,
			CompleteKey::Accept => {
				let row = open.selected;
				self.accept_link(row, cx);
			}
		}
		cx.notify();
		true
	}

	/// Writes the chosen target between the brackets and leaves the caret after `]]`.
	fn accept_link(&mut self, row: usize, cx: &mut Context<Self>) {
		let Some(open) = self.link_complete.take() else {
			return;
		};
		let Some(index) = open.rows.get(row) else {
			return;
		};
		let mut target = open.files[*index].1.clone();
		let head = self.primary_head();
		let closed = self.state.text()[head..].starts_with("]]");
		if !closed {
			target.push_str("]]");
		}
		self.state.break_undo_group();
		self.state.replace_range(open.start..head, &target);
		self.state.break_undo_group();
		let caret = open.start + target.len() + if closed { 2 } else { 0 };
		self.state.set_selections(vec![Selection::caret(caret)]);
		self.edited(cx);
		self.after_command(cx);
	}

	/// The list, anchored under the `[[` it completes.
	pub(crate) fn render_link_complete(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
		let open = self.link_list()?;
		let layout = self.layout.as_ref()?;
		let bounds = self.last_bounds?;
		let at = layout.point_for_offset(open.start);
		let position =
			bounds.origin + at + point(px(-8.), layout.line_height_at(open.start) + px(4.));
		let rows = open.rows.iter().enumerate().map(|(row, index)| {
			let selected = row == open.selected;
			div()
				.id(("link-complete", row))
				.h(px(28.))
				.px(px(8.))
				.flex()
				.flex_none()
				.items_center()
				.rounded(px(5.))
				.text_size(px(12.5))
				.text_color(rgb(palette::TEXT))
				.whitespace_nowrap()
				.cursor_pointer()
				.when(selected, |d| d.bg(rgb(palette::MUTED2)))
				.hover(|d| d.bg(rgb(palette::MUTED2)))
				.on_mouse_down(
					MouseButton::Left,
					cx.listener(move |this, _, _, cx| {
						this.accept_link(row, cx);
						cx.stop_propagation();
					}),
				)
				.child(open.files[*index].0.clone())
		});
		Some(
			deferred(
				anchored().position(position).snap_to_window().child(
					div()
						.occlude()
						.w(px(300.))
						.p(px(4.))
						.flex()
						.flex_col()
						.rounded(px(8.))
						.border_1()
						.border_color(rgb(palette::BORDER))
						.bg(rgb(palette::SIDEBAR))
						.shadow_lg()
						.overflow_hidden()
						.children(rows),
				),
			)
			.into_any_element(),
		)
	}
}
