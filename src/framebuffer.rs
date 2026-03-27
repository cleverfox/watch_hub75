/// Shared framebuffer wrapper with brightness scaling and shadow buffer
use embedded_graphics::pixelcolor::Rgb888;
use esp_hub75::framebuffer::plain::DmaFrameBuffer;

pub const ROWS: usize = 64;
pub const COLS: usize = 128;
pub const BITS: u8 = 4;
pub const NROWS: usize = esp_hub75::framebuffer::compute_rows(ROWS);
pub const FRAME_COUNT: usize = esp_hub75::framebuffer::compute_frame_count(BITS);

pub type FBType = DmaFrameBuffer<ROWS, COLS, NROWS, BITS, FRAME_COUNT>;

/// Shadow buffer stores original RGB values (before brightness scaling)
/// Layout: [r, g, b, r, g, b, ...] row-major, ROWS * COLS * 3 bytes
pub struct ShadowBuffer {
    pub data: [u8; ROWS * COLS * 3],
    pub brightness: u8, // 1-100
}

impl ShadowBuffer {
    pub const fn new() -> Self {
        ShadowBuffer {
            data: [0u8; ROWS * COLS * 3],
            brightness: 100,
        }
    }

    #[inline]
    fn offset(x: usize, y: usize) -> usize {
        (y * COLS + x) * 3
    }

    /// Get original RGB at pixel
    pub fn get_pixel(&self, x: usize, y: usize) -> (u8, u8, u8) {
        if x >= COLS || y >= ROWS {
            return (0, 0, 0);
        }
        let off = Self::offset(x, y);
        (self.data[off], self.data[off + 1], self.data[off + 2])
    }

    /// Set pixel in shadow buffer and apply brightness to framebuffer.
    /// Skips DMA write if the shadow buffer already has the same color (no-op).
    #[inline]
    pub fn set_pixel(&mut self, fb: &mut FBType, x: usize, y: usize, r: u8, g: u8, b: u8) {
        if x >= COLS || y >= ROWS {
            return;
        }
        let off = Self::offset(x, y);
        // Skip if pixel unchanged
        if self.data[off] == r && self.data[off + 1] == g && self.data[off + 2] == b {
            return;
        }
        self.data[off] = r;
        self.data[off + 1] = g;
        self.data[off + 2] = b;

        let (br, bg, bb) = self.apply_brightness(r, g, b);
        use embedded_graphics::geometry::Point;
        use esp_hub75::framebuffer::FrameBufferOperations;
        fb.set_pixel(Point::new(x as i32, y as i32), Rgb888::new(br, bg, bb));
    }

    /// Set pixel from packed RGB integer
    pub fn set_pixel_rgb(&mut self, fb: &mut FBType, x: usize, y: usize, rgb: u32) {
        let r = ((rgb >> 16) & 0xFF) as u8;
        let g = ((rgb >> 8) & 0xFF) as u8;
        let b = (rgb & 0xFF) as u8;
        self.set_pixel(fb, x, y, r, g, b);
    }

    /// Fill a rectangle
    pub fn fill_rect(&mut self, fb: &mut FBType, x: usize, y: usize, w: usize, h: usize, r: u8, g: u8, b: u8) {
        for dy in 0..h {
            for dx in 0..w {
                self.set_pixel(fb, x + dx, y + dy, r, g, b);
            }
        }
    }

    /// Clear entire screen
    pub fn clear(&mut self, fb: &mut FBType, r: u8, g: u8, b: u8) {
        self.fill_rect(fb, 0, 0, COLS, ROWS, r, g, b);
    }

    /// Change brightness and reapply to all pixels
    pub fn set_brightness(&mut self, fb: &mut FBType, brightness: u8) {
        self.brightness = brightness.clamp(1, 100);
        // Reapply all pixels from shadow to framebuffer
        for y in 0..ROWS {
            for x in 0..COLS {
                let off = Self::offset(x, y);
                let r = self.data[off];
                let g = self.data[off + 1];
                let b = self.data[off + 2];
                let (br, bg, bb) = self.apply_brightness(r, g, b);
                use embedded_graphics::geometry::Point;
                use esp_hub75::framebuffer::FrameBufferOperations;
                fb.set_pixel(Point::new(x as i32, y as i32), Rgb888::new(br, bg, bb));
            }
        }
    }

    #[inline]
    fn apply_brightness(&self, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        if self.brightness >= 100 {
            return (r, g, b);
        }
        let br = (r as u16 * self.brightness as u16 / 100) as u8;
        let bg = (g as u16 * self.brightness as u16 / 100) as u8;
        let bb = (b as u16 * self.brightness as u16 / 100) as u8;
        (br, bg, bb)
    }
}
