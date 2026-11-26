//! Hand-drawn outlines of elements, in the manner of rough.js: every line is drawn twice
//! with slightly random ends and a slight bow. The randomness comes from the element's
//! seed, so a shape looks the same on every frame and wherever it is moved.

use std::f64::consts::PI;

use super::geometry::{self, Point, Rect, rotate};
use super::model::Element;
use super::rng::Rng;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathOp {
	Move(Point),
	Line(Point),
	/// A cubic Bezier curve.
	Curve {
		c1: Point,
		c2: Point,
		to: Point,
	},
}

/// What to paint for an element, in scene coordinates.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shape {
	/// Open or closed paths, stroked.
	pub strokes: Vec<Vec<PathOp>>,
	/// The area to fill with the background color (solid fill).
	pub fill: Option<Vec<Point>>,
	/// Hatch lines (hachure and cross-hatch fills).
	pub hachure: Vec<(Point, Point)>,
}

fn lerp(a: Point, b: Point, t: f64) -> Point {
	[a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn length(a: Point, b: Point) -> f64 {
	((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

/// How far a point may stray for a line of `len` at this roughness.
fn wobble(roughness: f64, len: f64) -> f64 {
	(roughness * (1. + len.min(200.) / 200.)).min(len * 0.25 + 0.5)
}

fn jitter(p: Point, amount: f64, rng: &mut Rng) -> Point {
	[p[0] + rng.offset(amount), p[1] + rng.offset(amount)]
}

/// One pen pass from `a` to `b`: random ends, a slight bow.
fn rough_pass(a: Point, b: Point, roughness: f64, rng: &mut Rng) -> Vec<PathOp> {
	let len = length(a, b);
	let off = wobble(roughness, len);
	let (start, end) = (jitter(a, off, rng), jitter(b, off, rng));
	// Perpendicular to the line, for the bow.
	let normal = if len > 0. {
		[-(b[1] - a[1]) / len, (b[0] - a[0]) / len]
	} else {
		[0., 0.]
	};
	let bow = rng.offset(roughness * len.min(200.) / 200.);
	let bowed = |t: f64| {
		let p = lerp(a, b, t);
		[p[0] + normal[0] * bow, p[1] + normal[1] * bow]
	};
	let c1 = jitter(bowed(0.5), off / 2., rng);
	let c2 = jitter(bowed(0.75), off / 2., rng);
	vec![PathOp::Move(start), PathOp::Curve { c1, c2, to: end }]
}

/// A line as the pen draws it: exact at roughness 0, two passes otherwise.
fn rough_line(a: Point, b: Point, roughness: f64, rng: &mut Rng, out: &mut Vec<Vec<PathOp>>) {
	if roughness <= 0. {
		out.push(vec![PathOp::Move(a), PathOp::Line(b)]);
		return;
	}
	out.push(rough_pass(a, b, roughness, rng));
	out.push(rough_pass(a, b, roughness, rng));
}

/// A smooth curve through `points` (Catmull-Rom), closed or open.
fn smooth(points: &[Point], closed: bool) -> Vec<PathOp> {
	let n = points.len();
	if n == 0 {
		return Vec::new();
	}
	let at = |i: isize| -> Point {
		if closed {
			points[i.rem_euclid(n as isize) as usize]
		} else {
			points[i.clamp(0, n as isize - 1) as usize]
		}
	};
	let mut ops = vec![PathOp::Move(points[0])];
	let segments = if closed { n } else { n - 1 };
	for i in 0..segments as isize {
		let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
		ops.push(PathOp::Curve {
			c1: [p1[0] + (p2[0] - p0[0]) / 6., p1[1] + (p2[1] - p0[1]) / 6.],
			c2: [p2[0] - (p3[0] - p1[0]) / 6., p2[1] - (p3[1] - p1[1]) / 6.],
			to: p2,
		});
	}
	ops
}

/// An exact ellipse as four cubic curves.
fn exact_ellipse(c: Point, a: f64, b: f64) -> Vec<PathOp> {
	const K: f64 = 0.552_284_749_830_793_6;
	let p = |x: f64, y: f64| [c[0] + a * x, c[1] + b * y];
	let mut ops = vec![PathOp::Move(p(1., 0.))];
	for (c1, c2, to) in [
		((1., K), (K, 1.), (0., 1.)),
		((-K, 1.), (-1., K), (-1., 0.)),
		((-1., -K), (-K, -1.), (0., -1.)),
		((K, -1.), (1., -K), (1., 0.)),
	] {
		ops.push(PathOp::Curve {
			c1: p(c1.0, c1.1),
			c2: p(c2.0, c2.1),
			to: p(to.0, to.1),
		});
	}
	ops
}

fn ellipse_points(c: Point, a: f64, b: f64, n: usize, start: f64) -> Vec<Point> {
	(0..n)
		.map(|i| {
			let t = start + 2. * PI * i as f64 / n as f64;
			[c[0] + a * t.cos(), c[1] + b * t.sin()]
		})
		.collect()
}

/// The two strokes of an arrow head at `tip`, for a shaft coming from `from`.
fn arrow_head(from: Point, tip: Point, stroke_width: f64, out: &mut Vec<Vec<PathOp>>) {
	let len = length(from, tip);
	if len < 1. {
		return;
	}
	let size = (16. + stroke_width * 4.).min(len / 2.);
	let back = [
		(from[0] - tip[0]) / len * size,
		(from[1] - tip[1]) / len * size,
	];
	for angle in [25f64.to_radians(), -25f64.to_radians()] {
		let side = rotate([tip[0] + back[0], tip[1] + back[1]], tip, angle);
		out.push(vec![PathOp::Move(side), PathOp::Line(tip)]);
	}
}

/// Hatch lines across the convex polygon `polygon`, `gap` apart, at `angle`.
fn hatch(polygon: &[Point], gap: f64, angle: f64, out: &mut Vec<(Point, Point)>) {
	let (sin, cos) = angle.sin_cos();
	let (dir, normal) = ([cos, sin], [-sin, cos]);
	let across = |p: &Point| p[0] * normal[0] + p[1] * normal[1];
	let along = |p: &Point| p[0] * dir[0] + p[1] * dir[1];
	let (min, max) = polygon
		.iter()
		.map(across)
		.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
			(lo.min(v), hi.max(v))
		});
	if !(min.is_finite() && max.is_finite()) || gap <= 0. {
		return;
	}
	let mut t = min + gap / 2.;
	while t < max {
		// Where the hatch line crosses the polygon's edges.
		let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
		for i in 0..polygon.len() {
			let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
			let (ta, tb) = (across(&a), across(&b));
			if (ta - t) * (tb - t) <= 0. && ta != tb {
				let s = along(&lerp(a, b, (t - ta) / (tb - ta)));
				lo = lo.min(s);
				hi = hi.max(s);
			}
		}
		if hi > lo {
			let at = |s: f64| [normal[0] * t + dir[0] * s, normal[1] * t + dir[1] * s];
			out.push((at(lo), at(hi)));
		}
		t += gap;
	}
}

/// The outline of a rectangle with rounded corners: its sides (to be drawn as pen lines)
/// and its corner curves.
fn rounded_rect(b: &Rect, roughness: f64, rng: &mut Rng, out: &mut Vec<Vec<PathOp>>) {
	let radius = (b.w.min(b.h) * 0.25).min(32.);
	let corners = [
		[b.x, b.y],
		[b.x + b.w, b.y],
		[b.x + b.w, b.y + b.h],
		[b.x, b.y + b.h],
	];
	for i in 0..4 {
		let (prev, corner, next) = (corners[(i + 3) % 4], corners[i], corners[(i + 1) % 4]);
		let toward = |to: Point| {
			let len = length(corner, to).max(1e-9);
			lerp(corner, to, radius / len)
		};
		let (enter, leave) = (toward(prev), toward(next));
		out.push(vec![
			PathOp::Move(enter),
			PathOp::Curve {
				c1: lerp(enter, corner, 0.552),
				c2: lerp(leave, corner, 0.552),
				to: leave,
			},
		]);
		let side_end = {
			let len = length(next, corner).max(1e-9);
			lerp(next, corner, radius / len)
		};
		rough_line(leave, side_end, roughness, rng, out);
	}
}

/// The strokes and fill of `el`, in scene coordinates.
pub fn rough_shape(el: &Element) -> Shape {
	let mut shape = Shape::default();
	if el.is_deleted {
		return shape;
	}
	let mut rng = Rng::new(el.seed as u64);
	let roughness = el.roughness.clamp(0., 4.);
	let b = geometry::bounds(el);
	// Everything is built at the origin, then moved: the wobble follows the element.
	let local = Rect {
		x: 0.,
		y: 0.,
		w: b.w,
		h: b.h,
	};
	let c = local.center();
	let mut outline: Option<Vec<Point>> = None;
	let strokes = &mut shape.strokes;
	match el.kind.as_str() {
		"rectangle" => {
			let corners = [[0., 0.], [b.w, 0.], [b.w, b.h], [0., b.h]];
			if el.rounded() && b.w.min(b.h) > 4. {
				rounded_rect(&local, roughness, &mut rng, strokes);
			} else if roughness <= 0. {
				let mut path = vec![PathOp::Move(corners[0])];
				path.extend((1..=4).map(|i| PathOp::Line(corners[i % 4])));
				strokes.push(path);
			} else {
				for i in 0..4 {
					rough_line(
						corners[i],
						corners[(i + 1) % 4],
						roughness,
						&mut rng,
						strokes,
					);
				}
			}
			outline = Some(corners.to_vec());
		}
		"diamond" => {
			let corners = [[c[0], 0.], [b.w, c[1]], [c[0], b.h], [0., c[1]]];
			if roughness <= 0. {
				let mut path = vec![PathOp::Move(corners[0])];
				path.extend((1..=4).map(|i| PathOp::Line(corners[i % 4])));
				strokes.push(path);
			} else {
				for i in 0..4 {
					rough_line(
						corners[i],
						corners[(i + 1) % 4],
						roughness,
						&mut rng,
						strokes,
					);
				}
			}
			outline = Some(corners.to_vec());
		}
		"ellipse" => {
			let (a, r) = (b.w / 2., b.h / 2.);
			if roughness <= 0. {
				strokes.push(exact_ellipse(c, a, r));
			} else {
				let off = wobble(roughness, a.min(r) * 2.) * 0.6;
				for _ in 0..2 {
					let start = rng.next_f64() * 2. * PI;
					let points: Vec<Point> = ellipse_points(c, a, r, 20, start)
						.into_iter()
						.map(|p| [p[0] + rng.offset(off), p[1] + rng.offset(off)])
						.collect();
					strokes.push(smooth(&points, true));
				}
			}
			outline = Some(ellipse_points(c, a, r, 32, 0.));
		}
		"line" | "arrow" => {
			// Local points: relative to the top left of the bounds.
			let points: Vec<Point> = geometry::abs_points(el)
				.iter()
				.map(|p| [p[0] - b.x, p[1] - b.y])
				.collect();
			match points.as_slice() {
				[] | [_] => {}
				[a, z] => rough_line(*a, *z, roughness, &mut rng, strokes),
				_ if el.rounded() => {
					let passes = if roughness <= 0. { 1 } else { 2 };
					for _ in 0..passes {
						let off = roughness * 0.8;
						let jittered: Vec<Point> = points
							.iter()
							.map(|p| [p[0] + rng.offset(off), p[1] + rng.offset(off)])
							.collect();
						strokes.push(smooth(&jittered, false));
					}
				}
				_ if roughness <= 0. => {
					let mut path = vec![PathOp::Move(points[0])];
					path.extend(points[1..].iter().map(|p| PathOp::Line(*p)));
					strokes.push(path);
				}
				_ => {
					for w in points.windows(2) {
						rough_line(w[0], w[1], roughness, &mut rng, strokes);
					}
				}
			}
			if el.kind == "arrow" && points.len() >= 2 {
				let n = points.len();
				if el.end_arrowhead.is_some() {
					arrow_head(points[n - 2], points[n - 1], el.stroke_width, strokes);
				}
				if el.start_arrowhead.is_some() {
					arrow_head(points[1], points[0], el.stroke_width, strokes);
				}
			}
		}
		"freedraw" => {
			let points: Vec<Point> = geometry::abs_points(el)
				.iter()
				.map(|p| [p[0] - b.x, p[1] - b.y])
				.collect();
			match points.as_slice() {
				[] => {}
				[only] => strokes.push(vec![PathOp::Move(*only), PathOp::Line(*only)]),
				_ => strokes.push(smooth(&points, false)),
			}
		}
		_ => {}
	}
	let filled =
		!el.background_color.eq_ignore_ascii_case("transparent") && !el.background_color.is_empty();
	if let Some(outline) = outline.filter(|_| filled) {
		match el.fill_style.as_str() {
			"solid" => shape.fill = Some(outline),
			style => {
				let gap = (el.stroke_width * 4.).max(4.);
				let angle = -41f64.to_radians();
				hatch(&outline, gap, angle, &mut shape.hachure);
				if style == "cross-hatch" {
					hatch(&outline, gap, angle + PI / 2., &mut shape.hachure);
				}
			}
		}
	}
	// To the scene: move to the element's place, then turn around its center.
	let center = b.center();
	let place = |p: Point| rotate([p[0] + b.x, p[1] + b.y], center, el.angle);
	for stroke in &mut shape.strokes {
		for op in stroke.iter_mut() {
			*op = match *op {
				PathOp::Move(p) => PathOp::Move(place(p)),
				PathOp::Line(p) => PathOp::Line(place(p)),
				PathOp::Curve { c1, c2, to } => PathOp::Curve {
					c1: place(c1),
					c2: place(c2),
					to: place(to),
				},
			};
		}
	}
	if let Some(fill) = &mut shape.fill {
		for p in fill.iter_mut() {
			*p = place(*p);
		}
	}
	for (a, z) in &mut shape.hachure {
		(*a, *z) = (place(*a), place(*z));
	}
	shape
}

/// The path as a polyline; curves are cut into segments about `tolerance * 8` long.
pub fn flatten(path: &[PathOp], tolerance: f64) -> Vec<Point> {
	let mut out: Vec<Point> = Vec::new();
	for op in path {
		match *op {
			PathOp::Move(p) | PathOp::Line(p) => out.push(p),
			PathOp::Curve { c1, c2, to } => {
				let from = out.last().copied().unwrap_or(to);
				let rough_len = length(from, c1) + length(c1, c2) + length(c2, to);
				let steps = (rough_len / (tolerance.max(0.01) * 8.))
					.ceil()
					.clamp(4., 64.) as usize;
				for i in 1..=steps {
					let t = i as f64 / steps as f64;
					let (ab, bc, cd) = (lerp(from, c1, t), lerp(c1, c2, t), lerp(c2, to, t));
					out.push(lerp(lerp(ab, bc, t), lerp(bc, cd, t), t));
				}
			}
		}
	}
	out
}

/// Cuts a polyline into dashes `on` long, `off` apart.
pub fn dash(points: &[Point], on: f64, off: f64) -> Vec<Vec<Point>> {
	if points.len() < 2 {
		return Vec::new();
	}
	if on <= 0. || off <= 0. {
		return vec![points.to_vec()];
	}
	let mut out = Vec::new();
	let mut current = vec![points[0]];
	let mut drawing = true;
	let mut remaining = on;
	for w in points.windows(2) {
		let (a, b) = (w[0], w[1]);
		let len = length(a, b);
		if len == 0. {
			continue;
		}
		let mut pos = 0.;
		while pos < len {
			let step = remaining.min(len - pos);
			pos += step;
			remaining -= step;
			let p = if pos >= len { b } else { lerp(a, b, pos / len) };
			if drawing {
				current.push(p);
			}
			if remaining <= 1e-9 {
				if drawing {
					out.push(std::mem::take(&mut current));
					remaining = off;
				} else {
					current = vec![p];
					remaining = on;
				}
				drawing = !drawing;
			}
		}
	}
	if drawing && current.len() >= 2 {
		out.push(current);
	}
	out
}

#[cfg(test)]
mod tests {
	use std::f64::consts::FRAC_PI_2;

	use super::*;
	use crate::drawing::geometry::{self, Point, rotate};
	use crate::drawing::model::Element;
	use crate::drawing::rng::Rng;

	fn el(kind: &str, w: f64, h: f64, roughness: f64) -> Element {
		let mut e = Element::new(kind, 10., 20., &mut Rng::new(3));
		e.width = w;
		e.height = h;
		e.roughness = roughness;
		e
	}

	fn linear(kind: &str, points: &[[f64; 2]], roughness: f64) -> Element {
		let mut e = el(kind, 0., 0., roughness);
		(e.x, e.y) = (0., 0.);
		e.points = Some(points.to_vec());
		geometry::fit_linear(&mut e);
		e
	}

	fn all_points(shape: &Shape) -> Vec<Point> {
		shape
			.strokes
			.iter()
			.flat_map(|s| flatten(s, 0.25))
			.collect()
	}

	fn has(points: &[Point], p: Point) -> bool {
		points
			.iter()
			.any(|q| (q[0] - p[0]).abs() < 1e-6 && (q[1] - p[1]).abs() < 1e-6)
	}

	#[test]
	fn same_seed_same_shape() {
		let a = el("rectangle", 100., 60., 1.);
		assert_eq!(rough_shape(&a), rough_shape(&a));
		let mut b = a.clone();
		b.seed += 1;
		assert_ne!(rough_shape(&a), rough_shape(&b));
		// Moving an element moves its wobble with it: no shimmer while dragging.
		let mut moved = a.clone();
		moved.x += 50.;
		let (p, q) = (
			all_points(&rough_shape(&a)),
			all_points(&rough_shape(&moved)),
		);
		assert_eq!(p.len(), q.len());
		assert!(
			p.iter()
				.zip(&q)
				.all(|(p, q)| (q[0] - p[0] - 50.).abs() < 1e-6 && (q[1] - p[1]).abs() < 1e-6)
		);
	}

	#[test]
	fn roughness_zero_is_exact() {
		let r = el("rectangle", 100., 60., 0.);
		let points = all_points(&rough_shape(&r));
		for corner in [[10., 20.], [110., 20.], [110., 80.], [10., 80.]] {
			assert!(has(&points, corner), "{corner:?}");
		}
		let mut turned = r.clone();
		turned.angle = FRAC_PI_2;
		let points = all_points(&rough_shape(&turned));
		assert!(has(&points, rotate([10., 20.], [60., 50.], FRAC_PI_2)));
		let e = el("ellipse", 100., 60., 0.);
		for p in all_points(&rough_shape(&e)) {
			let k = ((p[0] - 60.) / 50.).powi(2) + ((p[1] - 50.) / 30.).powi(2);
			assert!((k - 1.).abs() < 0.02, "{p:?} {k}");
		}
		let d = el("diamond", 100., 60., 0.);
		let points = all_points(&rough_shape(&d));
		assert!(has(&points, [60., 20.]) && has(&points, [110., 50.]));
	}

	#[test]
	fn rough_strokes_stay_near_the_shape() {
		for kind in ["rectangle", "diamond", "ellipse"] {
			let mut e = el(kind, 160., 90., 2.);
			let shape = rough_shape(&e);
			assert!(shape.strokes.len() >= 2, "{kind}: drawn twice like a pen");
			e.background_color = "transparent".into();
			for p in all_points(&shape) {
				assert!(
					geometry::hit_test(&e, p, 12.),
					"{kind}: {p:?} is far from the outline"
				);
			}
		}
		// Tiny shapes do not explode.
		let tiny = el("rectangle", 3., 3., 2.);
		for p in all_points(&rough_shape(&tiny)) {
			assert!(
				(p[0] - 11.5).abs() < 8. && (p[1] - 21.5).abs() < 8.,
				"{p:?}"
			);
		}
	}

	#[test]
	fn lines_arrows_and_freedraw() {
		let line = linear("line", &[[0., 0.], [100., 0.]], 0.);
		let arrow = linear("arrow", &[[0., 0.], [100., 0.]], 0.);
		let (l, a) = (rough_shape(&line), rough_shape(&arrow));
		assert_eq!(l.strokes.len(), 1);
		assert_eq!(a.strokes.len(), 3, "a shaft and two head strokes");
		let head: Vec<Point> = a.strokes[1..]
			.iter()
			.flat_map(|s| flatten(s, 0.25))
			.collect();
		assert!(has(&head, [100., 0.]), "the head meets the tip");
		assert!(head.iter().all(|p| p[0] <= 100. + 1e-6 && p[0] > 60.));
		// An arrow with no end head is a plain line.
		let mut bare = arrow.clone();
		bare.end_arrowhead = None;
		assert_eq!(rough_shape(&bare).strokes.len(), 1);
		// Both ends.
		let mut both = arrow.clone();
		both.start_arrowhead = Some("arrow".into());
		assert_eq!(rough_shape(&both).strokes.len(), 5);
		// A multi-point line passes through its points.
		let bent = linear("line", &[[0., 0.], [50., 40.], [100., 0.]], 0.);
		let points = all_points(&rough_shape(&bent));
		assert!(has(&points, [0., 0.]) && has(&points, [50., 40.]) && has(&points, [100., 0.]));
		let free = linear("freedraw", &[[0., 0.], [5., 5.], [10., 0.], [15., 5.]], 1.);
		let shape = rough_shape(&free);
		assert_eq!(
			shape.strokes.len(),
			1,
			"freedraw is drawn once, as the hand moved"
		);
		let points = all_points(&shape);
		assert!(has(&points, [0., 0.]) && has(&points, [15., 5.]));
		// Degenerate input draws nothing and does not panic.
		assert!(rough_shape(&linear("line", &[], 1.)).strokes.is_empty());
		assert!(
			rough_shape(&linear("arrow", &[[0., 0.], [0., 0.]], 1.))
				.strokes
				.len() <= 2
		);
		assert!(rough_shape(&el("text", 50., 20., 1.)).strokes.is_empty());
		assert!(rough_shape(&el("frame", 50., 20., 1.)).strokes.is_empty());
	}

	#[test]
	fn fills() {
		let mut r = el("rectangle", 100., 60., 1.);
		assert!(rough_shape(&r).fill.is_none());
		r.background_color = "#ffc9c9".into();
		let shape = rough_shape(&r);
		assert_eq!(shape.fill.as_ref().map(Vec::len), Some(4));
		assert!(shape.hachure.is_empty());
		r.fill_style = "hachure".into();
		let shape = rough_shape(&r);
		assert!(shape.fill.is_none());
		assert!(shape.hachure.len() > 5);
		for (a, b) in &shape.hachure {
			for p in [a, b] {
				assert!(
					p[0] >= 9.999 && p[0] <= 110.001 && p[1] >= 19.999 && p[1] <= 80.001,
					"{p:?}"
				);
			}
		}
		let single = shape.hachure.len();
		r.fill_style = "cross-hatch".into();
		assert!(rough_shape(&r).hachure.len() > single);
		let mut e = el("ellipse", 100., 60., 1.);
		e.background_color = "#ffc9c9".into();
		assert!(rough_shape(&e).fill.is_some_and(|f| f.len() >= 16));
	}

	#[test]
	fn flatten_and_dash() {
		let path = [PathOp::Move([0., 0.]), PathOp::Line([100., 0.])];
		assert_eq!(flatten(&path, 0.5), [[0., 0.], [100., 0.]]);
		let curve = [
			PathOp::Move([0., 0.]),
			PathOp::Curve {
				c1: [0., 50.],
				c2: [100., 50.],
				to: [100., 0.],
			},
		];
		let points = flatten(&curve, 0.5);
		assert!(points.len() > 4);
		assert_eq!((points[0], *points.last().unwrap()), ([0., 0.], [100., 0.]));
		let dashes = dash(&[[0., 0.], [100., 0.]], 10., 5.);
		assert_eq!(dashes.len(), 7);
		assert_eq!(dashes[0], [[0., 0.], [10., 0.]]);
		assert_eq!(dashes[1][0], [15., 0.]);
		assert_eq!(dashes[6], [[90., 0.], [100., 0.]]);
		// A dash continues around a corner.
		let corner = dash(&[[0., 0.], [5., 0.], [5., 20.]], 10., 5.);
		assert_eq!(corner[0], [[0., 0.], [5., 0.], [5., 5.]]);
		assert!(dash(&[[0., 0.]], 10., 5.).is_empty());
		assert!(dash(&[[0., 0.], [10., 0.]], 0., 0.).len() <= 1);
	}

	/// Cost of the hand-drawn geometry of a large scene, computed once per element version
	/// (run with `cargo test --release -p ruby-core perf_ -- --ignored --nocapture`).
	#[test]
	#[ignore]
	fn perf_shapes_of_2000_elements() {
		let mut rng = Rng::new(7);
		let kinds = ["rectangle", "ellipse", "diamond", "arrow"];
		let elements: Vec<Element> = (0..2000)
			.map(|i| {
				let mut el = Element::new(
					kinds[i % 4],
					(i % 50) as f64 * 120.,
					(i / 50) as f64 * 90.,
					&mut rng,
				);
				el.width = 100.;
				el.height = 60.;
				if el.kind == "arrow" {
					el.points = Some(vec![[0., 0.], [100., 60.]]);
				}
				el
			})
			.collect();
		let start = std::time::Instant::now();
		for el in &elements {
			std::hint::black_box(rough_shape(el));
		}
		let all = start.elapsed();
		println!("shapes of 2000 elements: {all:?} ({:?} each)", all / 2000);
		if !cfg!(debug_assertions) {
			assert!(all < std::time::Duration::from_millis(100), "{all:?}");
		}
	}
}
