//! Tab strip above the editor (spec section 4: active tab has a 2 px accent top border).

use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ruby_core::vault::{display_name, kind_for_path};

use crate::theme::palette;
use crate::workspace::Workspace;

#[derive(Clone)]
pub struct DraggedTab {
	pub index: usize,
	pub title: SharedString,
}

impl Render for DraggedTab {
	fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
		div()
			.px(px(12.))
			.py(px(6.))
			.rounded(px(6.))
			.bg(rgb(palette::MUTED2))
			.border_1()
			.border_color(rgb(palette::BORDER))
			.text_size(px(12.5))
			.text_color(rgb(palette::FG))
			.child(self.title.clone())
	}
}

pub fn tab_title(path: &std::path::Path) -> String {
	let file_name = path
		.file_name()
		.map(|n| n.to_string_lossy().into_owned())
		.unwrap_or_default();
	display_name(&file_name, kind_for_path(path))
}

pub fn render_tab_bar(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
	let active = ws.tabs.active();
	let tabs = ws.tabs.tabs().iter().enumerate().map(|(ix, tab)| {
		let is_active = active == Some(ix);
		let title: SharedString = tab_title(&tab.path).into();
		div()
			.id(("tab", ix))
			.group("tab")
			.flex()
			.items_center()
			.gap(px(8.))
			.h(px(32.))
			.px(px(12.))
			.rounded_t(px(6.))
			.text_size(px(12.5))
			.cursor_pointer()
			.map(|d| {
				if is_active {
					d.relative()
						.bg(rgb(palette::BG))
						.text_color(rgb(palette::FG))
						.border_l_1()
						.border_r_1()
						.border_t_1()
						.border_color(rgb(palette::BORDER))
						.mb(px(-1.))
						.h(px(33.))
						.child(
							div()
								.absolute()
								.top(px(-1.))
								.left(px(-1.))
								.right(px(-1.))
								.h(px(2.))
								.rounded_t(px(6.))
								.bg(rgb(palette::ACCENT)),
						)
				} else {
					d.text_color(rgb(palette::FG3))
						.hover(|s| s.text_color(rgb(palette::FG2)))
				}
			})
			.child(title.clone())
			.child(
				div()
					.id(("tab-close", ix))
					.rounded(px(4.))
					.p(px(2.))
					.hover(|s| s.bg(rgb(palette::MUTED2)))
					.when(!is_active, |d| {
						d.invisible().group_hover("tab", |s| s.visible())
					})
					.child(Icon::new(IconName::Close).xsmall())
					.on_click(cx.listener(move |this, _, window, cx| {
						cx.stop_propagation();
						this.close_tab_at(ix, window, cx);
					})),
			)
			.on_click(cx.listener(move |this, _, window, cx| this.activate_tab(ix, window, cx)))
			.on_mouse_down(
				MouseButton::Middle,
				cx.listener(move |this, _, window, cx| this.close_tab_at(ix, window, cx)),
			)
			.on_drag(
				DraggedTab {
					index: ix,
					title: title.clone(),
				},
				|dragged, _, _, cx| cx.new(|_| dragged.clone()),
			)
			.drag_over::<DraggedTab>(|s, _, _, _| s.bg(rgb(palette::MUTED2)))
			.on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
				this.move_tab(dragged.index, ix, window, cx);
			}))
	});
	div()
		.id("tab-bar")
		.h(px(38.))
		.flex_none()
		.flex()
		.items_end()
		.gap(px(2.))
		.px(px(8.))
		.bg(rgb(palette::SIDEBAR))
		.border_b_1()
		.border_color(rgb(palette::BORDER))
		.overflow_x_scroll()
		.children(tabs)
}
