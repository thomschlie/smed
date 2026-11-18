//! LCD frames (docs/protocol.md, "LCD frame"): a 512-byte header, then the image as raw
//! deflate of 640×150 pixels in BGR565 little-endian (blue in the high bits), sent as two bulk transfers to
//! endpoint 0x01 of interface 0.
//!
//! The device shows only images deflated as **one final block with the fixed Huffman codes**
//! (BTYPE 1), the form of every frame 3DxWare sends; a stream with dynamic Huffman blocks is
//! accepted on USB but not shown. So smed has its own small encoder for that form.

/// The panel in pixels.
pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 150;

/// The header is one 512-byte transfer.
pub const HEADER_LEN: usize = 512;

/// Frame kind `0x11`: a full frame. smed sends only these (no `0x18` transition steps).
const KIND_FULL: u8 = 0x11;

/// One frame, ready for the two bulk transfers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub header: Vec<u8>,
    pub image: Vec<u8>,
}

/// Packs RGBA pixels (row by row, `WIDTH × HEIGHT × 4` bytes, alpha ignored) into a frame.
pub fn encode(rgba: &[u8]) -> Frame {
    assert_eq!(rgba.len(), WIDTH * HEIGHT * 4, "an LCD frame is 640×150");
    let mut bgr565 = Vec::with_capacity(WIDTH * HEIGHT * 2);
    let (pixels, _) = rgba.as_chunks::<4>();
    for [r, g, b, _] in pixels {
        bgr565.extend_from_slice(&to_bgr565(*r, *g, *b).to_le_bytes());
    }
    let image = deflate_fixed(&bgr565);
    Frame {
        header: header(image.len()),
        image,
    }
}

/// `0x11 0x0f`, the image length as u32 LE at 2-5, `3f 01 ef 00` at 11-14, `0x0f` at 15, zeros
/// to 512: what 3DxWare sends for a full frame (bytes 11-14 are not understood, but constant).
fn header(image_len: usize) -> Vec<u8> {
    let mut header = vec![0_u8; HEADER_LEN];
    header[0] = KIND_FULL;
    header[1] = 0x0f;
    header[2..6].copy_from_slice(&(image_len as u32).to_le_bytes());
    header[11..15].copy_from_slice(&[0x3f, 0x01, 0xef, 0x00]);
    header[15] = 0x0f;
    header
}

/// Bytes per row of the BGR565 image.
const ROW: usize = WIDTH * 2;

/// Match distances tried, in order: the same pixel, the row above, and two pixels back.
const DISTANCES: [usize; 4] = [2, ROW, 4, 2 * ROW];
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;

/// Raw deflate of `data` as a single final block with the fixed Huffman codes (RFC 1951,
/// 3.2.6). LZ77 looks back only at [`DISTANCES`]: enough for flat UI pages.
pub fn deflate_fixed(data: &[u8]) -> Vec<u8> {
    let mut out = BitWriter::default();
    out.bits(1, 1); // BFINAL
    out.bits(1, 2); // BTYPE 01: fixed Huffman
    let mut at = 0;
    while at < data.len() {
        let (length, distance) = DISTANCES
            .iter()
            .filter(|&&distance| distance <= at)
            .map(|&distance| {
                let length = data[at..]
                    .iter()
                    .zip(&data[at - distance..])
                    .take(MAX_MATCH)
                    .take_while(|(a, b)| a == b)
                    .count();
                (length, distance)
            })
            .max_by_key(|&(length, distance)| (length, std::cmp::Reverse(distance)))
            .unwrap_or((0, 0));
        if length >= MIN_MATCH {
            out.length(length);
            out.distance(distance);
            at += length;
        } else {
            out.literal(data[at]);
            at += 1;
        }
    }
    out.symbol(256); // end of block
    out.finish()
}

/// Deflate's bit order: values LSB first, Huffman codes MSB first.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    buffer: u32,
    count: u32,
}

/// (base length, extra bits) of length codes 257-285.
const LENGTHS: [(u16, u8); 29] = [
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (8, 0),
    (9, 0),
    (10, 0),
    (11, 1),
    (13, 1),
    (15, 1),
    (17, 1),
    (19, 2),
    (23, 2),
    (27, 2),
    (31, 2),
    (35, 3),
    (43, 3),
    (51, 3),
    (59, 3),
    (67, 4),
    (83, 4),
    (99, 4),
    (115, 4),
    (131, 5),
    (163, 5),
    (195, 5),
    (227, 5),
    (258, 0),
];

/// (base distance, extra bits) of distance codes 0-29.
const DISTANCE_CODES: [(u16, u8); 30] = [
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 1),
    (7, 1),
    (9, 2),
    (13, 2),
    (17, 3),
    (25, 3),
    (33, 4),
    (49, 4),
    (65, 5),
    (97, 5),
    (129, 6),
    (193, 6),
    (257, 7),
    (385, 7),
    (513, 8),
    (769, 8),
    (1025, 9),
    (1537, 9),
    (2049, 10),
    (3073, 10),
    (4097, 11),
    (6145, 11),
    (8193, 12),
    (12289, 12),
    (16385, 13),
    (24577, 13),
];

impl BitWriter {
    /// `count` bits of `value`, LSB first.
    fn bits(&mut self, value: u32, count: u32) {
        self.buffer |= value << self.count;
        self.count += count;
        while self.count >= 8 {
            self.bytes.push(self.buffer as u8);
            self.buffer >>= 8;
            self.count -= 8;
        }
    }

    /// A Huffman code of `length` bits, MSB first.
    fn code(&mut self, code: u32, length: u32) {
        self.bits(code.reverse_bits() >> (32 - length), length);
    }

    /// A literal/length symbol (0-287) in the fixed code.
    fn symbol(&mut self, symbol: u16) {
        let symbol = u32::from(symbol);
        match symbol {
            0..=143 => self.code(0x30 + symbol, 8),
            144..=255 => self.code(0x190 + symbol - 144, 9),
            256..=279 => self.code(symbol - 256, 7),
            _ => self.code(0xc0 + symbol - 280, 8),
        }
    }

    fn literal(&mut self, byte: u8) {
        self.symbol(u16::from(byte));
    }

    fn length(&mut self, length: usize) {
        let length = length as u16;
        let index = LENGTHS
            .iter()
            .rposition(|&(base, _)| base <= length)
            .unwrap_or(0);
        let (base, extra) = LENGTHS[index];
        self.symbol(257 + index as u16);
        self.bits(u32::from(length - base), u32::from(extra));
    }

    fn distance(&mut self, distance: usize) {
        let distance = distance as u16;
        let index = DISTANCE_CODES
            .iter()
            .rposition(|&(base, _)| base <= distance)
            .unwrap_or(0);
        let (base, extra) = DISTANCE_CODES[index];
        self.code(index as u32, 5);
        self.bits(u32::from(distance - base), u32::from(extra));
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes.push(self.buffer as u8);
        }
        self.bytes
    }
}

/// One pixel as the LCD takes it: blue in bits 11-15, green 5-10, red 0-4. (3DxWare's frames
/// decoded as RGB show Blender's orange logo blue; the device shows it orange.)
pub fn to_bgr565(r: u8, g: u8, b: u8) -> u16 {
    (u16::from(b >> 3) << 11) | (u16::from(g >> 2) << 5) | u16::from(r >> 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgr565_puts_blue_high_and_red_low() {
        assert_eq!(to_bgr565(0xff, 0xff, 0xff), 0xffff);
        assert_eq!(to_bgr565(0, 0, 0xff), 0xf800);
        assert_eq!(to_bgr565(0, 0xff, 0), 0x07e0);
        assert_eq!(to_bgr565(0xff, 0, 0), 0x001f);
        assert_eq!(to_bgr565(0x07, 0x03, 0x07), 0, "low bits are dropped");
    }

    #[test]
    fn the_header_matches_3dxware_full_frames() {
        let header = header(0x0001_2345);
        assert_eq!(header.len(), 512);
        assert_eq!(
            &header[..16],
            &[
                0x11, 0x0f, 0x45, 0x23, 0x01, 0x00, 0, 0, 0, 0, 0, 0x3f, 0x01, 0xef, 0x00, 0x0f
            ]
        );
        assert!(header[16..].iter().all(|&b| b == 0));
    }

    #[test]
    fn the_image_inflates_to_bgr565_little_endian() {
        let mut rgba = vec![0_u8; WIDTH * HEIGHT * 4];
        rgba[..4].copy_from_slice(&[0xff, 0, 0, 0xff]); // first pixel red
        let frame = encode(&rgba);
        assert_eq!(
            u32::from_le_bytes(frame.header[2..6].try_into().unwrap()) as usize,
            frame.image.len()
        );
        let raw = miniz_oxide::inflate::decompress_to_vec(&frame.image).unwrap();
        assert_eq!(raw.len(), 192_000);
        assert_eq!(&raw[..4], &[0x1f, 0x00, 0x00, 0x00], "red is the low bits");
    }

    #[test]
    fn the_image_is_one_final_fixed_huffman_block_like_3dxware() {
        let frame = encode(&vec![0x40_u8; WIDTH * HEIGHT * 4]);
        assert_eq!(frame.image[0] & 0b111, 0b011, "BFINAL 1, BTYPE 01");
        assert!(
            frame.image.len() < 2000,
            "a flat page compresses: {}",
            frame.image.len()
        );
    }

    #[test]
    fn the_fixed_encoder_round_trips_every_length_and_distance() {
        // Bytes with runs of many lengths, repeats one and two rows back, and noise.
        let mut data = Vec::new();
        let mut seed = 7_u32;
        for row in 0..40 {
            for column in 0..ROW {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = (seed >> 16) as u8;
                data.push(match (row % 4, column % 300) {
                    (0, c) if c < 290 => (c / 7) as u8,
                    (1, _) => data[data.len() - ROW],
                    (2, _) if column > 4 => data[data.len() - 2],
                    _ => noise,
                });
            }
        }
        let packed = deflate_fixed(&data);
        assert_eq!(
            miniz_oxide::inflate::decompress_to_vec(&packed).unwrap(),
            data
        );
        assert!(deflate_fixed(&[]).len() <= 2);
        assert_eq!(
            miniz_oxide::inflate::decompress_to_vec(&deflate_fixed(&[1, 2, 3])).unwrap(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn a_stream_ending_on_a_byte_boundary_gets_no_extra_byte() {
        // The block header (3 bits), six 9-bit literals and the 7-bit end of block: 64 bits.
        let data = [150, 151, 152, 153, 154, 155];
        let packed = deflate_fixed(&data);
        assert_eq!(packed.len(), 8);
        assert_eq!(
            miniz_oxide::inflate::decompress_to_vec(&packed).unwrap(),
            data
        );
    }
}
