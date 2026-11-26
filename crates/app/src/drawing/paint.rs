//! Painting a drawing with GPUI paths: hand-drawn shapes, text, pictures, the dot grid.

use gpui_kit::*;
use ruby_core::drawing::color::{dark_mode, parse_color};
use ruby_core::drawing::geometry::{self, Rect};
use ruby_core::drawing::model::Element;
use ruby_core::drawing::rough::{self, PathOp, Shape};

use super::DrawingModel;

pub const MIN_ZOOM: f64 = 0.1;
pub const MAX_ZOOM: f64 = 10.;
const GRID: f64 = 20.;
const GRID_DOT: u32 = 0x2a2a2e;

/// Where the scene is on screen: `screen = origin + pan + scene * zoom`.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewTransform {
	pub origin: Point<Pixels>,
	pub pan: [f64; 2],
	pub zoom: f64,
}

impl ViewTransform {
	pub fn to_screen(&self, p: [f64; 2]) -> Point<Pixels> {
		point(
			self.origin.x + px((p[0] * self.zoom + self.pan[0]) as f32),
			self.origin.y + px((p[1] * self.zoom + self.pan[1]) as f32),
		)
	}

	pub fn to_scene(&self, p: Point<Pixels>) -> [f64; 2] {
		let local = p - self.origin;
		[
			(f64::from(f32::from(local.x)) - self.pan[0]) / self.zoom,
			(f64::from(f32::from(local.y)) - self.pan[1]) / self.zoom,
		]
	}

	/// The part of the scene shown in `bounds`.
	pub fn visible(&self, bounds: Bounds<Pixels>) -> Rect {
		Rect::from_points(
			self.to_scene(bounds.origin),
			self.to_scene(bounds.bottom_right()),
		)
	}
}

/// Pan and zoom that show `scene` centered in an area of `size`, `padding` pixels inside.
pub fn fit(scene: Rect, size: Size<Pixels>, padding: f64, max_zoom: f64) -> ([f64; 2], f64) {
	let (w, h) = (
		f64::from(f32::from(size.width)),
		f64::from(f32::from(size.height)),
	);
	let zoom = ((w - padding * 2.).max(1.) / scene.w.max(1.))
		.min((h - padding * 2.).max(1.) / scene.h.max(1.))
		.clamp(MIN_ZOOM, max_zoom);
	let c = scene.center();
	([w / 2. - c[0] * zoom, h / 2. - c[1] * zoom], zoom)
}

/// A stored (light theme) color as shown on the dark canvas; `None` when invisible.
pub fn display_color(color: &str, opacity: f64) -> Option<Rgba> {
	let [r, g, b, a] = parse_color(color)?;
	if a == 0 {
		return None;
	}
	let [r, g, b] = dark_mode([r, g, b]);
	Some(Rgba {
		r: f32::from(r) / 255.,
		g: f32::from(g) / 255.,
		b: f32::from(b) / 255.,
		a: f32::from(a) / 255. * (opacity.clamp(0., 100.) / 100.) as f32,
	})
}

/// The bundled font of an Excalidraw font family number.
pub fn font_family(family: i64) -> &'static str {
	match family {
		// Helvetica, Nunito, Lilita One.
		2 | 6 | 7 => "Inter",
		// Cascadia, Comic Shanns.
		3 | 8 => "JetBrains Mono",
		// Virgil, Excalifont and anything newer: hand-drawn.
		_ => "Kalam",
	}
}

pub fn paint_grid(t: &ViewTransform, bounds: Bounds<Pixels>, window: &mut Window) {
	let mut step = GRID * t.zoom;
	while step < 12. {
		step *= 2.;
	}
	let (w, h) = (
		f64::from(f32::from(bounds.size.width)),
		f64::from(f32::from(bounds.size.height)),
	);
	let mut y = t.pan[1].rem_euclid(step);
	while y < h {
		let mut x = t.pan[0].rem_euclid(step);
		while x < w {
			let at = point(
				bounds.left() + px(x as f32 - 1.),
				bounds.top() + px(y as f32 - 1.),
			);
			window.paint_quad(
				fill(Bounds::new(at, size(px(2.), px(2.))), rgb(GRID_DOT)).corner_radii(px(1.)),
			);
			x += step;
		}
		y += step;
	}
}

fn add_ops(builder: &mut PathBuilder, ops: &[PathOp], t: &ViewTransform) {
	for op in ops {
		match *op {
			PathOp::Move(p) => builder.move_to(t.to_screen(p)),
			PathOp::Line(p) => builder.line_to(t.to_screen(p)),
			PathOp::Curve { c1, c2, to } => {
				builder.cubic_bezier_to(t.to_screen(to), t.to_screen(c1), t.to_screen(c2))
			}
		}
	}
}

/// Strokes a polyline given in window coordinates.
#[allow(dead_code)]
pub fn stroke_polyline(
	points: &[Point<Pixels>],
	width: Pixels,
	color: impl Into<Background>,
	window: &mut Window,
) {
	let Some((first, rest)) = points.split_first() else {
		return;
	};
	let mut builder = PathBuilder::stroke(width);
	builder.move_to(*first);
	for p in rest {
		builder.line_to(*p);
	}
	if let Ok(path) = builder.build() {
		window.paint_path(path, color);
	}
}

fn paint_shape(el: &Element, shape: &Shape, t: &ViewTransform, window: &mut Window) {
	if let Some(area) = &shape.fill
		&& area.len() >= 3
		&& let Some(color) = display_color(&el.background_color, el.opacity)
	{
		let mut builder = PathBuilder::fill();
		builder.move_to(t.to_screen(area[0]));
		for p in &area[1..] {
			builder.line_to(t.to_screen(*p));
		}
		builder.close();
		if let Ok(path) = builder.build() {
			window.paint_path(path, color);
		}
	}
	if !shape.hachure.is_empty()
		&& let Some(color) = display_color(&el.background_color, el.opacity)
	{
		let mut builder = PathBuilder::stroke(px((el.stroke_width * 0.5 * t.zoom).max(0.5) as f32));
		for (a, b) in &shape.hachure {
			builder.move_to(t.to_screen(*a));
			builder.line_to(t.to_screen(*b));
		}
		if let Ok(path) = builder.build() {
			window.paint_path(path, color);
		}
	}
	let Some(color) = display_color(&el.stroke_color, el.opacity) else {
		return;
	};
	let width = px((el.stroke_width * t.zoom).max(0.75) as f32);
	let mut builder = PathBuilder::stroke(width);
	let mut any = false;
	match el.stroke_style.as_str() {
		style @ ("dashed" | "dotted") => {
			let (on, off) = if style == "dashed" {
				(8. + el.stroke_width, 8. + el.stroke_width)
			} else {
				(1.5, 4. + el.stroke_width * 2.)
			};
			for ops in &shape.strokes {
				for piece in rough::dash(&rough::flatten(ops, 0.5 / t.zoom), on, off) {
					let Some((first, rest)) = piece.split_first() else {
						continue;
					};
					builder.move_to(t.to_screen(*first));
					for p in rest {
						builder.line_to(t.to_screen(*p));
					}
					any = true;
				}
			}
		}
		_ => {
			for ops in &shape.strokes {
				add_ops(&mut builder, ops, t);
				any |= ops.len() > 1;
			}
		}
	}
	if any && let Ok(path) = builder.build() {
		window.paint_path(path, color);
	}
}

/// Width and height of `text` in scene units for a text element of this font.
pub fn measure_text(text: &str, font_size: f64, family: i64, window: &mut Window) -> (f64, f64) {
	let font = font(font_family(family));
	let mut width: f64 = 0.;
	let mut lines = 0;
	for line in text.split('\n') {
		lines += 1;
		if line.is_empty() {
			continue;
		}
		let run = TextRun {
			len: line.len(),
			font: font.clone(),
			color: Hsla::default(),
			background_color: None,
			underline: None,
			strikethrough: None,
		};
		let shaped = window.text_system().shape_line(
			SharedString::from(line.to_string()),
			px(font_size as f32),
			&[run],
			None,
		);
		width = width.max(f64::from(f32::from(shaped.width)));
	}
	(width.ceil(), (f64::from(lines) * font_size * 1.25).ceil())
}

fn paint_text(el: &Element, t: &ViewTransform, window: &mut Window, cx: &mut App) {
	let (Some(text), Some(color)) = (&el.text, display_color(&el.stroke_color, el.opacity)) else {
		return;
	};
	let font_size = px((el.font_size * t.zoom) as f32);
	if font_size < px(2.) {
		return;
	}
	let line_height = font_size * 1.25;
	let font = font(font_family(el.font_family));
	// Text is not rotated: it stays upright around the element's center.
	let origin = t.to_screen([el.x, el.y]);
	let box_width = px((el.width * t.zoom) as f32);
	for (i, line) in text.split('\n').enumerate() {
		if line.is_empty() {
			continue;
		}
		let run = TextRun {
			len: line.len(),
			font: font.clone(),
			color: color.into(),
			background_color: None,
			underline: None,
			strikethrough: None,
		};
		let shaped = window.text_system().shape_line(
			SharedString::from(line.to_string()),
			font_size,
			&[run],
			None,
		);
		let x = match el.text_align.as_str() {
			"center" => (box_width - shaped.width) / 2.,
			"right" => box_width - shaped.width,
			_ => px(0.),
		};
		shaped
			.paint(
				point(origin.x + x, origin.y + line_height * i as f32),
				line_height,
				TextAlign::Left,
				None,
				window,
				cx,
			)
			.ok();
	}
}

fn paint_picture(
	model: &DrawingModel,
	el: &Element,
	t: &ViewTransform,
	window: &mut Window,
	cx: &mut App,
) {
	let b = geometry::bounds(el);
	let bounds = Bounds::from_corners(t.to_screen([b.x, b.y]), t.to_screen([b.x + b.w, b.y + b.h]));
	match el.file_id.as_ref().and_then(|id| model.picture(id, cx)) {
		Some(image) => {
			window
				.paint_image(bounds, bounds, Corners::default(), image, 0, false)
				.ok();
		}
		None => window.paint_quad(quad(
			bounds,
			px(4.),
			rgba(0xffffff0a),
			px(1.),
			rgb(crate::theme::palette::BORDER),
			BorderStyle::Solid,
		)),
	}
}

/// Paints every visible element of the drawing. `hidden` is the text being edited.
pub fn paint_scene(
	model: &DrawingModel,
	t: &ViewTransform,
	clip: Bounds<Pixels>,
	hidden: Option<&str>,
	window: &mut Window,
	cx: &mut App,
) {
	let visible = t.visible(clip);
	for el in &model.editor.scene.elements {
		if el.is_deleted || hidden == Some(el.id.as_str()) {
			continue;
		}
		let b = geometry::rotated_bounds(el);
		let margin = el.stroke_width * 2. + 8.;
		if b.x - margin > visible.x + visible.w
			|| b.x + b.w + margin < visible.x
			|| b.y - margin > visible.y + visible.h
			|| b.y + b.h + margin < visible.y
		{
			continue;
		}
		match el.kind.as_str() {
			"text" => paint_text(el, t, window, cx),
			"image" => paint_picture(model, el, t, window, cx),
			"rectangle" | "diamond" | "ellipse" | "arrow" | "line" | "freedraw" => {
				paint_shape(el, &model.shape(el), t, window)
			}
			// Types this app does not know are kept in the file and not drawn.
			_ => {}
		}
	}
}
