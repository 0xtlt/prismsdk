//! A small, dependency-free QR Code encoder (ISO/IEC 18004, model 2).
//!
//! Byte mode only, all 40 versions and all four error-correction levels. The
//! encoder picks the smallest version that fits and the mask with the lowest
//! standard penalty score.
//!
//! ```
//! use prism_ui::qr::{QrCode, QrEcc};
//!
//! let qr = QrCode::encode_text("https://example.com", QrEcc::Medium).unwrap();
//! assert_eq!(qr.size(), 17 + 4 * qr.version() as usize);
//! for (x, y, len) in qr.dark_runs() {
//!     // draw one rectangle per horizontal run of dark modules
//!     let _ = (x, y, len);
//! }
//! ```

/// Error-correction level: how much of the symbol can be damaged and still be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum QrEcc {
    /// Recovers about 7% of the codewords.
    Low,
    /// Recovers about 15% of the codewords.
    #[default]
    Medium,
    /// Recovers about 25% of the codewords.
    Quartile,
    /// Recovers about 30% of the codewords.
    High,
}

impl QrEcc {
    /// Row in the block tables.
    fn index(self) -> usize {
        self as usize
    }

    /// The two-bit value stored in the format information.
    fn format_bits(self) -> u32 {
        match self {
            QrEcc::Low => 1,
            QrEcc::Medium => 0,
            QrEcc::Quartile => 3,
            QrEcc::High => 2,
        }
    }
}

/// Why encoding failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QrError {
    /// The data does not fit in a version 40 symbol at the requested level.
    TooLong,
}

impl std::fmt::Display for QrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("data too long for a QR code at this error-correction level")
    }
}

impl std::error::Error for QrError {}

/// An encoded QR Code symbol: a square grid of dark and light modules.
///
/// The grid does not include the quiet zone; leave at least four light modules
/// around it when rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrCode {
    version: u8,
    ecc: QrEcc,
    mask: u8,
    size: usize,
    modules: Vec<bool>,
}

/// Error-correction codewords per block, indexed by `[ecc][version]`.
const ECC_PER_BLOCK: [[u8; 41]; 4] = [
    [
        0, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28,
        30, 30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
    [
        0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28,
        28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    ],
    [
        0, 13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28, 30,
        30, 30, 30, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
    [
        0, 17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30, 24,
        30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
];

/// Number of error-correction blocks, indexed by `[ecc][version]`.
const NUM_BLOCKS: [[u8; 41]; 4] = [
    [
        0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13,
        14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25,
    ],
    [
        0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21,
        23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
    ],
    [
        0, 1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25, 27, 29,
        34, 34, 35, 38, 40, 43, 45, 48, 51, 53, 56, 59, 62, 65, 68,
    ],
    [
        0, 1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30, 32,
        35, 37, 40, 42, 45, 48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81,
    ],
];

/// Modules left for data and error correction once every function pattern is drawn.
fn raw_data_modules(version: u8) -> usize {
    let v = version as usize;
    let mut n = (16 * v + 128) * v + 64;
    if v >= 2 {
        let align = v / 7 + 2;
        n -= (25 * align - 10) * align - 55;
        if v >= 7 {
            n -= 36;
        }
    }
    n
}

/// Data codewords (excluding error correction) for a version and level.
fn data_codewords(version: u8, ecc: QrEcc) -> usize {
    let (e, v) = (ecc.index(), version as usize);
    raw_data_modules(version) / 8 - ECC_PER_BLOCK[e][v] as usize * NUM_BLOCKS[e][v] as usize
}

/// Width of the byte-mode character count field.
fn count_bits(version: u8) -> usize {
    if version <= 9 { 8 } else { 16 }
}

/// The most bytes a version holds at a level.
fn byte_capacity(version: u8, ecc: QrEcc) -> usize {
    (data_codewords(version, ecc) * 8 - 4 - count_bits(version)) / 8
}

/// Centers of the alignment patterns along one axis (empty for version 1).
fn alignment_positions(version: u8) -> Vec<usize> {
    if version == 1 {
        return Vec::new();
    }
    let v = version as usize;
    let count = v / 7 + 2;
    let step = if v == 32 {
        26
    } else {
        (v * 4 + count * 2 + 1) / (count * 2 - 2) * 2
    };
    let mut out: Vec<usize> = (0..count - 1).map(|i| 4 * v + 10 - i * step).collect();
    out.push(6);
    out.reverse();
    out
}

/// Multiplication in GF(2^8) modulo x^8 + x^4 + x^3 + x^2 + 1 (0x11D).
fn gf_mul(a: u8, b: u8) -> u8 {
    let mut z: u16 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= ((b as u16 >> i) & 1) * a as u16;
    }
    z as u8
}

/// The Reed–Solomon generator polynomial of the given degree, highest term
/// (always 1) omitted.
fn rs_generator(degree: usize) -> Vec<u8> {
    let mut poly = vec![0u8; degree];
    poly[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            poly[j] = gf_mul(poly[j], root);
            if j + 1 < degree {
                poly[j] ^= poly[j + 1];
            }
        }
        root = gf_mul(root, 0x02);
    }
    poly
}

/// Error-correction codewords for one block of data.
fn rs_remainder(data: &[u8], generator: &[u8]) -> Vec<u8> {
    let mut rem = vec![0u8; generator.len()];
    for &b in data {
        let factor = b ^ rem.remove(0);
        rem.push(0);
        for (r, &g) in rem.iter_mut().zip(generator) {
            *r ^= gf_mul(g, factor);
        }
    }
    rem
}

/// The 15 format bits (level + mask, BCH-protected, XOR-masked with 0x5412).
fn format_bits(ecc: QrEcc, mask: u8) -> u32 {
    let data = ecc.format_bits() << 3 | mask as u32;
    let mut rem = data;
    for _ in 0..10 {
        rem = (rem << 1) ^ ((rem >> 9) * 0x537);
    }
    (data << 10 | rem) ^ 0x5412
}

/// The 18 version bits (BCH-protected with 0x1F25), for versions 7 and up.
fn version_bits(version: u8) -> u32 {
    let mut rem = version as u32;
    for _ in 0..12 {
        rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
    }
    (version as u32) << 12 | rem
}

/// Whether mask pattern `mask` flips the module at column `x`, row `y`.
fn mask_flips(mask: u8, x: usize, y: usize) -> bool {
    if mask == 5 {
        return x * y % 2 + x * y % 3 == 0;
    }
    let (value, modulus) = match mask {
        0 => (x + y, 2),
        1 => (y, 2),
        2 => (x, 3),
        3 => (x + y, 3),
        4 => (x / 3 + y / 2, 2),
        6 => (x * y % 2 + x * y % 3, 2),
        _ => ((x + y) % 2 + x * y % 3, 2),
    };
    value.is_multiple_of(modulus)
}

/// Packs the data segment, terminator and padding into data codewords.
fn data_bitstream(data: &[u8], version: u8, ecc: QrEcc) -> Vec<u8> {
    let capacity = data_codewords(version, ecc);
    let mut bits: Vec<bool> = Vec::with_capacity(capacity * 8);
    let mut push =
        |value: usize, len: usize| bits.extend((0..len).rev().map(|i| value >> i & 1 == 1));
    push(0b0100, 4);
    push(data.len(), count_bits(version));
    for &b in data {
        push(b as usize, 8);
    }
    let terminator = (capacity * 8 - bits.len()).min(4);
    bits.extend(std::iter::repeat_n(false, terminator));
    bits.resize(bits.len().div_ceil(8) * 8, false);
    let mut bytes: Vec<u8> = bits
        .chunks(8)
        .map(|c| c.iter().fold(0, |a, &b| a << 1 | b as u8))
        .collect();
    for pad in [0xEC, 0x11].into_iter().cycle() {
        if bytes.len() >= capacity {
            break;
        }
        bytes.push(pad);
    }
    bytes
}

/// Splits data into blocks, appends error correction and interleaves everything.
fn add_ecc_and_interleave(data: &[u8], version: u8, ecc: QrEcc) -> Vec<u8> {
    let (e, v) = (ecc.index(), version as usize);
    let blocks = NUM_BLOCKS[e][v] as usize;
    let ecc_len = ECC_PER_BLOCK[e][v] as usize;
    let raw = raw_data_modules(version) / 8;
    let short_blocks = blocks - raw % blocks;
    let short_data = raw / blocks - ecc_len;
    let generator = rs_generator(ecc_len);
    let mut data_blocks = Vec::with_capacity(blocks);
    let mut ecc_blocks = Vec::with_capacity(blocks);
    let mut offset = 0;
    for i in 0..blocks {
        let len = short_data + usize::from(i >= short_blocks);
        let block = &data[offset..offset + len];
        offset += len;
        ecc_blocks.push(rs_remainder(block, &generator));
        data_blocks.push(block);
    }
    let mut out = Vec::with_capacity(raw);
    for i in 0..=short_data {
        out.extend(data_blocks.iter().filter_map(|b| b.get(i)));
    }
    for i in 0..ecc_len {
        out.extend(ecc_blocks.iter().map(|b| b[i]));
    }
    out
}

/// The symbol while it is being built: modules plus which ones are function patterns.
struct Canvas {
    size: usize,
    modules: Vec<bool>,
    function: Vec<bool>,
}

impl Canvas {
    fn set(&mut self, x: usize, y: usize, dark: bool) {
        self.modules[y * self.size + x] = dark;
        self.function[y * self.size + x] = true;
    }

    /// Draws timing, finder, alignment, version patterns and reserves the format area.
    fn new(version: u8) -> Self {
        let size = 17 + 4 * version as usize;
        let mut c = Canvas {
            size,
            modules: vec![false; size * size],
            function: vec![false; size * size],
        };
        for i in 0..size {
            c.set(6, i, i % 2 == 0);
            c.set(i, 6, i % 2 == 0);
        }
        for (cx, cy) in [(3, 3), (size - 4, 3), (3, size - 4)] {
            for dy in -4isize..=4 {
                for dx in -4isize..=4 {
                    let (x, y) = (cx as isize + dx, cy as isize + dy);
                    if (0..size as isize).contains(&x) && (0..size as isize).contains(&y) {
                        let d = dx.abs().max(dy.abs());
                        c.set(x as usize, y as usize, d != 2 && d != 4);
                    }
                }
            }
        }
        let pos = alignment_positions(version);
        let last = pos.len().saturating_sub(1);
        for (i, &ay) in pos.iter().enumerate() {
            for (j, &ax) in pos.iter().enumerate() {
                if (i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0) {
                    continue;
                }
                for dy in -2isize..=2 {
                    for dx in -2isize..=2 {
                        let d = dx.abs().max(dy.abs());
                        c.set(
                            (ax as isize + dx) as usize,
                            (ay as isize + dy) as usize,
                            d != 1,
                        );
                    }
                }
            }
        }
        c.draw_format(0);
        if version >= 7 {
            let bits = version_bits(version);
            for i in 0..18 {
                let dark = bits >> i & 1 == 1;
                let (a, b) = (size - 11 + i % 3, i / 3);
                c.set(a, b, dark);
                c.set(b, a, dark);
            }
        }
        c
    }

    /// Writes both copies of the format information, plus the dark module.
    fn draw_format(&mut self, bits: u32) {
        let bit = |i: usize| bits >> i & 1 == 1;
        let s = self.size;
        for i in 0..6 {
            self.set(8, i, bit(i));
        }
        self.set(8, 7, bit(6));
        self.set(8, 8, bit(7));
        self.set(7, 8, bit(8));
        for i in 9..15 {
            self.set(14 - i, 8, bit(i));
        }
        for i in 0..8 {
            self.set(s - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set(8, s - 15 + i, bit(i));
        }
        self.set(8, s - 8, true);
    }

    /// Places codewords in the two-column zigzag, skipping function modules.
    /// Leftover remainder bits stay light.
    fn draw_codewords(&mut self, codewords: &[u8]) {
        let s = self.size;
        let total = codewords.len() * 8;
        let mut i = 0;
        let mut right = s - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            let upward = (right + 1) & 2 == 0;
            for vert in 0..s {
                let y = if upward { s - 1 - vert } else { vert };
                for x in [right, right - 1] {
                    if !self.function[y * s + x] && i < total {
                        self.modules[y * s + x] = codewords[i >> 3] >> (7 - (i & 7)) & 1 == 1;
                        i += 1;
                    }
                }
            }
            if right < 2 {
                break;
            }
            right -= 2;
        }
    }

    /// XORs a mask over every non-function module (applying it twice undoes it).
    fn apply_mask(&mut self, mask: u8) {
        let s = self.size;
        for y in 0..s {
            for x in 0..s {
                if !self.function[y * s + x] && mask_flips(mask, x, y) {
                    self.modules[y * s + x] ^= true;
                }
            }
        }
    }

    /// The standard penalty score (rules N1–N4); lower is better.
    fn penalty(&self) -> usize {
        let s = self.size;
        let at = |x: usize, y: usize| self.modules[y * s + x];
        let mut score = 0;
        let finder_like = [true, false, true, true, true, false, true];
        for horizontal in [true, false] {
            for a in 0..s {
                let line: Vec<bool> = (0..s)
                    .map(|b| if horizontal { at(b, a) } else { at(a, b) })
                    .collect();
                // N1: runs of five or more same-colored modules.
                let mut run = 1;
                for b in 1..=s {
                    if b < s && line[b] == line[b - 1] {
                        run += 1;
                    } else {
                        if run >= 5 {
                            score += run - 2;
                        }
                        run = 1;
                    }
                }
                // N3: 1:1:3:1:1 finder-like patterns with four light modules on
                // one side (the area beyond the symbol counts as light).
                let padded: Vec<bool> = [false; 4]
                    .into_iter()
                    .chain(line)
                    .chain([false; 4])
                    .collect();
                for w in padded.windows(11) {
                    if (w[..4] == [false; 4] && w[4..] == finder_like)
                        || (w[..7] == finder_like && w[7..] == [false; 4])
                    {
                        score += 40;
                    }
                }
            }
        }
        // N2: 2x2 blocks of one color.
        for y in 0..s - 1 {
            for x in 0..s - 1 {
                let c = at(x, y);
                if c == at(x + 1, y) && c == at(x, y + 1) && c == at(x + 1, y + 1) {
                    score += 3;
                }
            }
        }
        // N4: 10 points per 5% step away from a 50% dark ratio.
        let total = s * s;
        let dark = self.modules.iter().filter(|&&d| d).count();
        let k = (dark * 20)
            .abs_diff(total * 10)
            .div_ceil(total)
            .saturating_sub(1);
        score + k * 10
    }
}

impl QrCode {
    /// Encodes bytes in the smallest version (1–40) that fits at `ecc`.
    pub fn encode(data: &[u8], ecc: QrEcc) -> Result<QrCode, QrError> {
        Self::encode_with_min_version(data, ecc, 1)
    }

    /// Encodes the UTF-8 bytes of `text` in the smallest version that fits.
    pub fn encode_text(text: &str, ecc: QrEcc) -> Result<QrCode, QrError> {
        Self::encode(text.as_bytes(), ecc)
    }

    /// Encodes bytes in the smallest version that is at least `min_version`
    /// (clamped to 1–40) and fits at `ecc`; useful to get a fixed symbol size.
    pub fn encode_with_min_version(
        data: &[u8],
        ecc: QrEcc,
        min_version: u8,
    ) -> Result<QrCode, QrError> {
        let version = (min_version.clamp(1, 40)..=40)
            .find(|&v| data.len() <= byte_capacity(v, ecc))
            .ok_or(QrError::TooLong)?;
        Ok(Self::build(data, version, ecc, None))
    }

    /// Builds the symbol for a version that is known to fit, with a forced mask
    /// or the lowest-penalty one.
    fn build(data: &[u8], version: u8, ecc: QrEcc, mask: Option<u8>) -> QrCode {
        let codewords = add_ecc_and_interleave(&data_bitstream(data, version, ecc), version, ecc);
        let mut canvas = Canvas::new(version);
        canvas.draw_codewords(&codewords);
        let mask = mask.unwrap_or_else(|| {
            (0..8)
                .min_by_key(|&m| {
                    canvas.apply_mask(m);
                    canvas.draw_format(format_bits(ecc, m));
                    let score = canvas.penalty();
                    canvas.apply_mask(m);
                    score
                })
                .unwrap_or(0)
        });
        canvas.apply_mask(mask);
        canvas.draw_format(format_bits(ecc, mask));
        QrCode {
            version,
            ecc,
            mask,
            size: canvas.size,
            modules: canvas.modules,
        }
    }

    /// The version, 1–40.
    pub fn version(&self) -> u8 {
        self.version
    }

    /// The error-correction level.
    pub fn ecc(&self) -> QrEcc {
        self.ecc
    }

    /// The mask pattern that was applied, 0–7.
    pub fn mask(&self) -> u8 {
        self.mask
    }

    /// Modules per side: `17 + 4 * version` (quiet zone not included).
    pub fn size(&self) -> usize {
        self.size
    }

    /// Whether the module at column `x`, row `y` is dark; `false` out of range.
    pub fn dark(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.modules[y * self.size + x]
    }

    /// Horizontal runs of dark modules, row by row, as `(x, y, len)`, so a
    /// renderer can draw one rectangle per run.
    pub fn dark_runs(&self) -> Vec<(usize, usize, usize)> {
        let mut runs = Vec::new();
        for y in 0..self.size {
            let mut x = 0;
            while x < self.size {
                if self.dark(x, y) {
                    let start = x;
                    while self.dark(x, y) {
                        x += 1;
                    }
                    runs.push((start, y, x - start));
                } else {
                    x += 1;
                }
            }
        }
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVELS: [QrEcc; 4] = [QrEcc::Low, QrEcc::Medium, QrEcc::Quartile, QrEcc::High];

    /// Deterministic pseudo-random bytes.
    fn noise(len: usize, seed: u32) -> Vec<u8> {
        let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..len)
            .map(|_| {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (s >> 16) as u8
            })
            .collect()
    }

    /// Renders the symbol (4-module quiet zone, 4 px per module) and reads it back with rqrr.
    fn decode(qr: &QrCode) -> Vec<u8> {
        const PX: usize = 4;
        let side = (qr.size() + 8) * PX;
        let mut img = rqrr::PreparedImage::prepare_from_greyscale(side, side, |x, y| {
            let (mx, my) = ((x / PX).wrapping_sub(4), (y / PX).wrapping_sub(4));
            if qr.dark(mx, my) { 0 } else { 255 }
        });
        let grids = img.detect_grids();
        assert_eq!(
            grids.len(),
            1,
            "v{} {:?}: expected one grid",
            qr.version(),
            qr.ecc()
        );
        let mut out = Vec::new();
        let meta = grids[0].decode_to(&mut out).expect("rqrr failed to decode");
        assert_eq!(meta.version.0, qr.version() as usize);
        assert_eq!(meta.mask, qr.mask() as u16);
        assert_eq!(meta.ecc_level as u32, qr.ecc().format_bits());
        out
    }

    fn roundtrip(data: &[u8], ecc: QrEcc) -> QrCode {
        let qr = QrCode::encode(data, ecc).unwrap();
        assert_eq!(decode(&qr), data, "v{} {:?}", qr.version(), ecc);
        qr
    }

    /// Reads the first copy of the 15 format bits from a row-major matrix.
    fn read_format(m: &[bool], size: usize) -> u32 {
        let at = |x: usize, y: usize| m[y * size + x] as u32;
        let mut coords: Vec<(usize, usize)> = (0..6).map(|i| (8, i)).collect();
        coords.extend([(8, 7), (8, 8), (7, 8)]);
        coords.extend((9..15).map(|i| (14 - i, 8)));
        coords
            .iter()
            .enumerate()
            .fold(0, |acc, (i, &(x, y))| acc | at(x, y) << i)
    }

    #[test]
    fn format_and_version_reference_bits() {
        assert_eq!(format_bits(QrEcc::Low, 0), 0b111011111000100);
        assert_eq!(format_bits(QrEcc::Medium, 0), 0b101010000010010);
        assert_eq!(format_bits(QrEcc::Quartile, 0), 0b011010101011111);
        assert_eq!(format_bits(QrEcc::High, 0), 0b001011010001001);
        assert_eq!(format_bits(QrEcc::Medium, 5), 0b100000011001110);
        assert_eq!(format_bits(QrEcc::High, 7), 0b000100000111011);
        assert_eq!(version_bits(7), 0x07C94);
        assert_eq!(version_bits(21), 0x15683);
        assert_eq!(version_bits(40), 0x28C69);
        let qr = QrCode::encode_text("format", QrEcc::Quartile).unwrap();
        assert_eq!(
            read_format(&qr.modules, qr.size()),
            format_bits(QrEcc::Quartile, qr.mask())
        );
    }

    #[test]
    fn reed_solomon_reference() {
        // "HELLO WORLD" at 1-M (Thonky's tutorial): data codewords and their 10 EC codewords.
        let data = [
            32, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17,
        ];
        let ecc = rs_remainder(&data, &rs_generator(10));
        assert_eq!(ecc, [196, 35, 39, 119, 235, 215, 231, 226, 93, 23]);
    }

    #[test]
    fn sizes_and_capacities() {
        for v in 1..=40u8 {
            for ecc in LEVELS {
                let qr = QrCode::encode_with_min_version(b"", ecc, v).unwrap();
                assert_eq!((qr.version(), qr.size()), (v, 17 + 4 * v as usize));
                assert_eq!(qr.modules.len(), qr.size() * qr.size());
            }
        }
        let caps = |v| LEVELS.map(|e| byte_capacity(v, e));
        assert_eq!(caps(1), [17, 14, 11, 7]);
        assert_eq!(caps(7), [154, 122, 86, 64]);
        assert_eq!(caps(10), [271, 213, 151, 119]);
        assert_eq!(caps(27), [1465, 1125, 805, 625]);
        assert_eq!(caps(40), [2953, 2331, 1663, 1273]);
        assert_eq!(alignment_positions(1), Vec::<usize>::new());
        assert_eq!(alignment_positions(7), [6, 22, 38]);
        assert_eq!(alignment_positions(32), [6, 34, 60, 86, 112, 138]);
        assert_eq!(alignment_positions(40), [6, 30, 58, 86, 114, 142, 170]);
        assert_eq!(
            QrCode::encode_with_min_version(b"x", QrEcc::Low, 0)
                .unwrap()
                .version(),
            1
        );
        assert_eq!(
            QrCode::encode_with_min_version(b"x", QrEcc::Low, 99)
                .unwrap()
                .version(),
            40
        );
    }

    #[test]
    fn decodes_varied_content_at_every_level() {
        let binary: Vec<u8> = (0..=255).collect();
        let cases: [&[u8]; 6] = [
            b"",
            b"A",
            b"Hello, world!",
            b"https://example.com/path?query=prism&lang=fr#section-2",
            "Héllo wörld — こんにちは 🌍".as_bytes(),
            &binary,
        ];
        for ecc in LEVELS {
            for data in cases {
                roundtrip(data, ecc);
            }
        }
        let qr = QrCode::encode_text("Héllo — 🌍", QrEcc::Quartile).unwrap();
        assert_eq!(String::from_utf8(decode(&qr)).unwrap(), "Héllo — 🌍");
    }

    #[test]
    fn version_boundaries_decode() {
        for ecc in LEVELS {
            for v in [1u8, 2, 6, 7, 9, 10, 20, 26, 27, 40] {
                let cap = byte_capacity(v, ecc);
                let full = noise(cap, v as u32);
                assert_eq!(roundtrip(&full, ecc).version(), v, "{ecc:?} cap {cap}");
                let over = noise(cap + 1, v as u32);
                match QrCode::encode(&over, ecc) {
                    Ok(qr) => {
                        assert_eq!(qr.version(), v + 1);
                        assert_eq!(decode(&qr), over);
                    }
                    Err(e) => assert_eq!((v, e), (40, QrError::TooLong)),
                }
            }
        }
    }

    #[test]
    fn too_long() {
        assert_eq!(
            QrCode::encode(&[0; 2954], QrEcc::Low),
            Err(QrError::TooLong)
        );
        assert!(QrCode::encode(&[0; 2953], QrEcc::Low).is_ok());
        for (ecc, max) in LEVELS.into_iter().zip([2953, 2331, 1663, 1273]) {
            assert!(QrCode::encode(&vec![7; max], ecc).is_ok());
            assert_eq!(
                QrCode::encode(&vec![7; max + 1], ecc),
                Err(QrError::TooLong)
            );
            assert_eq!(
                QrCode::encode_text(&"é".repeat(max), ecc),
                Err(QrError::TooLong)
            );
        }
    }

    /// Module-for-module match with the `qrcode` crate for every version and
    /// level. Its mask choice may legitimately differ, so we compare against our
    /// symbol built with the mask it picked (read back from its format bits).
    #[test]
    fn matches_reference_encoder() {
        let theirs_levels = [
            qrcode::EcLevel::L,
            qrcode::EcLevel::M,
            qrcode::EcLevel::Q,
            qrcode::EcLevel::H,
        ];
        for v in 1..=40u8 {
            for (ecc, theirs_ecc) in LEVELS.into_iter().zip(theirs_levels) {
                let size = 17 + 4 * v as usize;
                let data = noise(byte_capacity(v, ecc) * 2 / 3, v as u32 * 7);
                let mut bits = qrcode::bits::Bits::new(qrcode::Version::Normal(v as i16));
                bits.push_byte_data(&data).unwrap();
                bits.push_terminator(theirs_ecc).unwrap();
                let theirs: Vec<bool> = qrcode::QrCode::with_bits(bits, theirs_ecc)
                    .unwrap()
                    .to_colors()
                    .into_iter()
                    .map(|c| c == qrcode::Color::Dark)
                    .collect();
                let format = read_format(&theirs, size);
                let mask = (0..8)
                    .find(|&m| format_bits(ecc, m) == format)
                    .expect("unknown format bits");
                let ours = QrCode::build(&data, v, ecc, Some(mask));
                assert!(
                    ours.modules == theirs,
                    "v{v} {ecc:?} mask {mask}: matrices differ"
                );
            }
        }
    }

    #[test]
    fn dark_runs_cover_exactly_the_dark_modules() {
        let qr = QrCode::encode_text("dark runs", QrEcc::High).unwrap();
        let mut grid = vec![false; qr.size() * qr.size()];
        let mut last: Option<(usize, usize, usize)> = None;
        for (x, y, len) in qr.dark_runs() {
            assert!(len > 0 && x + len <= qr.size());
            assert!(
                !qr.dark(x.wrapping_sub(1), y) && !qr.dark(x + len, y),
                "runs are maximal"
            );
            if let Some((lx, ly, ll)) = last {
                assert!(ly < y || lx + ll < x, "runs are ordered and disjoint");
            }
            for i in x..x + len {
                grid[y * qr.size() + i] = true;
            }
            last = Some((x, y, len));
        }
        assert_eq!(grid, qr.modules);
        assert!(!qr.dark(qr.size(), 0) && !qr.dark(0, qr.size()));
        assert!(
            qr.dark(0, 0) && qr.dark(8, qr.size() - 8),
            "finder corner and dark module"
        );
    }
}
