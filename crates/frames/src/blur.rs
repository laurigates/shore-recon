//! Blur metric: variance of the Laplacian of a JPEG's luma, matching OpenCV's
//! `cv2.Laplacian(cv2.imread(path, cv2.IMREAD_GRAYSCALE), cv2.CV_64F).var()`,
//! the measurement `blur_threshold` was calibrated on in M0 (ADR-0005), for
//! the YCbCr and grayscale JPEGs ffmpeg writes. CMYK and Adobe RGB-transform
//! JPEGs are not decoded as OpenCV does; photo input (#23) needs them.

use std::path::Path;

use shoreline_core::Result;
use shoreline_core::error::io_error;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// An 8-bit luma plane, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Luma {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// Decode the JPEG's Y channel directly, without a colour round trip; libjpeg
/// does the same for OpenCV's grayscale read.
pub fn decode_luma(path: &Path) -> Result<Luma> {
    let data = std::fs::read(path).map_err(io_error(path))?;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::Luma);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(&data), options);
    let invalid = |why: String| crate::stage_error(format!("{}: {why}", path.display()));
    let pixels = decoder.decode().map_err(|e| invalid(e.to_string()))?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| invalid("no dimensions after decoding".into()))?;
    if pixels.len() != width * height {
        return Err(invalid(format!(
            "decoded {} bytes for a {width}x{height} luma plane",
            pixels.len()
        )));
    }
    Ok(Luma {
        width,
        height,
        pixels,
    })
}

/// Population variance of the 4-neighbour Laplacian (`[0 1 0; 1 -4 1; 0 1 0]`,
/// OpenCV's `ksize = 1`) over every pixel, with reflect-101 borders
/// (OpenCV's default: `-1 → 1`, `n → n - 2`).
pub fn laplacian_variance(luma: &Luma) -> f64 {
    let (w, h) = (luma.width, luma.height);
    if w == 0 || h == 0 {
        return 0.0;
    }
    let px = |x: usize, y: usize| f64::from(luma.pixels[y * w + x]);
    let (mut sum, mut sum_sq) = (0.0, 0.0);
    for y in 0..h {
        let (up, down) = (
            reflect_101(y as isize - 1, h),
            reflect_101(y as isize + 1, h),
        );
        for x in 0..w {
            let (left, right) = (
                reflect_101(x as isize - 1, w),
                reflect_101(x as isize + 1, w),
            );
            let v = px(left, y) + px(right, y) + px(x, up) + px(x, down) - 4.0 * px(x, y);
            sum += v;
            sum_sq += v * v;
        }
    }
    let n = (w * h) as f64;
    let mean = sum / n;
    sum_sq / n - mean * mean
}

/// OpenCV's `BORDER_REFLECT_101` for an index at most one step outside `0..n`.
fn reflect_101(i: isize, n: usize) -> usize {
    let last = n as isize - 1;
    if n == 1 {
        0
    } else if i < 0 {
        (-i) as usize
    } else if i > last {
        (2 * last - i) as usize
    } else {
        i as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Allowed difference from OpenCV: relative, plus an absolute floor for
    /// nearly flat images, where a few ±1 differences between the two JPEG
    /// decoders' IDCTs dominate a tiny variance (`blurry.jpg`: 0.0065 at 2.25).
    /// Near the default threshold of 100 this allows about 0.12. Measured
    /// worst case is a third of this; M0's 261 real frames agreed within
    /// 1.7e-4 relative.
    const REL_TOLERANCE: f64 = 1e-3;
    const ABS_TOLERANCE: f64 = 0.02;

    fn allowed(want: f64) -> f64 {
        REL_TOLERANCE * want + ABS_TOLERANCE
    }

    fn fixtures() -> Vec<(std::path::PathBuf, f64)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/frames");
        let csv = std::fs::read_to_string(dir.join("expected.csv")).unwrap();
        csv.lines()
            .skip(1)
            .map(|line| {
                let (name, var) = line.split_once(',').unwrap();
                (dir.join(name), var.parse().unwrap())
            })
            .collect()
    }

    /// Error as a multiple of the allowed difference: ≤ 1 passes.
    fn excess(got: f64, want: f64) -> f64 {
        (got - want).abs() / allowed(want)
    }

    fn luma(width: usize, height: usize, pixels: &[u8]) -> Luma {
        Luma {
            width,
            height,
            pixels: pixels.to_vec(),
        }
    }

    #[test]
    fn matches_opencv_on_the_fixtures() {
        let fixtures = fixtures();
        assert!(fixtures.len() >= 6, "fixture set shrank: {fixtures:?}");
        for (path, want) in fixtures {
            let got = laplacian_variance(&decode_luma(&path).unwrap());
            assert!(
                excess(got, want) <= 1.0,
                "{}: got {got}, OpenCV {want}",
                path.display()
            );
        }
    }

    #[test]
    fn keep_or_drop_at_the_default_threshold_agrees_with_opencv() {
        for (path, want) in fixtures() {
            let got = laplacian_variance(&decode_luma(&path).unwrap());
            assert_eq!(got >= 100.0, want >= 100.0, "{}", path.display());
        }
    }

    #[test]
    fn decodes_dimensions() {
        let (path, _) = fixtures()
            .into_iter()
            .find(|(p, _)| p.ends_with("odd.jpg"))
            .unwrap();
        let l = decode_luma(&path).unwrap();
        assert_eq!((l.width, l.height, l.pixels.len()), (37, 23, 37 * 23));
    }

    #[test]
    fn non_jpeg_input_is_an_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.jpg");
        std::fs::write(&path, b"not a jpeg").unwrap();
        let err = decode_luma(&path).unwrap_err();
        assert!(err.to_string().contains("broken.jpg"), "{err}");
    }

    #[test]
    fn flat_image_has_zero_variance() {
        assert_eq!(laplacian_variance(&luma(4, 3, &[77; 12])), 0.0);
    }

    #[test]
    fn single_bright_pixel_by_hand() {
        // 3×3, centre 4, rest 0. Reflect-101 maps an edge pixel's missing
        // neighbour to the pixel two steps in, which for the edge middles is
        // the centre: they see it twice (8). Corners see only zeros.
        // Values [0,8,0, 8,-16,8, 0,8,0]: mean 16/9,
        // var = (4·64 + 256)/9 - (16/9)² = 4352/81.
        let got = laplacian_variance(&luma(3, 3, &[0, 0, 0, 0, 4, 0, 0, 0, 0]));
        assert!((got - 4352.0 / 81.0).abs() < 1e-12, "{got}");
    }

    #[test]
    fn reflect_101_borders_by_hand() {
        // 1-D row [0, 10, 0, 0] (height 1: vertical neighbours reflect onto
        // the row itself, contributing 2·p - 2·p = 0). Horizontal, reflect-101:
        // x=0: p[1]+p[1]-2·p[0] = 20; x=1: 0+0-20 = -20; x=2: 10+0 = 10;
        // x=3: p[2]+p[2] = 0. Mean 2.5, var = (17.5²+22.5²+7.5²+2.5²)/4.
        let got = laplacian_variance(&luma(4, 1, &[0, 10, 0, 0]));
        let want = (17.5f64.powi(2) + 22.5f64.powi(2) + 7.5f64.powi(2) + 2.5f64.powi(2)) / 4.0;
        assert!((got - want).abs() < 1e-12, "{got} vs {want}");
    }

    // The fixtures must be able to tell a wrong implementation from a right
    // one (rules: validate adversarial constructions). These variants are the
    // two plausible mistakes; on some fixture each must miss OpenCV by more
    // than three times the allowed difference.

    fn variance(values: impl Iterator<Item = f64>) -> f64 {
        let v: Vec<f64> = values.collect();
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / v.len() as f64
    }

    fn replicate_border_variance(l: &Luma) -> f64 {
        let at = |x: isize, y: isize| {
            let x = x.clamp(0, l.width as isize - 1) as usize;
            let y = y.clamp(0, l.height as isize - 1) as usize;
            f64::from(l.pixels[y * l.width + x])
        };
        variance((0..l.height as isize).flat_map(|y| {
            (0..l.width as isize).map(move |x| {
                at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1) - 4.0 * at(x, y)
            })
        }))
    }

    fn rec709_luma(path: &Path) -> Luma {
        let data = std::fs::read(path).unwrap();
        let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
        let mut d = JpegDecoder::new_with_options(ZCursor::new(&data), options);
        let rgb = d.decode().unwrap();
        let (width, height) = d.dimensions().unwrap();
        let pixels = rgb
            .chunks(3)
            .map(|p| {
                let y =
                    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]);
                y.round() as u8
            })
            .collect();
        Luma {
            width,
            height,
            pixels,
        }
    }

    #[test]
    fn fixtures_reject_replicate_borders() {
        let worst = fixtures()
            .into_iter()
            .map(|(path, want)| {
                excess(
                    replicate_border_variance(&decode_luma(&path).unwrap()),
                    want,
                )
            })
            .fold(0.0, f64::max);
        assert!(
            worst > 3.0,
            "replicate borders within {worst}x of the allowance"
        );
    }

    #[test]
    fn fixtures_reject_rec709_luma() {
        let worst = fixtures()
            .into_iter()
            .map(|(path, want)| excess(laplacian_variance(&rec709_luma(&path)), want))
            .fold(0.0, f64::max);
        assert!(
            worst > 3.0,
            "Rec. 709 luma within {worst}x of the allowance"
        );
    }
}
