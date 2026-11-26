//! Geometry of drawing elements: bounds, hit testing, resize and rotate handles, and where
//! arrows attach to shapes. Scene coordinates, y down, angles in radians.

use std::f64::consts::{FRAC_PI_2, PI};

use super::model::Element;

pub type Point = [f64; 2];

/// An axis-aligned box with a positive size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
	pub x: f64,
	pub y: f64,
	pub w: f64,
	pub h: f64,
}

impl Rect {
	pub fn from_points(a: Point, b: Point) -> Rect {
		Rect {
			x: a[0].min(b[0]),
			y: a[1].min(b[1]),
			w: (a[0] - b[0]).abs(),
			h: (a[1] - b[1]).abs(),
		}
	}

	pub fn center(&self) -> Point {
		[self.x + self.w / 2., self.y + self.h / 2.]
	}

	pub fn contains(&self, p: Point) -> bool {
		p[0] >= self.x && p[0] <= self.x + self.w && p[1] >= self.y && p[1] <= self.y + self.h
	}

	fn union(&self, other: &Rect) -> Rect {
		let (x, y) = (self.x.min(other.x), self.y.min(other.y));
		Rect {
			x,
			y,
			w: (self.x + self.w).max(other.x + other.w) - x,
			h: (self.y + self.h).max(other.y + other.h) - y,
		}
	}

	fn corners(&self) -> [Point; 4] {
		[
			[self.x, self.y],
			[self.x + self.w, self.y],
			[self.x + self.w, self.y + self.h],
			[self.x, self.y + self.h],
		]
	}
}

/// `p` turned by `angle` around `center`.
pub fn rotate(p: Point, center: Point, angle: f64) -> Point {
	let (sin, cos) = angle.sin_cos();
	let (dx, dy) = (p[0] - center[0], p[1] - center[1]);
	[
		center[0] + dx * cos - dy * sin,
		center[1] + dx * sin + dy * cos,
	]
}

pub fn is_linear(el: &Element) -> bool {
	matches!(el.kind.as_str(), "line" | "arrow" | "freedraw")
}

/// The points of a line, arrow or freedraw in scene coordinates (before rotation).
pub fn abs_points(el: &Element) -> Vec<Point> {
	el.points
		.iter()
		.flatten()
		.map(|p| [el.x + p[0], el.y + p[1]])
		.collect()
}

/// The element's box before rotation.
pub fn bounds(el: &Element) -> Rect {
	if is_linear(el) {
		let points = abs_points(el);
		if let Some(first) = points.first() {
			return points
				.iter()
				.fold(Rect::from_points(*first, *first), |r, p| {
					r.union(&Rect::from_points(*p, *p))
				});
		}
	}
	Rect::from_points([el.x, el.y], [el.x + el.width, el.y + el.height])
}

pub fn center(el: &Element) -> Point {
	bounds(el).center()
}

/// The axis-aligned box around the rotated element.
pub fn rotated_bounds(el: &Element) -> Rect {
	let b = bounds(el);
	if el.angle == 0. {
		return b;
	}
	let c = b.center();
	let corners = b.corners().map(|p| rotate(p, c, el.angle));
	corners[1..]
		.iter()
		.fold(Rect::from_points(corners[0], corners[0]), |r, p| {
			r.union(&Rect::from_points(*p, *p))
		})
}

/// The box around every visible element.
pub fn scene_bounds(elements: &[Element]) -> Option<Rect> {
	elements
		.iter()
		.filter(|e| !e.is_deleted)
		.map(rotated_bounds)
		.reduce(|a, b| a.union(&b))
}

/// Sets `width` and `height` of a line from its points, with its first point at `(x, y)`.
pub fn fit_linear(el: &mut Element) {
	let Some(points) = &mut el.points else {
		return;
	};
	let Some(first) = points.first().copied() else {
		return;
	};
	for p in points.iter_mut() {
		*p = [p[0] - first[0], p[1] - first[1]];
	}
	el.x += first[0];
	el.y += first[1];
	let b = bounds(el);
	el.width = b.w;
	el.height = b.h;
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
	let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
	let len2 = dx * dx + dy * dy;
	let t = if len2 == 0. {
		0.
	} else {
		(((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0., 1.)
	};
	((p[0] - a[0] - t * dx).powi(2) + (p[1] - a[1] - t * dy).powi(2)).sqrt()
}

fn is_filled(el: &Element) -> bool {
	!el.background_color.eq_ignore_ascii_case("transparent") && !el.background_color.is_empty()
}

/// The corners of a diamond in `b`: top, right, bottom, left.
fn diamond_corners(b: &Rect) -> [Point; 4] {
	let c = b.center();
	[
		[c[0], b.y],
		[b.x + b.w, c[1]],
		[c[0], b.y + b.h],
		[b.x, c[1]],
	]
}

/// How far `p` is outside the shape (negative inside), for a point in the element's own
/// unrotated frame.
fn signed_distance(el: &Element, b: &Rect, p: Point) -> f64 {
	let c = b.center();
	let (dx, dy) = ((p[0] - c[0]).abs(), (p[1] - c[1]).abs());
	let (hw, hh) = (b.w / 2., b.h / 2.);
	match el.kind.as_str() {
		"ellipse" => {
			if hw <= 0. || hh <= 0. {
				return (dx * dx + dy * dy).sqrt();
			}
			// Scaled distance: exact on the axes, close enough elsewhere for a few pixels.
			let k = ((dx / hw).powi(2) + (dy / hh).powi(2)).sqrt();
			if k == 0. {
				-hw.min(hh)
			} else {
				(dx * dx + dy * dy).sqrt() * (1. - 1. / k)
			}
		}
		"diamond" => {
			let corners = diamond_corners(b);
			let edge = (0..4)
				.map(|i| distance_to_segment(p, corners[i], corners[(i + 1) % 4]))
				.fold(f64::INFINITY, f64::min);
			let inside = hw > 0. && hh > 0. && dx / hw + dy / hh <= 1.;
			if inside { -edge } else { edge }
		}
		_ => {
			let (ox, oy) = (dx - hw, dy - hh);
			if ox <= 0. && oy <= 0. {
				ox.max(oy)
			} else {
				(ox.max(0.).powi(2) + oy.max(0.).powi(2)).sqrt()
			}
		}
	}
}

/// Whether a click at `p` selects the element: on the outline of an unfilled shape,
/// anywhere inside a filled one, text or an image, near a segment of a line.
pub fn hit_test(el: &Element, p: Point, tolerance: f64) -> bool {
	let b = bounds(el);
	let local = rotate(p, b.center(), -el.angle);
	if is_linear(el) {
		let points = abs_points(el);
		let reach = tolerance + el.stroke_width / 2.;
		return match points.as_slice() {
			[] => false,
			[only] => distance_to_segment(local, *only, *only) <= reach,
			_ => points
				.windows(2)
				.any(|w| distance_to_segment(local, w[0], w[1]) <= reach),
		};
	}
	if b.w == 0. && b.h == 0. {
		return false;
	}
	let d = signed_distance(el, &b, local);
	if is_filled(el) || matches!(el.kind.as_str(), "text" | "image") {
		d <= tolerance
	} else {
		d.abs() <= tolerance
	}
}

/// Indexes of the visible elements entirely inside `rect` (marquee selection).
pub fn elements_in_rect(elements: &[Element], rect: Rect) -> Vec<usize> {
	elements
		.iter()
		.enumerate()
		.filter(|(_, e)| !e.is_deleted)
		.filter(|(_, e)| {
			let b = rotated_bounds(e);
			rect.contains([b.x, b.y]) && rect.contains([b.x + b.w, b.y + b.h])
		})
		.map(|(i, _)| i)
		.collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
	N,
	S,
	E,
	W,
	NE,
	NW,
	SE,
	SW,
	Rotate,
}

/// Distance of the rotate handle above the box, in screen pixels.
pub const ROTATE_HANDLE_DISTANCE: f64 = 24.;
/// How close to a handle a click must be, in screen pixels.
const HANDLE_REACH: f64 = 8.;

/// Where each handle is in the scene, for a view at `zoom`.
pub fn handle_positions(el: &Element, zoom: f64) -> [(Handle, Point); 9] {
	let b = bounds(el);
	let c = b.center();
	let (l, t, r, bt) = (b.x, b.y, b.x + b.w, b.y + b.h);
	[
		(Handle::NW, [l, t]),
		(Handle::N, [c[0], t]),
		(Handle::NE, [r, t]),
		(Handle::E, [r, c[1]]),
		(Handle::SE, [r, bt]),
		(Handle::S, [c[0], bt]),
		(Handle::SW, [l, bt]),
		(Handle::W, [l, c[1]]),
		(Handle::Rotate, [c[0], t - ROTATE_HANDLE_DISTANCE / zoom]),
	]
	.map(|(handle, p)| (handle, rotate(p, c, el.angle)))
}

/// The handle under `p`, if any (corners win over edges).
pub fn handle_at(el: &Element, p: Point, zoom: f64) -> Option<Handle> {
	let reach = HANDLE_REACH / zoom;
	let mut best: Option<(f64, bool, Handle)> = None;
	for (handle, at) in handle_positions(el, zoom) {
		let d = ((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2)).sqrt();
		let edge = matches!(handle, Handle::N | Handle::S | Handle::E | Handle::W);
		if d <= reach && best.is_none_or(|(bd, bedge, _)| (edge, d) < (bedge, bd)) {
			best = Some((d, edge, handle));
		}
	}
	best.map(|(_, _, handle)| handle)
}

/// Resizes `el` (a copy of `original`, the element when the drag started) so that
/// `handle` follows the pointer at `to`. The opposite side stays where it was, also for a
/// rotated element; `keep_ratio` keeps the proportions.
pub fn resize(el: &mut Element, original: &Element, handle: Handle, to: Point, keep_ratio: bool) {
	let b = bounds(original);
	let c = b.center();
	let q = rotate(to, c, -original.angle);
	let (mut l, mut t, mut r, mut bt) = (b.x, b.y, b.x + b.w, b.y + b.h);
	let (west, east) = (
		matches!(handle, Handle::W | Handle::NW | Handle::SW),
		matches!(handle, Handle::E | Handle::NE | Handle::SE),
	);
	let (north, south) = (
		matches!(handle, Handle::N | Handle::NW | Handle::NE),
		matches!(handle, Handle::S | Handle::SW | Handle::SE),
	);
	if west {
		l = q[0];
	}
	if east {
		r = q[0];
	}
	if north {
		t = q[1];
	}
	if south {
		bt = q[1];
	}
	if keep_ratio && b.w > 0. && b.h > 0. {
		let scale = ((r - l).abs() / b.w).max((bt - t).abs() / b.h);
		let (w, h) = (b.w * scale, b.h * scale);
		// Grow from the side opposite to the handle (from the middle for an edge handle).
		if west {
			l = r - w;
		} else if east {
			r = l + w;
		} else {
			(l, r) = (c[0] - w / 2., c[0] + w / 2.);
		}
		if north {
			t = bt - h;
		} else if south {
			bt = t + h;
		} else {
			(t, bt) = (c[1] - h / 2., c[1] + h / 2.);
		}
	}
	let new = Rect::from_points([l, t], [r, bt]);
	let (w, h) = (new.w.max(1.), new.h.max(1.));
	// The new center, back in the scene: the untouched side does not move.
	let nc = rotate([new.x + w / 2., new.y + h / 2.], c, original.angle);
	let (nx, ny) = (nc[0] - w / 2., nc[1] - h / 2.);
	if is_linear(original) {
		let (sx, sy) = (
			if b.w > 0. { w / b.w } else { 1. },
			if b.h > 0. { h / b.h } else { 1. },
		);
		let scaled: Vec<Point> = abs_points(original)
			.iter()
			.map(|p| [nx + (p[0] - b.x) * sx, ny + (p[1] - b.y) * sy])
			.collect();
		if let Some(first) = scaled.first().copied() {
			el.x = first[0];
			el.y = first[1];
			el.points = Some(
				scaled
					.iter()
					.map(|p| [p[0] - first[0], p[1] - first[1]])
					.collect(),
			);
		}
		el.width = w;
		el.height = h;
		return;
	}
	el.x = nx;
	el.y = ny;
	el.width = w;
	el.height = h;
	if original.kind == "text" && b.h > 0. {
		el.font_size = (original.font_size * h / b.h).max(4.);
	}
}

/// Turns `el` so that its rotate handle points at `p`; `snap` rounds to 15 degrees.
pub fn rotate_to(el: &mut Element, p: Point, snap: bool) {
	let c = center(el);
	let mut angle = (p[1] - c[1]).atan2(p[0] - c[0]) + FRAC_PI_2;
	if snap {
		let step = PI / 12.;
		angle = (angle / step).round() * step;
	}
	// Kept in [0, 2 pi).
	el.angle = angle.rem_euclid(2. * PI);
	if (el.angle - 2. * PI).abs() < 1e-12 {
		el.angle = 0.;
	}
}

fn can_bind(el: &Element) -> bool {
	!el.is_deleted
		&& matches!(
			el.kind.as_str(),
			"rectangle" | "diamond" | "ellipse" | "text" | "image"
		)
}

/// Where a line from the shape's center toward `toward` leaves its outline, `gap` further
/// out: the point an arrow bound to the shape ends at.
pub fn outline_point(shape: &Element, toward: Point, gap: f64) -> Point {
	let b = bounds(shape);
	let c = b.center();
	let local = rotate(toward, c, -shape.angle);
	let (mut dx, mut dy) = (local[0] - c[0], local[1] - c[1]);
	let len = (dx * dx + dy * dy).sqrt();
	if len < 1e-9 {
		(dx, dy) = (1., 0.);
	} else {
		(dx, dy) = (dx / len, dy / len);
	}
	let (hw, hh) = ((b.w / 2.).max(1e-9), (b.h / 2.).max(1e-9));
	// Distance from the center to the outline along the direction.
	let reach = match shape.kind.as_str() {
		"ellipse" => 1. / ((dx / hw).powi(2) + (dy / hh).powi(2)).sqrt(),
		"diamond" => 1. / (dx.abs() / hw + dy.abs() / hh),
		_ => (hw / dx.abs().max(1e-12)).min(hh / dy.abs().max(1e-12)),
	};
	let on = [c[0] + dx * (reach + gap), c[1] + dy * (reach + gap)];
	rotate(on, c, shape.angle)
}

/// The topmost shape an arrow end dropped at `p` attaches to: one that contains `p` or
/// whose outline is within `max_distance`.
pub fn bindable_at(elements: &[Element], p: Point, max_distance: f64) -> Option<usize> {
	elements.iter().enumerate().rev().find_map(|(i, el)| {
		if !can_bind(el) {
			return None;
		}
		let b = bounds(el);
		let local = rotate(p, b.center(), -el.angle);
		(signed_distance(el, &b, local) <= max_distance).then_some(i)
	})
}

#[cfg(test)]
mod tests {
	use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

	use super::*;
	use crate::drawing::model::Element;
	use crate::drawing::rng::Rng;

	fn el(kind: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
		let mut e = Element::new(kind, x, y, &mut Rng::new(1));
		e.width = w;
		e.height = h;
		e
	}

	fn line(x: f64, y: f64, points: &[[f64; 2]]) -> Element {
		let mut e = el("line", x, y, 0., 0.);
		e.points = Some(points.to_vec());
		fit_linear(&mut e);
		e
	}

	fn near(a: Point, b: Point) -> bool {
		(a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
	}

	#[test]
	fn rotation_and_bounds() {
		assert!(near(rotate([1., 0.], [0., 0.], FRAC_PI_2), [0., 1.]));
		assert!(near(rotate([2., 1.], [1., 1.], PI), [0., 1.]));
		let l = line(5., 5., &[[0., 0.], [-10., 20.]]);
		assert_eq!(
			bounds(&l),
			Rect {
				x: -5.,
				y: 5.,
				w: 10.,
				h: 20.
			}
		);
		assert_eq!((l.width, l.height), (10., 20.));
		let mut r = el("rectangle", 0., 0., 100., 50.);
		r.angle = FRAC_PI_2;
		let b = rotated_bounds(&r);
		assert!(near([b.x, b.y], [25., -25.]) && near([b.w, b.h], [50., 100.]));
		// A box drawn right to left has a negative size: bounds are normalized.
		let flipped = el("rectangle", 10., 10., -10., -4.);
		assert_eq!(
			bounds(&flipped),
			Rect {
				x: 0.,
				y: 6.,
				w: 10.,
				h: 4.
			}
		);
		let all = scene_bounds(&[r.clone(), l.clone()]).unwrap();
		assert!(near([all.x, all.y], [-5., -25.]));
		assert!(scene_bounds(&[]).is_none());
		let mut gone = r.clone();
		gone.is_deleted = true;
		assert!(scene_bounds(&[gone]).is_none());
	}

	#[test]
	fn hit_testing_shapes() {
		let mut r = el("rectangle", 0., 0., 100., 60.);
		assert!(hit_test(&r, [0., 30.], 4.));
		assert!(hit_test(&r, [102., 30.], 4.));
		assert!(
			!hit_test(&r, [50., 30.], 4.),
			"an unfilled shape is hit on its outline only"
		);
		assert!(!hit_test(&r, [110., 30.], 4.));
		r.background_color = "#ffc9c9".into();
		assert!(hit_test(&r, [50., 30.], 4.));
		r.angle = FRAC_PI_4;
		assert!(
			!hit_test(&r, [2., 2.], 1.),
			"the unrotated corner is outside once rotated"
		);
		assert!(hit_test(&r, [50., 30.], 1.));
		let e = el("ellipse", 0., 0., 100., 60.);
		assert!(hit_test(&e, [100., 30.], 3.));
		assert!(hit_test(&e, [50., 0.], 3.));
		assert!(!hit_test(&e, [50., 30.], 3.));
		assert!(
			!hit_test(&e, [2., 2.], 3.),
			"the corner of the box is outside the ellipse"
		);
		let d = el("diamond", 0., 0., 100., 100.);
		assert!(hit_test(&d, [50., 0.], 3.));
		assert!(hit_test(&d, [25., 25.], 3.));
		assert!(!hit_test(&d, [50., 50.], 3.));
		assert!(!hit_test(&d, [5., 5.], 3.));
		let l = line(0., 0., &[[0., 0.], [100., 0.], [100., 100.]]);
		assert!(hit_test(&l, [50., 3.], 4.));
		assert!(hit_test(&l, [98., 50.], 4.));
		assert!(!hit_test(&l, [50., 50.], 4.));
		let t = el("text", 0., 0., 80., 25.);
		assert!(
			hit_test(&t, [40., 12.], 2.),
			"text is hit anywhere in its box"
		);
		let i = el("image", 0., 0., 80., 25.);
		assert!(hit_test(&i, [40., 12.], 2.));
		// A zero-size element does not divide by zero.
		assert!(!hit_test(&el("ellipse", 0., 0., 0., 0.), [50., 50.], 2.));
	}

	#[test]
	fn marquee_takes_fully_enclosed_elements() {
		let a = el("rectangle", 0., 0., 10., 10.);
		let b = el("rectangle", 50., 50., 10., 10.);
		let mut c = el("rectangle", 2., 2., 2., 2.);
		c.is_deleted = true;
		let found = elements_in_rect(
			&[a, b, c],
			Rect {
				x: -1.,
				y: -1.,
				w: 20.,
				h: 20.,
			},
		);
		assert_eq!(found, [0]);
		assert_eq!(
			Rect::from_points([10., 10.], [0., 4.]),
			Rect {
				x: 0.,
				y: 4.,
				w: 10.,
				h: 6.
			}
		);
	}

	#[test]
	fn handles() {
		let r = el("rectangle", 0., 0., 100., 60.);
		assert_eq!(handle_at(&r, [100., 60.], 1.), Some(Handle::SE));
		assert_eq!(handle_at(&r, [0., 0.], 1.), Some(Handle::NW));
		assert_eq!(handle_at(&r, [50., 60.], 1.), Some(Handle::S));
		assert_eq!(
			handle_at(&r, [50., -ROTATE_HANDLE_DISTANCE], 1.),
			Some(Handle::Rotate)
		);
		assert_eq!(handle_at(&r, [50., 30.], 1.), None);
		// Handles keep their screen size: at zoom 4 they are 4 times smaller in the scene.
		assert_eq!(handle_at(&r, [105., 65.], 1.), Some(Handle::SE));
		assert_eq!(handle_at(&r, [105., 65.], 4.), None);
		let mut rotated = r.clone();
		rotated.angle = FRAC_PI_2;
		// Rotated a quarter turn around (50, 30): the top right corner is now at the bottom right.
		assert_eq!(handle_at(&rotated, [80., 80.], 1.), Some(Handle::NE));
	}

	#[test]
	fn resizing() {
		let orig = el("rectangle", 10., 10., 100., 60.);
		let mut r = orig.clone();
		resize(&mut r, &orig, Handle::E, [150., 500.], false);
		assert_eq!((r.x, r.y, r.width, r.height), (10., 10., 140., 60.));
		let mut r = orig.clone();
		resize(&mut r, &orig, Handle::NW, [0., 0.], false);
		assert_eq!((r.x, r.y, r.width, r.height), (0., 0., 110., 70.));
		let mut r = orig.clone();
		resize(&mut r, &orig, Handle::SE, [210., 70.], true);
		assert!(
			near([r.width, r.height], [200., 120.]),
			"{} {}",
			r.width,
			r.height
		);
		// Dragged past the opposite edge: the box flips, its size stays positive.
		let mut r = orig.clone();
		resize(&mut r, &orig, Handle::E, [0., 0.], false);
		assert_eq!((r.x, r.width), (0., 10.));
		// A rotated element keeps its opposite edge where it was.
		let mut orig = el("rectangle", 0., 0., 100., 60.);
		orig.angle = FRAC_PI_2;
		let fixed = |e: &Element| rotate([e.x, e.y + e.height / 2.], center(e), e.angle);
		let mut r = orig.clone();
		// The E handle of this element points down: drag it 40 lower.
		resize(&mut r, &orig, Handle::E, [50., 120.], false);
		assert!(
			near([r.width, r.height], [140., 60.]),
			"{} {}",
			r.width,
			r.height
		);
		assert!(near(fixed(&r), fixed(&orig)));
		// Lines scale their points.
		let orig = line(0., 0., &[[0., 0.], [100., 50.]]);
		let mut l = orig.clone();
		resize(&mut l, &orig, Handle::SE, [200., 100.], false);
		assert_eq!(l.points.as_deref(), Some(&[[0., 0.], [200., 100.]][..]));
		assert_eq!((l.width, l.height), (200., 100.));
		// Text scales its font with its height.
		let mut orig = el("text", 0., 0., 80., 25.);
		orig.text = Some("x".into());
		let mut t = orig.clone();
		resize(&mut t, &orig, Handle::SE, [160., 50.], false);
		assert_eq!(t.font_size, 40.);
	}

	#[test]
	fn rotating() {
		let mut r = el("rectangle", 0., 0., 100., 60.);
		rotate_to(&mut r, [200., 30.], false);
		assert!((r.angle - FRAC_PI_2).abs() < 1e-9);
		rotate_to(&mut r, [50., -100.], false);
		assert!(r.angle.abs() < 1e-9);
		// Snapping goes to multiples of 15 degrees.
		rotate_to(&mut r, [200., 25.], true);
		assert!((r.angle - FRAC_PI_2).abs() < 1e-9);
	}

	#[test]
	fn arrows_bind_to_outlines() {
		let r = el("rectangle", 0., 0., 100., 100.);
		assert!(near(outline_point(&r, [200., 50.], 0.), [100., 50.]));
		assert!(near(outline_point(&r, [200., 50.], 5.), [105., 50.]));
		assert!(near(outline_point(&r, [50., -80.], 0.), [50., 0.]));
		let e = el("ellipse", 0., 0., 100., 100.);
		assert!(near(outline_point(&e, [200., 50.], 0.), [100., 50.]));
		let d = el("diamond", 0., 0., 100., 100.);
		assert!(near(outline_point(&d, [200., 50.], 0.), [100., 50.]));
		let p = outline_point(&d, [150., 150.], 0.);
		assert!(near(p, [75., 75.]), "{p:?}");
		// Toward the center itself: any point of the outline, never NaN.
		let p = outline_point(&r, [50., 50.], 0.);
		assert!(p[0].is_finite() && p[1].is_finite());
		let arrow = line(300., 300., &[[0., 0.], [10., 0.]]);
		let scene = [r.clone(), arrow, el("ellipse", 80., 0., 100., 100.)];
		assert_eq!(bindable_at(&scene, [-6., 50.], 10.), Some(0));
		assert_eq!(
			bindable_at(&scene, [50., 50.], 10.),
			Some(0),
			"inside a shape binds to it"
		);
		assert_eq!(
			bindable_at(&scene, [95., 50.], 10.),
			Some(2),
			"the topmost shape wins"
		);
		assert_eq!(
			bindable_at(&scene, [305., 300.], 10.),
			None,
			"arrows are not bindable"
		);
		assert_eq!(bindable_at(&scene, [-40., 50.], 10.), None);
	}
}
