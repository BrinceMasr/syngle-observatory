//! The PNG of a frame that is asked for again and again (the sidecar's `/render`, Syngle-Ecosystem #634).
//!
//! `Pixmap::encode_png` copies the frame, un-premultiplies every pixel and compresses at the encoder's default level: 17 ms of
//! a 30 ms frame at 1280 x 800 (measured on the build box). A pan, a thumbnail or an agent looking at the map wants its pixels
//! sooner and does not keep the file, so this encoder compresses at the encoder's quickest level with the `Sub` filter (2 ms,
//! about 1.7 times the bytes) and borrows the frame when every pixel is opaque, which a map's frame nearly always is (the water
//! is drawn over everything). The decoded pixels are the same as `encode_png`'s: both un-premultiply the same way.

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use std::borrow::Cow;
use tiny_skia::Pixmap;

/// How hard a PNG is worked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngEffort {
    /// The smallest file the encoder gives by default: an export a person keeps.
    Smallest,
    /// The quickest encode: a frame the engine serves again and again.
    Fastest,
}

/// The frame as straight (not premultiplied) RGBA bytes: borrowed when every pixel is opaque, since premultiplying changes
/// nothing then.
fn straight_rgba(pm: &Pixmap) -> Cow<'_, [u8]> {
    if pm.data().chunks_exact(4).all(|p| p.get(3) == Some(&255)) {
        return Cow::Borrowed(pm.data());
    }
    let mut out = Vec::with_capacity(pm.data().len());
    for pixel in pm.pixels() {
        let c = pixel.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Cow::Owned(out)
}

/// `pm` as PNG bytes at `effort`.
pub fn encode(pm: &Pixmap, effort: PngEffort) -> Result<Vec<u8>, String> {
    match effort {
        PngEffort::Smallest => pm.encode_png().map_err(|e| e.to_string()),
        PngEffort::Fastest => {
            let mut out = Vec::with_capacity(pm.data().len() / 3);
            PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Sub)
                .write_image(&straight_rgba(pm), pm.width(), pm.height(), ExtendedColorType::Rgba8)
                .map_err(|e| e.to_string())?;
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{Color, Paint, PathBuilder, Transform};

    /// A frame with edges, a gradient and (when `alpha` is under 255) see-through pixels.
    fn frame(alpha: u8) -> Pixmap {
        let mut pm = Pixmap::new(97, 61).unwrap();
        pm.fill(Color::from_rgba8(20, 60, 120, alpha));
        let mut paint = Paint::default();
        for i in 0..40u8 {
            paint.set_color_rgba8(i.wrapping_mul(6), 200u8.saturating_sub(i), 90, alpha.saturating_sub(i / 2));
            let mut pb = PathBuilder::new();
            pb.push_circle(5.0 + f32::from(i) * 2.2, 8.0 + f32::from(i) * 1.1, 6.5);
            pm.fill_path(&pb.finish().unwrap(), &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
        }
        pm
    }

    #[test]
    fn the_quick_png_decodes_to_the_pixels_the_default_one_does() {
        for alpha in [255, 200, 7] {
            let pm = frame(alpha);
            let slow = Pixmap::decode_png(&encode(&pm, PngEffort::Smallest).unwrap()).unwrap();
            let quick = Pixmap::decode_png(&encode(&pm, PngEffort::Fastest).unwrap()).unwrap();
            assert_eq!(quick.data(), slow.data(), "alpha {alpha}");
            assert_eq!((quick.width(), quick.height()), (97, 61));
        }
    }

    #[test]
    fn an_opaque_frame_is_borrowed_and_a_see_through_one_is_not() {
        assert!(matches!(straight_rgba(&frame(255)), Cow::Borrowed(_)));
        assert!(matches!(straight_rgba(&frame(120)), Cow::Owned(_)));
    }
}
