//! Status bar under the editor: mode (click to toggle), word count, caret position; element
//! count and zoom for a drawing.

use gpui_kit::*;

use crate::drawing::DrawingView;
use crate::editor::NoteEditor;
use crate::i18n::{fmt, tr};
use crate::theme::palette;

pub fn render_status_bar(
	editor: Option<&Entity<NoteEditor>>,
	drawing: Option<&Entity<DrawingView>>,
	cx: &mut App,
) -> impl IntoElement {
	let bar = div()
		.h(px(26.))
		.flex_none()
		.flex()
		.items_center()
		.justify_end()
		.gap(px(16.))
		.px(px(14.))
		.bg(rgb(palette::SIDEBAR))
		.border_t_1()
		.border_color(rgb(palette::BORDER))
		.text_size(px(11.))
		.text_color(rgb(palette::FG3));
	if let Some(drawing) = drawing {
		let view = drawing.read(cx);
		let count = view.model.read(cx).element_count();
		let zoom = view.zoom_percent();
		return bar
			.child(fmt(tr(cx).elements, &[&count]))
			.child(format!("{zoom}%"));
	}
	let Some(editor) = editor else {
		return bar;
	};
	let (mode, words, position) = editor.update(cx, |e, cx| {
		let t = tr(cx);
		let words = e.stats().words;
		let (line, col) = e.state.line_col(e.primary_head());
		let mode = if e.live { t.mode_live } else { t.mode_source };
		(
			mode,
			fmt(t.words, &[&words]),
			fmt(t.position, &[&line, &col]),
		)
	});
	let toggle = editor.clone();
	bar.child(
		div()
			.id("status-mode")
			.cursor_pointer()
			.hover(|s| s.text_color(rgb(palette::FG2)))
			.child(mode)
			.on_click(move |_, _, cx| toggle.update(cx, |e, cx| e.toggle_live(cx))),
	)
	.child(words)
	.child(position)
}
