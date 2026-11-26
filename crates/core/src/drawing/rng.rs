//! A small deterministic random generator (xorshift64*): element ids and seeds, and the
//! hand-drawn wobble of shapes, which must be the same on every frame.

pub struct Rng(u64);

impl Rng {
	pub fn new(seed: u64) -> Self {
		// Spread the seed bits (a seed of 0 would stay 0).
		Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
	}

	pub fn next_u64(&mut self) -> u64 {
		let mut x = self.0;
		x ^= x >> 12;
		x ^= x << 25;
		x ^= x >> 27;
		self.0 = x;
		x.wrapping_mul(0x2545_F491_4F6C_DD1D)
	}

	/// In `[0, 1)`.
	pub fn next_f64(&mut self) -> f64 {
		(self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
	}

	/// In `[-amount, amount]`.
	pub fn offset(&mut self, amount: f64) -> f64 {
		(self.next_f64() * 2. - 1.) * amount
	}

	/// A positive 31-bit integer (Excalidraw seeds and nonces).
	pub fn next_seed(&mut self) -> i64 {
		((self.next_u64() >> 33) as i64).max(1)
	}

	/// A 20-character id like Excalidraw's.
	pub fn next_id(&mut self) -> String {
		const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";
		(0..20)
			.map(|_| CHARS[(self.next_u64() >> 58) as usize] as char)
			.collect()
	}
}

#[cfg(test)]
mod tests {
	use super::Rng;

	#[test]
	fn deterministic_and_in_range() {
		let (mut a, mut b, mut c) = (Rng::new(5), Rng::new(5), Rng::new(6));
		let xs: Vec<u64> = (0..4).map(|_| a.next_u64()).collect();
		assert_eq!(xs, (0..4).map(|_| b.next_u64()).collect::<Vec<_>>());
		assert_ne!(xs[0], c.next_u64());
		let mut zero = Rng::new(0);
		assert_ne!(zero.next_u64(), zero.next_u64());
		for _ in 0..200 {
			let f = a.next_f64();
			assert!((0. ..1.).contains(&f));
			assert!(a.offset(2.).abs() <= 2.);
			assert!(a.next_seed() >= 1);
		}
		assert_eq!(a.next_id().len(), 20);
	}
}
