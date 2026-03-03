// pixel coords -> NDC (Y flipped: screen top = NDC +1)
#[inline]
pub fn px(x: f32, y: f32, sw: f32, sh: f32) -> [f32; 2] {
    [(x / sw) * 2.0 - 1.0, 1.0 - (y / sh) * 2.0]
}
