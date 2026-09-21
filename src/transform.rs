use serde::{Deserialize, Serialize};

use crate::geometry::Rect;

/// How a source region is fitted into its target rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    /// Distort to fill the target completely.
    Stretch,
    /// Keep the aspect ratio, letterbox/pillarbox the rest (black).
    #[default]
    Fit,
    /// Keep the aspect ratio, fill the target, crop what doesn't fit.
    Fill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Filter {
    #[default]
    Linear,
    Nearest,
}

/// Geometric transform of one mapping. Rotation is clockwise, in degrees,
/// one of 0/90/180/270 (anything else is treated as 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Transform {
    pub flip_h: bool,
    pub flip_v: bool,
    pub rotation: u16,
    pub fit: Fit,
    pub filter: Filter,
}

/// Where and how to draw one mapping: the viewport inside the target window
/// and a row-major 2×3 matrix mapping quad UV (0..1 over the viewport) to
/// source texture UV: `su = m[0]*u + m[1]*v + m[2]`, `sv = m[3]*u + m[4]*v + m[5]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub dst: Rect,
    pub matrix: [f32; 6],
}

/// Computes the viewport and UV matrix for `src` (physical pixels of a
/// `src_size` frame) drawn into `dst` (physical pixels of the target window).
pub fn layout(t: &Transform, src: Rect, src_size: (i32, i32), dst: Rect) -> Layout {
    let quarter_turn = matches!(t.rotation, 90 | 270);
    // Size of the region as displayed (rotation swaps the axes).
    let (disp_w, disp_h) = if quarter_turn { (src.h as f64, src.w as f64) } else { (src.w as f64, src.h as f64) };

    let (viewport, crop) = match t.fit {
        Fit::Stretch => (dst, (0.0, 0.0, 1.0, 1.0)),
        Fit::Fit => {
            let scale = (dst.w as f64 / disp_w).min(dst.h as f64 / disp_h);
            let vw = (disp_w * scale).round().max(1.0) as i32;
            let vh = (disp_h * scale).round().max(1.0) as i32;
            let viewport = Rect { x: dst.x + (dst.w - vw) / 2, y: dst.y + (dst.h - vh) / 2, w: vw, h: vh };
            (viewport, (0.0, 0.0, 1.0, 1.0))
        }
        Fit::Fill => {
            let scale = (dst.w as f64 / disp_w).max(dst.h as f64 / disp_h);
            let fx = (dst.w as f64 / scale / disp_w).min(1.0);
            let fy = (dst.h as f64 / scale / disp_h).min(1.0);
            (dst, (0.5 - fx / 2.0, 0.5 - fy / 2.0, fx, fy))
        }
    };

    // Quad UV → display space (fill crop) → flips → rotation → source crop UV.
    let (u0, v0) = (src.x as f64 / src_size.0 as f64, src.y as f64 / src_size.1 as f64);
    let (du, dv) = (src.w as f64 / src_size.0 as f64, src.h as f64 / src_size.1 as f64);
    let f = |u: f64, v: f64| -> (f64, f64) {
        let mut d = (crop.0 + u * crop.2, crop.1 + v * crop.3);
        if t.flip_h {
            d.0 = 1.0 - d.0;
        }
        if t.flip_v {
            d.1 = 1.0 - d.1;
        }
        let s = match t.rotation {
            90 => (d.1, 1.0 - d.0),
            180 => (1.0 - d.0, 1.0 - d.1),
            270 => (1.0 - d.1, d.0),
            _ => d,
        };
        (u0 + s.0 * du, v0 + s.1 * dv)
    };
    let p0 = f(0.0, 0.0);
    let px = f(1.0, 0.0);
    let py = f(0.0, 1.0);
    let matrix = [
        (px.0 - p0.0) as f32,
        (py.0 - p0.0) as f32,
        p0.0 as f32,
        (px.1 - p0.1) as f32,
        (py.1 - p0.1) as f32,
        p0.1 as f32,
    ];
    Layout { dst: viewport, matrix }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC_SIZE: (i32, i32) = (2560, 1440);
    const SRC: Rect = Rect { x: 0, y: 0, w: 1280, h: 720 };
    const DST: Rect = Rect { x: 0, y: 0, w: 768, h: 1024 };

    fn close(a: [f32; 6], b: [f32; 6]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    fn apply(m: [f32; 6], u: f32, v: f32) -> (f32, f32) {
        (m[0] * u + m[1] * v + m[2], m[3] * u + m[4] * v + m[5])
    }

    #[test]
    fn stretch_without_transform_is_the_plain_crop() {
        let t = Transform { fit: Fit::Stretch, ..Default::default() };
        let l = layout(&t, SRC, SRC_SIZE, DST);
        assert_eq!(l.dst, DST);
        assert!(close(l.matrix, [0.5, 0.0, 0.0, 0.0, 0.5, 0.0]), "{:?}", l.matrix);
    }

    #[test]
    fn offset_crop_lands_in_the_matrix_translation() {
        let t = Transform { fit: Fit::Stretch, ..Default::default() };
        let l = layout(&t, Rect { x: 1280, y: 720, w: 640, h: 360 }, SRC_SIZE, DST);
        assert!(close(l.matrix, [0.25, 0.0, 0.5, 0.0, 0.25, 0.5]), "{:?}", l.matrix);
    }

    #[test]
    fn flips_mirror_the_axes() {
        let t = Transform { flip_h: true, fit: Fit::Stretch, ..Default::default() };
        let m = layout(&t, SRC, SRC_SIZE, DST).matrix;
        assert_eq!(apply(m, 0.0, 0.0), (0.5, 0.0), "left edge samples the right edge of the crop");
        assert_eq!(apply(m, 1.0, 0.0), (0.0, 0.0));
        let t = Transform { flip_v: true, fit: Fit::Stretch, ..Default::default() };
        let m = layout(&t, SRC, SRC_SIZE, DST).matrix;
        assert_eq!(apply(m, 0.0, 0.0), (0.0, 0.5));
    }

    #[test]
    fn rotation_90_clockwise_puts_source_top_left_at_display_top_right() {
        let t = Transform { rotation: 90, fit: Fit::Stretch, ..Default::default() };
        let m = layout(&t, SRC, SRC_SIZE, DST).matrix;
        assert_eq!(apply(m, 1.0, 0.0), (0.0, 0.0));
        assert_eq!(apply(m, 0.0, 0.0), (0.0, 0.5), "display top-left shows source bottom-left");
        assert_eq!(apply(m, 1.0, 1.0), (0.5, 0.0), "display bottom-right shows source top-right");
    }

    #[test]
    fn rotation_180_and_270() {
        let t = Transform { rotation: 180, fit: Fit::Stretch, ..Default::default() };
        let m = layout(&t, SRC, SRC_SIZE, DST).matrix;
        assert_eq!(apply(m, 0.0, 0.0), (0.5, 0.5));
        let t = Transform { rotation: 270, fit: Fit::Stretch, ..Default::default() };
        let m = layout(&t, SRC, SRC_SIZE, DST).matrix;
        assert_eq!(apply(m, 0.0, 0.0), (0.5, 0.0), "display top-left shows source top-right");
    }

    #[test]
    fn fit_letterboxes_a_wide_region_on_a_portrait_target() {
        let t = Transform::default();
        let l = layout(&t, SRC, SRC_SIZE, DST);
        assert_eq!(l.dst, Rect { x: 0, y: 296, w: 768, h: 432 });
        assert!(close(l.matrix, [0.5, 0.0, 0.0, 0.0, 0.5, 0.0]));
    }

    #[test]
    fn fit_with_rotation_uses_the_rotated_aspect() {
        let t = Transform { rotation: 90, ..Default::default() };
        let l = layout(&t, SRC, SRC_SIZE, DST);
        assert_eq!(l.dst, Rect { x: 96, y: 0, w: 576, h: 1024 });
    }

    #[test]
    fn fit_respects_a_target_sub_rect() {
        let t = Transform::default();
        let l = layout(&t, SRC, SRC_SIZE, Rect { x: 0, y: 512, w: 768, h: 512 });
        assert_eq!(l.dst, Rect { x: 0, y: 552, w: 768, h: 432 });
    }

    #[test]
    fn fill_crops_the_source_horizontally_when_the_target_is_taller() {
        let t = Transform { fit: Fit::Fill, ..Default::default() };
        let l = layout(&t, SRC, SRC_SIZE, DST);
        assert_eq!(l.dst, DST);
        // Visible: 768/1.4222 = 540 of 1280 source px, centred → starts at 370 px = 0.1445 uv.
        let (u, v) = apply(l.matrix, 0.0, 0.0);
        assert!((u - 370.0 / 2560.0).abs() < 1e-3, "{u}");
        assert!(v.abs() < 1e-6);
        let (u1, v1) = apply(l.matrix, 1.0, 1.0);
        assert!((u1 - 910.0 / 2560.0).abs() < 1e-3, "{u1}");
        assert!((v1 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn invalid_rotation_is_treated_as_zero() {
        let t = Transform { rotation: 45, fit: Fit::Stretch, ..Default::default() };
        let m = layout(&t, SRC, SRC_SIZE, DST).matrix;
        assert!(close(m, [0.5, 0.0, 0.0, 0.0, 0.5, 0.0]));
    }

    #[test]
    fn enums_serialise_lowercase() {
        #[derive(Serialize, Deserialize)]
        struct W {
            fit: Fit,
            filter: Filter,
        }
        let text = toml::to_string(&W { fit: Fit::Fill, filter: Filter::Nearest }).unwrap();
        assert!(text.contains("fit = \"fill\"") && text.contains("filter = \"nearest\""), "{text}");
        let back: W = toml::from_str("fit = \"stretch\"\nfilter = \"linear\"\n").unwrap();
        assert_eq!(back.fit, Fit::Stretch);
        assert_eq!(back.filter, Filter::Linear);
    }
}
