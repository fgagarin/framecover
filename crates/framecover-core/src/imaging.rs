//! Image helpers matching the Pillow behaviour of the Python version.

use std::io::Cursor;

use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, RgbImage, codecs::jpeg::JpegEncoder};

use crate::error::{Error, Result};

/// Maximum cover-art size (fit inside, aspect preserved).
pub const MAX_THUMB_W: u32 = 640;
pub const MAX_THUMB_H: u32 = 360;
pub const JPEG_QUALITY: u8 = 90;

/// Size that fits `(w, h)` inside `(max_w, max_h)` preserving aspect ratio,
/// never upscaling — the same rule as `PIL.Image.thumbnail`.
pub fn fit_within(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w <= max_w && h <= max_h {
        return (w, h);
    }
    let aspect = w as f64 / h as f64;
    let (mut nw, mut nh) = (max_w as f64, max_h as f64);
    if nw / nh >= aspect {
        nw = (nh * aspect).round();
    } else {
        nh = (nw / aspect).round();
    }
    ((nw as u32).max(1), (nh as u32).max(1))
}

/// Downscale with Lanczos to fit inside `(max_w, max_h)`; returns a copy.
pub fn thumbnail(img: &RgbImage, max_w: u32, max_h: u32) -> RgbImage {
    let (w, h) = fit_within(img.width(), img.height(), max_w, max_h);
    if (w, h) == img.dimensions() {
        return img.clone();
    }
    image::imageops::resize(img, w, h, FilterType::Lanczos3)
}

/// Encode as baseline JPEG at `quality`.
pub fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality)
        .encode_image(img)
        .map_err(|e| Error::Other(format!("JPEG encoding failed: {e}")))?;
    Ok(buf)
}

/// Encode as PNG.
pub fn encode_png(img: &RgbImage) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| Error::Other(format!("PNG encoding failed: {e}")))?;
    Ok(buf.into_inner())
}

/// Decode any supported image bytes (JPEG/PNG) into RGB.
pub fn decode_rgb(bytes: &[u8]) -> Result<RgbImage> {
    image::load_from_memory(bytes)
        .map(DynamicImage::into_rgb8)
        .map_err(|e| Error::Other(format!("image decoding failed: {e}")))
}

/// Cover art bytes as written by the thumbnail writer: fit 640x360, JPEG q90.
pub fn cover_jpeg(frame: &RgbImage) -> Result<Vec<u8>> {
    encode_jpeg(&thumbnail(frame, MAX_THUMB_W, MAX_THUMB_H), JPEG_QUALITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_matches_pillow() {
        assert_eq!(fit_within(1920, 1080, 640, 360), (640, 360));
        assert_eq!(fit_within(1080, 1920, 640, 360), (203, 360));
        assert_eq!(fit_within(320, 240, 640, 360), (320, 240));
        assert_eq!(fit_within(1000, 400, 640, 360), (640, 256));
    }

    #[test]
    fn jpeg_roundtrip() {
        let img = RgbImage::from_pixel(1280, 720, image::Rgb([10, 200, 30]));
        let bytes = cover_jpeg(&img).unwrap();
        let back = decode_rgb(&bytes).unwrap();
        assert_eq!(back.dimensions(), (640, 360));
    }
}
