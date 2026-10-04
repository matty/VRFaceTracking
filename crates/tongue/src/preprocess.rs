//! Camera frame preprocessing, matching the reference's OpenCV pipeline.

/// Width of one mouth camera view in the 800x400 stereo strip.
pub const VIEW: usize = 400;
pub const STRIP_WIDTH: usize = VIEW * 2;
pub const FRAME_BYTES: usize = STRIP_WIDTH * VIEW;

/// One source sample's weight in a destination pixel.
struct Tap {
    destination: usize,
    source: usize,
    alpha: f32,
}

/// OpenCV's `computeResizeAreaTab`: the source pixels each destination
/// pixel averages, weighted by overlap.
fn area_taps(source: usize, destination: usize) -> Vec<Tap> {
    let scale = 1.0 / (destination as f64 / source as f64);
    let mut taps = vec![];
    for dx in 0..destination {
        let fsx1 = dx as f64 * scale;
        let fsx2 = fsx1 + scale;
        let cell = scale.min(source as f64 - fsx1);
        let mut sx2 = fsx2.floor() as usize;
        sx2 = sx2.min(source - 1);
        let sx1 = (fsx1.ceil() as usize).min(sx2);
        if sx1 as f64 - fsx1 > 1e-3 {
            taps.push(Tap {
                destination: dx,
                source: sx1 - 1,
                alpha: ((sx1 as f64 - fsx1) / cell) as f32,
            });
        }
        for sx in sx1..sx2 {
            taps.push(Tap {
                destination: dx,
                source: sx,
                alpha: (1.0 / cell) as f32,
            });
        }
        if fsx2 - sx2 as f64 > 1e-3 {
            taps.push(Tap {
                destination: dx,
                source: sx2,
                alpha: ((fsx2 - sx2 as f64).min(1.0).min(cell) / cell) as f32,
            });
        }
    }
    taps
}

/// Area resampling of one square gray8 view, as `cv2.resize(...,
/// interpolation=cv2.INTER_AREA)` does for a non-integer shrink, including
/// its float accumulation order and round-half-to-even output.
pub struct AreaResize {
    size: usize,
    taps: Vec<Tap>,
    /// Per destination column, its first source column and how many it
    /// averages; their weights are `weights[offset..]` in tap order.
    columns: Vec<(usize, usize, usize)>,
    weights: Vec<f32>,
}

impl AreaResize {
    pub fn new(size: usize) -> Self {
        assert!(size > 0 && size <= VIEW, "tongue views only shrink");
        let taps = area_taps(VIEW, size);
        // Each destination column's taps are consecutive source columns.
        let mut columns = vec![];
        let mut weights = vec![];
        for dx in 0..size {
            let mine: Vec<&Tap> = taps.iter().filter(|tap| tap.destination == dx).collect();
            columns.push((mine[0].source, mine.len(), weights.len()));
            for (k, tap) in mine.iter().enumerate() {
                debug_assert_eq!(tap.source, mine[0].source + k);
                weights.push(tap.alpha);
            }
        }
        Self {
            size,
            taps,
            columns,
            weights,
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Resizes view `view` (0 = left, 1 = right) of a stereo strip into
    /// `out` (size * size bytes).
    pub fn view(&self, strip: &[u8], view: usize, out: &mut [u8]) {
        assert_eq!(strip.len(), FRAME_BYTES);
        self.view_of(strip, STRIP_WIDTH, view, out);
    }

    /// Resizes the `view`th 400 px view of a strip `width` pixels wide,
    /// such as one of the five cameras of a 2000 px strip, into `out`.
    ///
    /// The same sums as OpenCV in the same order, so the same bytes: each
    /// source row's horizontal pass once, then each destination row as the
    /// weighted sum of its source rows' passes.
    pub fn view_of(&self, strip: &[u8], width: usize, view: usize, out: &mut [u8]) {
        assert_eq!(strip.len(), width * VIEW);
        assert!((view + 1) * VIEW <= width);
        assert_eq!(out.len(), self.size * self.size);
        let size = self.size;
        thread_local! {
            static ROWS: std::cell::RefCell<Vec<f32>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        ROWS.with(|rows| {
            let mut rows = rows.borrow_mut();
            rows.resize(VIEW * size, 0.0);
            for (sy, row) in rows.chunks_exact_mut(size).enumerate() {
                let source = &strip[sy * width + view * VIEW..][..VIEW];
                for (target, &(first, count, offset)) in row.iter_mut().zip(&self.columns) {
                    let mut total = 0f32;
                    for (&pixel, &alpha) in source[first..first + count]
                        .iter()
                        .zip(&self.weights[offset..offset + count])
                    {
                        total += pixel as f32 * alpha;
                    }
                    *target = total;
                }
            }
            let mut sum = vec![0f32; size];
            let mut current = None;
            let flush = |dy: usize, sum: &[f32], out: &mut [u8]| {
                for (target, value) in out[dy * size..(dy + 1) * size].iter_mut().zip(sum) {
                    *target = value.round_ties_even().clamp(0.0, 255.0) as u8;
                }
            };
            for tap in &self.taps {
                let row = &rows[tap.source * size..][..size];
                let beta = tap.alpha;
                if current != Some(tap.destination) {
                    if let Some(dy) = current {
                        flush(dy, &sum, out);
                    }
                    for (total, value) in sum.iter_mut().zip(row) {
                        *total = beta * value;
                    }
                    current = Some(tap.destination);
                } else {
                    for (total, value) in sum.iter_mut().zip(row) {
                        *total += beta * value;
                    }
                }
            }
            if let Some(dy) = current {
                flush(dy, &sum, out);
            }
        });
    }

    /// Both views of a strip as `[2, size, size]` floats in 0..1.
    pub fn stereo(&self, strip: &[u8], out: &mut Vec<f32>) {
        let mut pixels = vec![0u8; self.size * self.size];
        out.clear();
        for view in 0..2 {
            self.view(strip, view, &mut pixels);
            out.extend(pixels.iter().map(|&value| value as f32 / 255.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resize as written before: a horizontal pass per tap.
    fn reference(resize: &AreaResize, strip: &[u8], width: usize, view: usize) -> Vec<u8> {
        let size = resize.size;
        let mut out = vec![0u8; size * size];
        let mut row = vec![0f32; size];
        let mut sum = vec![0f32; size];
        let mut current = None;
        let mut flush = |dy: usize, sum: &[f32]| {
            for (target, value) in out[dy * size..(dy + 1) * size].iter_mut().zip(sum) {
                *target = value.round_ties_even().clamp(0.0, 255.0) as u8;
            }
        };
        for tap in &resize.taps {
            let source = &strip[tap.source * width + view * VIEW..][..VIEW];
            row.fill(0.0);
            for x in &resize.taps {
                row[x.destination] += source[x.source] as f32 * x.alpha;
            }
            if current != Some(tap.destination) {
                if let Some(dy) = current {
                    flush(dy, &sum);
                }
                for (total, value) in sum.iter_mut().zip(&row) {
                    *total = tap.alpha * value;
                }
                current = Some(tap.destination);
            } else {
                for (total, value) in sum.iter_mut().zip(&row) {
                    *total += tap.alpha * value;
                }
            }
        }
        if let Some(dy) = current {
            flush(dy, &sum);
        }
        out
    }

    #[test]
    fn the_resize_gives_the_same_bytes_as_before() {
        let mut seed = 1u64;
        let strip: Vec<u8> = (0..2000 * 400)
            .map(|_| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (seed >> 56) as u8
            })
            .collect();
        for size in [64, 96, 128, 192, 224] {
            let resize = AreaResize::new(size);
            for view in [0, 2, 4] {
                let mut out = vec![0u8; size * size];
                resize.view_of(&strip, 2000, view, &mut out);
                assert_eq!(
                    out,
                    reference(&resize, &strip, 2000, view),
                    "{size} px, view {view}"
                );
            }
        }
    }

    #[test]
    fn area_weights_cover_each_destination_pixel_once() {
        for size in [192, 224, 400] {
            let taps = area_taps(VIEW, size);
            let mut totals = vec![0f64; size];
            for tap in &taps {
                totals[tap.destination] += tap.alpha as f64;
            }
            assert!(
                totals.iter().all(|total| (total - 1.0).abs() < 1e-5),
                "{size}"
            );
        }
    }

    #[test]
    fn a_flat_view_stays_flat() {
        let mut strip = vec![0u8; FRAME_BYTES];
        for row in strip.chunks_mut(STRIP_WIDTH) {
            row[..VIEW].fill(77);
            row[VIEW..].fill(200);
        }
        let resize = AreaResize::new(224);
        let mut out = vec![0u8; 224 * 224];
        resize.view(&strip, 0, &mut out);
        assert!(out.iter().all(|&value| value == 77));
        resize.view(&strip, 1, &mut out);
        assert!(out.iter().all(|&value| value == 200));
    }
}
