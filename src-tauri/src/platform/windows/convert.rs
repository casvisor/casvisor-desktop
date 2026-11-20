/// Converts a BGRA frame to NV12 (BT.601, limited range), shrinking it by
/// `scale` (1 or 2). `width` and `height` are the output size and must be even.
#[allow(clippy::too_many_arguments)]
pub fn bgra_to_nv12(
    src: &[u8],
    pitch: usize,
    src_width: usize,
    src_height: usize,
    scale: usize,
    width: usize,
    height: usize,
    out: &mut Vec<u8>,
) {
    out.resize(width * height * 3 / 2, 0);
    let (luma, chroma) = out.split_at_mut(width * height);

    let pixel = |x: usize, y: usize| -> (i32, i32, i32) {
        let x = x.min(src_width - 1);
        let y = y.min(src_height - 1);
        let i = y * pitch + x * 4;
        (src[i + 2] as i32, src[i + 1] as i32, src[i] as i32)
    };
    let sample = |x: usize, y: usize| -> (i32, i32, i32) {
        if scale == 1 {
            return pixel(x, y);
        }
        let (sx, sy) = (x * scale, y * scale);
        let a = pixel(sx, sy);
        let b = pixel(sx + 1, sy);
        let c = pixel(sx, sy + 1);
        let d = pixel(sx + 1, sy + 1);
        (
            (a.0 + b.0 + c.0 + d.0) >> 2,
            (a.1 + b.1 + c.1 + d.1) >> 2,
            (a.2 + b.2 + c.2 + d.2) >> 2,
        )
    };
    let y_of = |(r, g, b): (i32, i32, i32)| (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;

    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let p00 = sample(x, y);
            let p01 = sample(x + 1, y);
            let p10 = sample(x, y + 1);
            let p11 = sample(x + 1, y + 1);
            luma[y * width + x] = y_of(p00);
            luma[y * width + x + 1] = y_of(p01);
            luma[(y + 1) * width + x] = y_of(p10);
            luma[(y + 1) * width + x + 1] = y_of(p11);

            let r = (p00.0 + p01.0 + p10.0 + p11.0) >> 2;
            let g = (p00.1 + p01.1 + p10.1 + p11.1) >> 2;
            let b = (p00.2 + p01.2 + p10.2 + p11.2) >> 2;
            let i = (y / 2) * width + x;
            chroma[i] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            chroma[i + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    }
}
