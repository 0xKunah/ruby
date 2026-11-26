//! Decoding and encoding of note files: UTF-8 with optional BOM, LF or CRLF.

const BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
	Lf,
	Crlf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFormat {
	pub line_ending: LineEnding,
	pub bom: bool,
}

#[derive(Debug, PartialEq)]
pub enum DecodeError {
	NotUtf8,
}

/// Decodes file bytes into editor text (always `\n`) plus the format needed to write it back.
/// The line ending is the first one found in the file, `Lf` when there is none.
pub fn decode(bytes: &[u8]) -> Result<(String, FileFormat), DecodeError> {
	let (bom, body) = match bytes.strip_prefix(BOM) {
		Some(rest) => (true, rest),
		None => (false, bytes),
	};
	let text = std::str::from_utf8(body).map_err(|_| DecodeError::NotUtf8)?;
	let line_ending = match text.find('\n') {
		Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => LineEnding::Crlf,
		_ => LineEnding::Lf,
	};
	Ok((text.replace("\r\n", "\n"), FileFormat { line_ending, bom }))
}

/// Encodes editor text back to file bytes in the given format.
pub fn encode(text: &str, format: FileFormat) -> Vec<u8> {
	let mut out = Vec::with_capacity(text.len() + 3);
	if format.bom {
		out.extend_from_slice(BOM);
	}
	match format.line_ending {
		LineEnding::Lf => out.extend_from_slice(text.as_bytes()),
		LineEnding::Crlf => out.extend_from_slice(text.replace('\n', "\r\n").as_bytes()),
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn decode_lf() {
		let (text, f) = decode(b"a\nb").unwrap();
		assert_eq!(text, "a\nb");
		assert_eq!(
			f,
			FileFormat {
				line_ending: LineEnding::Lf,
				bom: false
			}
		);
	}

	#[test]
	fn decode_crlf_normalizes() {
		let (text, f) = decode(b"a\r\nb\r\n").unwrap();
		assert_eq!(text, "a\nb\n");
		assert_eq!(f.line_ending, LineEnding::Crlf);
	}

	#[test]
	fn decode_strips_bom() {
		let (text, f) = decode(b"\xEF\xBB\xBFhi").unwrap();
		assert_eq!(text, "hi");
		assert!(f.bom);
	}

	#[test]
	fn roundtrip_crlf_bom_accents() {
		let bytes = "\u{FEFF}# Titre\r\n\r\n- été\r\n".as_bytes().to_vec();
		let (text, f) = decode(&bytes).unwrap();
		assert_eq!(encode(&text, f), bytes);
	}

	#[test]
	fn mixed_endings_use_first() {
		let (text, f) = decode(b"a\r\nb\nc").unwrap();
		assert_eq!(text, "a\nb\nc");
		assert_eq!(encode(&text, f), b"a\r\nb\r\nc".to_vec());
	}

	#[test]
	fn rejects_non_utf8() {
		assert_eq!(decode(b"caf\xE9"), Err(DecodeError::NotUtf8));
	}
}
