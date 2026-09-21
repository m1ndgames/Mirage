/// Axis-aligned rectangle in physical pixels, relative to a monitor's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// Clamps the rectangle into a `width`×`height` monitor. The result is never
    /// empty, so a rectangle that lies entirely outside becomes a 1×1 pixel in
    /// the bottom-right corner rather than a zero-sized crop.
    pub fn clamp_to(self, width: i32, height: i32) -> Rect {
        let x0 = self.x.clamp(0, width - 1);
        let y0 = self.y.clamp(0, height - 1);
        let x1 = (self.x + self.w).clamp(x0 + 1, width);
        let y1 = (self.y + self.h).clamp(y0 + 1, height);
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_to_monitor_bounds() {
        let r = Rect { x: -10, y: 1400, w: 3000, h: 100 };
        assert_eq!(r.clamp_to(2560, 1440), Rect { x: 0, y: 1400, w: 2560, h: 40 });
    }

    #[test]
    fn clamp_never_produces_empty_rect() {
        let r = Rect { x: 5000, y: 5000, w: 10, h: 10 };
        let c = r.clamp_to(2560, 1440);
        assert!(c.w >= 1 && c.h >= 1);
        assert!(c.x + c.w <= 2560 && c.y + c.h <= 1440);
    }
}
