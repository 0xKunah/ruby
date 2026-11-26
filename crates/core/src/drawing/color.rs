//! Drawing colors. Files store Excalidraw's light-theme colors; the canvas shows them
//! through Excalidraw's dark-mode filter, so a file looks the same here and there.

/// Stroke swatches (stored values). Shown: light gray, red, green, blue, amber.
pub const STROKES: [&str; 5] = ["#1e1e1e", "#ea4d4d", "#00752a", "#1868cb", "#c65400"];

/// Background swatches (stored values): none, then red, green, blue and amber tints.
pub const BACKGROUNDS: [&str; 5] = ["transparent", "#fcd2d2", "#96caab", "#b1c9e8", "#c8b87a"];

/// `#rgb`, `#rrggbb`, `#rrggbbaa` or `transparent`, as RGBA.
pub fn parse_color(color: &str) -> Option<[u8; 4]> {
	let color = color.trim();
	if color.eq_ignore_ascii_case("transparent") {
		return Some([0, 0, 0, 0]);
	}
	let hex = color.strip_prefix('#')?;
	if !hex.is_ascii() {
		return None;
	}
	let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
	match hex.len() {
		3 => {
			let nibble = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|n| n * 17);
			Some([nibble(0)?, nibble(1)?, nibble(2)?, 255])
		}
		6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
		8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
		_ => None,
	}
}

/// Excalidraw's dark-mode filter: CSS `invert(93%) hue-rotate(180deg)`.
pub fn dark_mode(rgb: [u8; 3]) -> [u8; 3] {
	// hue-rotate(180deg) as a matrix on RGB.
	const HUE: [[f64; 3]; 3] = [
		[-0.574, 1.430, 0.144],
		[0.426, 0.430, 0.144],
		[0.426, 1.430, -0.856],
	];
	let inverted = rgb.map(|c| 0.93 * (255. - c as f64) + 0.07 * c as f64);
	HUE.map(|row| {
		let v: f64 = (0..3).map(|i| row[i] * inverted[i]).sum();
		v.round().clamp(0., 255.) as u8
	})
}

#[cfg(test)]
mod tests {
	use super::{BACKGROUNDS, STROKES, dark_mode, parse_color};

	fn distance(a: [u8; 3], b: [u8; 3]) -> i32 {
		(0..3)
			.map(|i| (a[i] as i32 - b[i] as i32).abs())
			.max()
			.unwrap()
	}

	#[test]
	fn parses_hex_forms() {
		assert_eq!(parse_color("#1e1e1e"), Some([0x1e, 0x1e, 0x1e, 255]));
		assert_eq!(parse_color("#FFF"), Some([255, 255, 255, 255]));
		assert_eq!(parse_color("#ff000080"), Some([255, 0, 0, 0x80]));
		assert_eq!(parse_color("transparent"), Some([0, 0, 0, 0]));
		assert_eq!(parse_color(" #000000 "), Some([0, 0, 0, 255]));
		assert_eq!(parse_color("#12"), None);
		assert_eq!(parse_color("#gggggg"), None);
		assert_eq!(parse_color("\u{e9}\u{e9}\u{e9}\u{e9}"), None);
		assert_eq!(parse_color(""), None);
	}

	#[test]
	fn dark_mode_inverts_lightness_and_keeps_hue() {
		// invert(93%) of black is #ededed; hue rotation leaves grays alone.
		assert!(distance(dark_mode([0, 0, 0]), [0xed, 0xed, 0xed]) <= 1);
		assert!(distance(dark_mode([255, 255, 255]), [0x12, 0x12, 0x12]) <= 1);
		// A red stays reddish, a blue stays bluish.
		let red = dark_mode([0xe0, 0x31, 0x31]);
		assert!(red[0] > red[1] && red[0] > red[2], "{red:?}");
		let blue = dark_mode([0x19, 0x71, 0xc2]);
		assert!(blue[2] > blue[0] && blue[2] > blue[1], "{blue:?}");
	}

	#[test]
	fn swatches_show_as_the_mockup_colors() {
		let shown = [
			[0xe5, 0xe5, 0xe5],
			[0xf8, 0x71, 0x71],
			[0x4a, 0xde, 0x80],
			[0x60, 0xa5, 0xfa],
			[0xfa, 0xcc, 0x15],
		];
		// The dark transform cannot produce a bright green or yellow: those two are the
		// closest colors it can reach.
		let tolerance = [24, 4, 36, 4, 72];
		for ((stored, want), tolerance) in STROKES.iter().zip(shown).zip(tolerance) {
			let [r, g, b, _] = parse_color(stored).unwrap();
			let got = dark_mode([r, g, b]);
			assert!(
				distance(got, want) <= tolerance,
				"{stored}: {got:?} vs {want:?}"
			);
		}
		assert_eq!(BACKGROUNDS[0], "transparent");
		assert!(BACKGROUNDS[1..].iter().all(|c| parse_color(c).is_some()));
	}
}
