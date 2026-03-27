/// Bitmap storage — uses heap only for actual bitmap data (Vec<u8>)

extern crate alloc;
use alloc::vec::Vec;

use crate::framebuffer::{FBType, ShadowBuffer, COLS, ROWS};

pub struct StoredBitmap {
    pub name: heapless::String<32>,
    pub width: u16,
    pub height: u16,
    pub data: Vec<u8>,
}

const MAX_BITMAPS: usize = 16;

pub struct BitmapStore {
    entries: heapless::Vec<StoredBitmap, MAX_BITMAPS>,
}

impl BitmapStore {
    pub fn new() -> Self {
        BitmapStore {
            entries: heapless::Vec::new(),
        }
    }

    pub fn store(&mut self, name: &str, width: u16, height: u16, data: Vec<u8>) {
        // Overwrite if exists
        for entry in &mut self.entries {
            if entry.name.as_str() == name {
                entry.width = width;
                entry.height = height;
                entry.data = data;
                return;
            }
        }
        let mut n = heapless::String::<32>::new();
        let _ = n.push_str(name);
        let _ = self.entries.push(StoredBitmap {
            name: n,
            width,
            height,
            data,
        });
    }

    pub fn show(
        &self,
        shadow: &mut ShadowBuffer,
        fb: &mut FBType,
        name: &str,
        x_off: usize,
        y_off: usize,
    ) -> bool {
        let bmp = match self.entries.iter().find(|e| e.name.as_str() == name) {
            Some(b) => b,
            None => return false,
        };

        for y in 0..bmp.height as usize {
            let row_off = y * bmp.width as usize * 3;
            for x in 0..bmp.width as usize {
                let px = x_off + x;
                let py = y_off + y;
                if px < COLS && py < ROWS {
                    let i = row_off + x * 3;
                    shadow.set_pixel(fb, px, py, bmp.data[i], bmp.data[i + 1], bmp.data[i + 2]);
                }
            }
        }
        true
    }

    pub fn list(&self) -> heapless::Vec<&str, MAX_BITMAPS> {
        let mut names = heapless::Vec::new();
        for entry in &self.entries {
            let _ = names.push(entry.name.as_str());
        }
        names
    }
}
