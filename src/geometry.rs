/// Axis-aligned rectangle in physical pixels, relative to a monitor's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// Parses "x,y,w,h" (whitespace around numbers is ignored).
    pub fn parse(s: &str) -> Result<Rect, String> {
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return Err(format!("expected x,y,w,h – got '{s}'"));
        }
        let nums = parts
            .iter()
            .map(|p| p.parse::<i32>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("invalid number in '{s}': {e}"))?;
        let r = Rect { x: nums[0], y: nums[1], w: nums[2], h: nums[3] };
        if r.w <= 0 || r.h <= 0 {
            return Err(format!("width and height must be positive – got '{s}'"));
        }
        Ok(r)
    }

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

    /// Normalised texture coordinates for a `width`×`height` frame: [u0, v0, du, dv].
    pub fn to_uv(self, width: i32, height: i32) -> [f32; 4] {
        let (fw, fh) = (width as f32, height as f32);
        [self.x as f32 / fw, self.y as f32 / fh, self.w as f32 / fw, self.h as f32 / fh]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_four_numbers() {
        assert_eq!(Rect::parse("10, 20,300,400"), Ok(Rect { x: 10, y: 20, w: 300, h: 400 }));
    }

    #[test]
    fn rejects_wrong_count_and_bad_numbers() {
        assert!(Rect::parse("1,2,3").is_err());
        assert!(Rect::parse("1,2,3,x").is_err());
    }

    #[test]
    fn rejects_non_positive_size() {
        assert!(Rect::parse("0,0,0,10").is_err());
        assert!(Rect::parse("0,0,10,-1").is_err());
    }

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

    #[test]
    fn converts_to_uv() {
        let uv = Rect { x: 640, y: 360, w: 1280, h: 720 }.to_uv(2560, 1440);
        assert_eq!(uv, [0.25, 0.25, 0.5, 0.5]);
    }
}
