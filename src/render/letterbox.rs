//! Quelle zentriert in das Ziel einpassen, Seitenverhältnis erhalten (schwarze Ränder).

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub fn letterbox(src_w: i32, src_h: i32, dst_w: i32, dst_h: i32) -> Rect {
    if src_w <= 0 || src_h <= 0 || dst_w <= 0 || dst_h <= 0 {
        return Rect::default();
    }
    // Höhe füllen, wenn die Quelle (relativ) höher ist als das Ziel, sonst Breite.
    if i64::from(src_w) * i64::from(dst_h) <= i64::from(dst_w) * i64::from(src_h) {
        let w = (f64::from(src_w) * f64::from(dst_h) / f64::from(src_h)).round() as i32;
        return Rect {
            x: (dst_w - w) / 2,
            y: 0,
            w,
            h: dst_h,
        };
    }
    let h = (f64::from(src_h) * f64::from(dst_w) / f64::from(src_w)).round() as i32;
    Rect {
        x: 0,
        y: (dst_h - h) / 2,
        w: dst_w,
        h,
    }
}
