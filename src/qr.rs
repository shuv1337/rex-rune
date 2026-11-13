//! QR Code Model 2, byte mode, level M, versions 1–10 (ISO/IEC 18004).
//! No ECI, structured append, drawing or quiet zone; callers add four light modules.
use std::fmt;

/// A square symbol, in row-major order. Coordinates exclude the quiet zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    size: usize,
    modules: Vec<bool>,
}
impl Symbol {
    pub fn size(&self) -> usize {
        self.size
    }
    pub fn dark(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLong;
impl fmt::Display for TooLong {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("QR byte input exceeds version 10-M capacity (213 bytes)")
    }
}
impl std::error::Error for TooLong {}

// Total codewords, ECC codewords per block, number of blocks. Short blocks precede
// long blocks; long blocks have exactly one extra data codeword (Table 9).
const BLOCKS: [(usize, usize, usize); 10] = [
    (26, 10, 1),
    (44, 16, 1),
    (70, 26, 1),
    (100, 18, 2),
    (134, 24, 2),
    (172, 16, 4),
    (196, 18, 4),
    (242, 22, 4),
    (292, 22, 5),
    (346, 26, 5),
];

/// Encode the smallest version that fits. Empty byte strings are supported.
pub fn encode(bytes: &[u8]) -> Result<Symbol, TooLong> {
    encode_mask(bytes, None).map(|(s, _)| s)
}

fn encode_mask(bytes: &[u8], forced: Option<u8>) -> Result<(Symbol, u8), TooLong> {
    if bytes.len() > 213 {
        return Err(TooLong);
    }
    let version = (1..=10)
        .find(|&v| {
            let (total, ecc, blocks) = BLOCKS[v - 1];
            4 + if v < 10 { 8 } else { 16 } + bytes.len() * 8 <= (total - ecc * blocks) * 8
        })
        .ok_or(TooLong)?;
    let (total, ecc, count) = BLOCKS[version - 1];
    let capacity = (total - ecc * count) * 8;
    let mut bits = Vec::with_capacity(capacity);
    push(&mut bits, 4, 4);
    push(
        &mut bits,
        bytes.len() as u32,
        if version < 10 { 8 } else { 16 },
    );
    for &b in bytes {
        push(&mut bits, b.into(), 8);
    }
    let terminator = 4.min(capacity - bits.len());
    push(&mut bits, 0, terminator);
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    let mut data: Vec<u8> = bits
        .chunks(8)
        .map(|chunk| chunk.iter().fold(0, |v, &b| (v << 1) | u8::from(b)))
        .collect();
    let mut pad = 0xec;
    while data.len() < capacity / 8 {
        data.push(pad);
        pad ^= 0xec ^ 0x11;
    }
    let short = total / count - ecc;
    let short_count = count - total % count;
    let divisor = generator(ecc);
    let mut blocks = Vec::new();
    let mut offset = 0;
    for i in 0..count {
        let len = short + usize::from(i >= short_count);
        let block = data[offset..offset + len].to_vec();
        let parity = remainder(&block, &divisor);
        blocks.push((block, parity));
        offset += len;
    }
    let mut codewords = Vec::with_capacity(total);
    for i in 0..=short {
        for (block, _) in &blocks {
            if let Some(&b) = block.get(i) {
                codewords.push(b);
            }
        }
    }
    for i in 0..ecc {
        for (_, parity) in &blocks {
            codewords.push(parity[i]);
        }
    }
    let mut base = Builder::new(version);
    base.patterns(version);
    base.data(&codewords);
    let masks: Vec<u8> = forced.map_or_else(|| (0..8).collect(), |m| vec![m]);
    let mut best = None;
    for mask in masks {
        let mut candidate = base.clone();
        for y in 0..candidate.symbol.size {
            for x in 0..candidate.symbol.size {
                let i = y * candidate.symbol.size + x;
                if !candidate.function[i] && masked(mask, x, y) {
                    candidate.symbol.modules[i] ^= true;
                }
            }
        }
        candidate.format(mask);
        let penalty = penalty(&candidate.symbol);
        if best.as_ref().is_none_or(|(p, _, _)| penalty < *p) {
            best = Some((penalty, candidate.symbol, mask));
        }
    }
    let (_, symbol, mask) = best.expect("eight masks or one forced mask");
    Ok((symbol, mask))
}
fn push(bits: &mut Vec<bool>, value: u32, count: usize) {
    for i in (0..count).rev() {
        bits.push(value >> i & 1 != 0);
    }
}

// GF(256), primitive polynomial x^8 + x^4 + x^3 + x^2 + 1 (0x11d).
fn multiply(mut a: u8, mut b: u8) -> u8 {
    let mut result = 0;
    for _ in 0..8 {
        if b & 1 != 0 {
            result ^= a;
        }
        b >>= 1;
        let high = a & 0x80 != 0;
        a <<= 1;
        if high {
            a ^= 0x1d;
        }
    }
    result
}
// Descending coefficients, including the leading 1, of ∏(x − α^i).
fn generator(degree: usize) -> Vec<u8> {
    let mut poly = vec![1];
    let mut root = 1;
    for _ in 0..degree {
        let mut next = vec![0; poly.len() + 1];
        for (i, &c) in poly.iter().enumerate() {
            next[i] ^= c;
            next[i + 1] ^= multiply(c, root);
        }
        poly = next;
        root = multiply(root, 2);
    }
    poly
}
fn remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut rem = vec![0; divisor.len() - 1];
    for &byte in data {
        let lead = byte ^ rem[0];
        rem.rotate_left(1);
        *rem.last_mut().unwrap() = 0;
        for (r, &c) in rem.iter_mut().zip(&divisor[1..]) {
            *r ^= multiply(lead, c);
        }
    }
    rem
}
fn format_bits(mask: u8) -> u32 {
    // Level M has the two-bit indicator 00.
    let data = u32::from(mask);
    let mut rem = data << 10;
    for i in (10..=14).rev() {
        if rem >> i & 1 != 0 {
            rem ^= 0x537 << (i - 10);
        }
    }
    ((data << 10) | rem) ^ 0x5412
}
fn version_bits(version: usize) -> u32 {
    let mut rem = (version as u32) << 12;
    for i in (12..=17).rev() {
        if rem >> i & 1 != 0 {
            rem ^= 0x1f25 << (i - 12);
        }
    }
    ((version as u32) << 12) | rem
}

#[derive(Clone)]
struct Builder {
    symbol: Symbol,
    function: Vec<bool>,
}
impl Builder {
    fn new(version: usize) -> Self {
        let size = version * 4 + 17;
        Self {
            symbol: Symbol {
                size,
                modules: vec![false; size * size],
            },
            function: vec![false; size * size],
        }
    }
    fn set(&mut self, x: usize, y: usize, dark: bool) {
        let i = y * self.symbol.size + x;
        self.symbol.modules[i] = dark;
        self.function[i] = true;
    }
    fn patterns(&mut self, version: usize) {
        let n = self.symbol.size;
        for i in 0..n {
            self.set(6, i, i % 2 == 0);
            self.set(i, 6, i % 2 == 0);
        }
        for (cx, cy) in [(3, 3), (n - 4, 3), (3, n - 4)] {
            for dy in -4isize..=4 {
                for dx in -4isize..=4 {
                    let x = cx as isize + dx;
                    let y = cy as isize + dy;
                    if x >= 0 && y >= 0 && x < n as isize && y < n as isize {
                        let distance = dx.abs().max(dy.abs());
                        self.set(x as usize, y as usize, distance != 2 && distance != 4);
                    }
                }
            }
        }
        let centers: &[usize] = match version {
            1 => &[],
            2 => &[6, 18],
            3 => &[6, 22],
            4 => &[6, 26],
            5 => &[6, 30],
            6 => &[6, 34],
            7 => &[6, 22, 38],
            8 => &[6, 24, 42],
            9 => &[6, 26, 46],
            10 => &[6, 28, 50],
            _ => unreachable!(),
        };
        for (iy, &y) in centers.iter().enumerate() {
            for (ix, &x) in centers.iter().enumerate() {
                let last = centers.len() - 1;
                if (iy == 0 && (ix == 0 || ix == last)) || (iy == last && ix == 0) {
                    continue;
                }
                for dy in -2isize..=2 {
                    for dx in -2isize..=2 {
                        self.set(
                            (x as isize + dx) as usize,
                            (y as isize + dy) as usize,
                            dx.abs().max(dy.abs()) != 1,
                        );
                    }
                }
            }
        }
        self.format(0);
        if version >= 7 {
            let bits = version_bits(version);
            for i in 0..18 {
                let a = n - 11 + i % 3;
                let b = i / 3;
                self.set(a, b, bits >> i & 1 != 0);
                self.set(b, a, bits >> i & 1 != 0);
            }
        }
    }
    fn format(&mut self, mask: u8) {
        let bits = format_bits(mask);
        let n = self.symbol.size;
        for i in 0..=5 {
            self.set(8, i, bits >> i & 1 != 0);
        }
        self.set(8, 7, bits >> 6 & 1 != 0);
        self.set(8, 8, bits >> 7 & 1 != 0);
        self.set(7, 8, bits >> 8 & 1 != 0);
        for i in 9..15 {
            self.set(14 - i, 8, bits >> i & 1 != 0);
        }
        for i in 0..8 {
            self.set(n - 1 - i, 8, bits >> i & 1 != 0);
        }
        for i in 8..15 {
            self.set(8, n - 15 + i, bits >> i & 1 != 0);
        }
        self.set(8, n - 8, true);
    }
    fn data(&mut self, codewords: &[u8]) {
        let n = self.symbol.size;
        let mut bit = 0;
        let mut right = n - 1;
        let mut up = true;
        while right > 0 {
            if right == 6 {
                right -= 1;
            }
            for row in 0..n {
                let y = if up { n - 1 - row } else { row };
                for x in [right, right - 1] {
                    let i = y * n + x;
                    if !self.function[i] {
                        self.symbol.modules[i] = bit < codewords.len() * 8
                            && (codewords[bit / 8] >> (7 - bit % 8)) & 1 != 0;
                        bit += 1;
                    }
                }
            }
            up = !up;
            right = right.saturating_sub(2);
        }
        debug_assert!(bit >= codewords.len() * 8 && bit < codewords.len() * 8 + 8);
    }
}
fn masked(mask: u8, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y).is_multiple_of(2),
        1 => y.is_multiple_of(2),
        2 => x.is_multiple_of(3),
        3 => (x + y).is_multiple_of(3),
        4 => (x / 3 + y / 2).is_multiple_of(2),
        5 => x * y % 2 + x * y % 3 == 0,
        6 => (x * y % 2 + x * y % 3).is_multiple_of(2),
        7 => ((x + y) % 2 + x * y % 3).is_multiple_of(2),
        _ => unreachable!(),
    }
}
fn penalty(s: &Symbol) -> usize {
    let n = s.size;
    let mut score = 0;
    for vertical in [false, true] {
        for a in 0..n {
            let line: Vec<bool> = (0..n)
                .map(|b| if vertical { s.dark(a, b) } else { s.dark(b, a) })
                .collect();
            let mut run = 1;
            for i in 1..n {
                if line[i] == line[i - 1] {
                    run += 1;
                    if run == 5 {
                        score += 3;
                    } else if run > 5 {
                        score += 1;
                    }
                } else {
                    run = 1;
                }
            }
            score += finder_penalty(&line);
        }
    }
    for y in 0..n - 1 {
        for x in 0..n - 1 {
            let c = s.dark(x, y);
            if c == s.dark(x + 1, y) && c == s.dark(x, y + 1) && c == s.dark(x + 1, y + 1) {
                score += 3;
            }
        }
    }
    let dark = s.modules.iter().filter(|&&v| v).count();
    score + (dark * 20).abs_diff(n * n * 10) / (n * n) * 10
}

// N3: 1:1:3:1:1 dark/light runs, with four light units before or after.
// Include the light exterior, and count both qualifying orientations, even when
// the same core has four light units on both sides. Ratios can be scaled.
fn finder_penalty(line: &[bool]) -> usize {
    let mut runs = vec![line.len()];
    let mut dark = false;
    for &value in line {
        if value == dark {
            *runs.last_mut().unwrap() += 1;
        } else {
            runs.push(1);
            dark = value;
        }
    }
    if dark {
        runs.push(line.len());
    } else {
        *runs.last_mut().unwrap() += line.len();
    }
    let mut score = 0;
    for i in (1..runs.len().saturating_sub(5)).step_by(2) {
        let unit = runs[i];
        if runs[i + 1] == unit
            && runs[i + 2] == 3 * unit
            && runs[i + 3] == unit
            && runs[i + 4] == unit
        {
            score += 40 * usize::from(runs[i - 1] >= 4 * unit && runs[i + 5] >= unit);
            score += 40 * usize::from(runs[i + 5] >= 4 * unit && runs[i - 1] >= unit);
        }
    }
    score
}

#[cfg(test)]
#[path = "qr/tests.rs"]
mod tests;
