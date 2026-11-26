//! Right panel "PLAN": headings of the active note, the one under the caret highlighted.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::editor::NoteEditor;
use crate::i18n::tr;
use crate::theme::palette;
use crate::workspace::Workspace;

pub fn render_outline(
	editor: Option<Entity<NoteEditor>>,
	cx: &mut Context<Workspace>,
) -> impl IntoElement {
	let (headings, head) = match &editor {
		Some(e) => e.update(cx, |e, _| (e.stats().headings.clone(), e.primary_head())),
		None => (Vec::new(), 0),
	};
	let t = tr(cx);
	let min_level = headings.iter().map(|h| h.level).min().unwrap_or(1);
	let current = headings.iter().rposition(|h| h.offset <= head);
	let rows = headings.into_iter().enumerate().map(|(ix, h)| {
		let is_current = current == Some(ix);
		let editor = editor.clone();
		div()
			.id(("outline", ix))
			.ml(px(10. * f32::from(h.level - min_level)))
			.pl(px(8.))
			.py(px(3.))
			.border_l_2()
			.border_color(if is_current {
				rgb(palette::ACCENT)
			} else {
				rgba(0x00000000)
			})
			.text_size(px(12.))
			.text_color(rgb(if is_current {
				palette::FG
			} else {
				palette::FG3
			}))
			.overflow_hidden()
			.text_ellipsis()
			.whitespace_nowrap()
			.cursor_pointer()
			.hover(|s| s.text_color(rgb(palette::FG2)))
			.child(h.text)
			.on_click(move |_, window, cx| {
				if let Some(editor) = &editor {
					editor.update(cx, |e, cx| e.scroll_to_offset(h.offset, window, cx));
				}
			})
	});
	let rows: Vec<_> = rows.collect();
	let empty = rows.is_empty();
	div()
		.id("outline")
		.size_full()
		.flex()
		.flex_col()
		.gap(px(1.))
		.p(px(12.))
		.bg(rgb(palette::SIDEBAR))
		.border_l_1()
		.border_color(rgb(palette::BORDER))
		.overflow_y_scroll()
		.child(
			div()
				.mb(px(8.))
				.text_size(px(11.))
				.font_weight(FontWeight::SEMIBOLD)
				.text_color(rgb(palette::FG2))
				.child(t.outline_title),
		)
		.when(empty, |d| {
			d.child(
				div()
					.text_size(px(12.))
					.text_color(rgb(palette::FG3))
					.child(t.no_headings),
			)
		})
		.children(rows)
}
