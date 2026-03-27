/// TEXT overlay tracking for CLRTXT support — stack allocated

use crate::framebuffer::{FBType, ShadowBuffer};

struct TextRegion {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

const MAX_REGIONS: usize = 32;

pub struct TextOverlayManager {
    regions: heapless::Vec<TextRegion, MAX_REGIONS>,
}

impl TextOverlayManager {
    pub fn new() -> Self {
        TextOverlayManager {
            regions: heapless::Vec::new(),
        }
    }

    pub fn record(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let x = x.max(0) as usize;
        let y = y.max(0) as usize;
        let w = w.max(0) as usize;
        let h = h.max(0) as usize;
        if w > 0 && h > 0 {
            let _ = self.regions.push(TextRegion { x, y, w, h });
        }
    }

    pub fn clear_all(&mut self, shadow: &mut ShadowBuffer, fb: &mut FBType) {
        for region in &self.regions {
            shadow.fill_rect(fb, region.x, region.y, region.w, region.h, 0, 0, 0);
        }
        self.regions.clear();
    }

    pub fn reset(&mut self) {
        self.regions.clear();
    }
}
