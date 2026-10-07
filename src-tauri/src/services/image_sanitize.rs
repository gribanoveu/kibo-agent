//! Sanitizing a picture the user attached, before any of it reaches a model:
//! content disarm and reconstruction. The file is decoded to pixels, and a new
//! one is encoded from them by us — so nothing of the original but its pixels
//! survives: no EXIF (GPS, camera, a thumbnail of the uncropped shot), no XMP
//! or IPTC, no ICC profile, no text chunks or comments, nothing appended after
//! the image's end. See `docs/30-vision.md`, "Санация (CDR)".
//!
//! PNG and JPEG only, decoded by `image`'s pure-Rust codecs — the crate is
//! built with those two and no others.

use std::io::Cursor;

use base64::Engine as _;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, RgbImage};

use crate::domain::image::{ImageError, ImageMediaType, ImagePart};

/// The sizes sanitizing holds a picture to. A parameter rather than constants
/// so a test can exercise each with a picture of a few kilobytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bounds {
    /// Refused before anything is parsed. DeepSeek's own limit per image.
    max_input_bytes: usize,
    /// Refused once the header is read, before the pixels are. DeepSeek's.
    max_side: u32,
    /// Longer than this is scaled down to it. Anthropic's recommended size;
    /// DeepSeek scales to about 1300 on its side regardless.
    target_side: u32,
    /// A PNG over this goes out as JPEG instead. Anthropic's limit per image.
    max_output_bytes: usize,
}

const BOUNDS: Bounds = Bounds {
    max_input_bytes: 32 * 1024 * 1024,
    max_side: 8192,
    target_side: 1568,
    max_output_bytes: 5 * 1024 * 1024,
};

/// What a JPEG is written at — the source's own format for a photo, or the
/// fallback for a PNG too large.
const JPEG_QUALITY: u8 = 85;

/// The picture, sanitized: the only way an `ImagePart` is made.
pub fn sanitize(bytes: &[u8]) -> Result<ImagePart, ImageError> {
    sanitize_within(bytes, &BOUNDS)
}

fn sanitize_within(bytes: &[u8], bounds: &Bounds) -> Result<ImagePart, ImageError> {
    if bytes.len() > bounds.max_input_bytes {
        return Err(ImageError::TooLarge { bytes: bytes.len(), limit: bounds.max_input_bytes });
    }
    // By content, never by a name or a clipboard's MIME type.
    let format = match image::guess_format(bytes) {
        Ok(format @ (ImageFormat::Png | ImageFormat::Jpeg)) => format,
        Ok(other) => return Err(ImageError::UnsupportedFormat { detected: Some(format!("{other:?}")) }),
        Err(_) => return Err(ImageError::UnsupportedFormat { detected: None }),
    };

    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(Limits::default());
    let mut decoder = reader.into_decoder().map_err(|e| ImageError::Decode(e.to_string()))?;
    let (width, height) = decoder.dimensions();
    if width > bounds.max_side || height > bounds.max_side {
        return Err(ImageError::DimensionsTooLarge { width, height, limit: bounds.max_side });
    }
    // Read before the EXIF it lives in is dropped, or a phone's photo arrives
    // on its side. A damaged EXIF costs the rotation, not the picture: the
    // rest of that EXIF is being thrown away anyway.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    // An APNG's first frame is its default image, and that is all this reads.
    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| ImageError::Decode(e.to_string()))?;
    image.apply_orientation(orientation);

    if image.width().max(image.height()) > bounds.target_side {
        image = image.resize(bounds.target_side, bounds.target_side, FilterType::Lanczos3);
    }
    let image = eight_bit(image);

    let (media_type, encoded) = match format {
        ImageFormat::Jpeg => (ImageMediaType::Jpeg, encode_jpeg(&image.to_rgb8())?),
        _ => {
            let png = encode_png(&image)?;
            if png.len() > bounds.max_output_bytes {
                (ImageMediaType::Jpeg, encode_jpeg(&over_white(&image))?)
            } else {
                (ImageMediaType::Png, png)
            }
        }
    };
    if encoded.len() > bounds.max_output_bytes {
        return Err(ImageError::TooLarge { bytes: encoded.len(), limit: bounds.max_output_bytes });
    }
    Ok(ImagePart {
        media_type,
        data: base64::engine::general_purpose::STANDARD.encode(&encoded),
        width: image.width(),
        height: image.height(),
    })
}

/// 8 bits a channel, RGB — RGBA only while some pixel is not opaque. Grey,
/// 16-bit and palette pictures all come out as one of the two.
fn eight_bit(image: DynamicImage) -> DynamicImage {
    if !image.color().has_alpha() {
        return DynamicImage::ImageRgb8(image.into_rgb8());
    }
    let rgba = image.into_rgba8();
    if rgba.pixels().all(|p| p.0[3] == u8::MAX) {
        DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(rgba).into_rgb8())
    } else {
        DynamicImage::ImageRgba8(rgba)
    }
}

/// Transparency flattened onto white, for a JPEG, which has none.
fn over_white(image: &DynamicImage) -> RgbImage {
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let blend = |c: u8| ((u32::from(c) * u32::from(a) + 255 * u32::from(u8::MAX - a)) / 255) as u8;
        image::Rgb([blend(r), blend(g), blend(b)])
    })
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::new();
    image.write_with_encoder(PngEncoder::new(&mut out)).map_err(|e| ImageError::Encode(e.to_string()))?;
    Ok(out)
}

fn encode_jpeg(image: &RgbImage) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::new();
    image
        .write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))
        .map_err(|e| ImageError::Encode(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage};

    const SMALL: Bounds = Bounds { max_input_bytes: 1 << 20, max_side: 512, target_side: 100, max_output_bytes: 1 << 20 };

    /// The bytes that went out, decoded back from base64.
    fn bytes_of(part: &ImagePart) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD.decode(&part.data).expect("base64")
    }

    fn decoded(part: &ImagePart) -> DynamicImage {
        image::load_from_memory(&bytes_of(part)).expect("decodes")
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// Distinct halves, so a rotation is visible in the pixels and not only in
    /// the dimensions: red on the left, blue on the right.
    fn halves(width: u32, height: u32) -> RgbImage {
        RgbImage::from_fn(width, height, |x, _| if x < width / 2 { Rgb([255, 0, 0]) } else { Rgb([0, 0, 255]) })
    }

    /// A big-endian TIFF block: Orientation, Make, and a GPS IFD holding a
    /// latitude reference — enough for "the GPS went out" to be checkable.
    fn exif(orientation: u16) -> Vec<u8> {
        let mut t: Vec<u8> = b"MM\0\x2a\0\0\0\x08".to_vec();
        let entry = |t: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: [u8; 4]| {
            t.extend(tag.to_be_bytes());
            t.extend(kind.to_be_bytes());
            t.extend(count.to_be_bytes());
            t.extend(value);
        };
        // IFD0 at 8: three entries (2 + 3·12 + 4 = 42 bytes), so data from 50.
        t.extend(3u16.to_be_bytes());
        entry(&mut t, 0x010f, 2, 12, 50u32.to_be_bytes()); // Make, ASCII, at 50
        let [o0, o1] = orientation.to_be_bytes();
        entry(&mut t, 0x0112, 3, 1, [o0, o1, 0, 0]); // Orientation, SHORT
        entry(&mut t, 0x8825, 4, 1, 62u32.to_be_bytes()); // GPS IFD at 62
        t.extend(0u32.to_be_bytes());
        t.extend(b"SECRETCAMERA"); // 50..62
        t.extend(1u16.to_be_bytes()); // GPS IFD: one entry
        entry(&mut t, 0x0001, 2, 2, *b"N\0\0\0"); // GPSLatitudeRef
        t.extend(0u32.to_be_bytes());
        t
    }

    fn jpeg_with_metadata(image: &RgbImage, orientation: u16) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut out, 90);
        encoder.set_exif_metadata(exif(orientation)).expect("exif");
        encoder.set_icc_profile(b"SECRETPROFILE".repeat(16)).expect("icc");
        encoder.write_image(image.as_raw(), image.width(), image.height(), image::ExtendedColorType::Rgb8).expect("jpeg");
        out
    }

    fn png_with_text(image: &RgbImage) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.add_text_chunk("Comment".into(), "SECRETTEXT".into()).expect("text");
        encoder.add_ztxt_chunk("Author".into(), "SECRETZTEXT".into()).expect("ztxt");
        let mut writer = encoder.write_header().expect("header");
        writer.write_image_data(image.as_raw()).expect("data");
        writer.finish().expect("finish");
        out
    }

    fn png_of(image: DynamicImage) -> Vec<u8> {
        encode_png(&image).expect("png")
    }

    /// Deterministic noise: a PNG of it does not compress.
    fn noise(width: u32, height: u32) -> RgbaImage {
        let mut seed: u32 = 0x9e37_79b9;
        RgbaImage::from_fn(width, height, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let [r, g, b, _] = seed.to_le_bytes();
            Rgba([r, g, b, 0])
        })
    }

    #[test]
    fn the_real_bounds_are_the_providers_limits() {
        assert_eq!(BOUNDS.max_input_bytes, 32 * 1024 * 1024);
        assert_eq!(BOUNDS.max_side, 8192);
        assert_eq!(BOUNDS.target_side, 1568);
        assert_eq!(BOUNDS.max_output_bytes, 5 * 1024 * 1024);
    }

    /// A photo's metadata — camera, GPS, colour profile — and anything glued
    /// after its end do not go out; its orientation does, in the pixels.
    #[test]
    fn a_photo_loses_its_metadata_and_keeps_its_orientation() {
        let mut file = jpeg_with_metadata(&halves(40, 20), 6);
        file.extend(b"PK\x03\x04SECRETZIP");
        let original = image::load_from_memory(&file).expect("fixture decodes");
        assert!(contains(&file, b"SECRETCAMERA") && contains(&file, b"SECRETPROFILE"), "fixture carries both");

        let part = sanitize_within(&file, &SMALL).expect("sanitizes");
        let out = bytes_of(&part);
        assert_eq!(part.media_type, ImageMediaType::Jpeg);
        for leak in [&b"Exif"[..], b"SECRETCAMERA", b"ICC_PROFILE", b"SECRETPROFILE", b"SECRETZIP", b"PK\x03\x04"] {
            assert!(!contains(&out, leak), "{} went out", String::from_utf8_lossy(leak));
        }
        assert!(out.ends_with(&[0xff, 0xd9]), "nothing after the end of the image");

        // 6 is "rotate 90° clockwise": 40×20 stands up as 20×40, and the red
        // left half is now on top.
        assert_eq!((original.width(), original.height()), (40, 20), "the fixture itself is not rotated");
        assert_eq!((part.width, part.height), (20, 40));
        let pixels = decoded(&part).to_rgb8();
        assert_eq!((pixels.width(), pixels.height()), (20, 40));
        let top = pixels.get_pixel(10, 5).0;
        let bottom = pixels.get_pixel(10, 35).0;
        assert!(top[0] > 200 && top[2] < 60, "top is red: {top:?}");
        assert!(bottom[2] > 200 && bottom[0] < 60, "bottom is blue: {bottom:?}");
    }

    #[test]
    fn a_png_loses_its_text_and_its_tail() {
        let mut file = png_with_text(&halves(30, 30));
        file.extend(b"PK\x03\x04SECRETZIP");
        assert!(contains(&file, b"SECRETTEXT"), "fixture carries it");

        let part = sanitize_within(&file, &SMALL).expect("sanitizes");
        let out = bytes_of(&part);
        assert_eq!(part.media_type, ImageMediaType::Png);
        for leak in [&b"tEXt"[..], b"zTXt", b"SECRETTEXT", b"SECRETZIP"] {
            assert!(!contains(&out, leak), "{} went out", String::from_utf8_lossy(leak));
        }
        assert!(out.ends_with(b"IEND\xae\x42\x60\x82"), "nothing after IEND");
        assert_eq!(decoded(&part).to_rgb8(), halves(30, 30), "the pixels are untouched");
    }

    /// A PNG can carry EXIF too (`eXIf`), and its orientation counts the same.
    #[test]
    fn a_png_with_exif_is_turned_and_stripped() {
        let mut out = Vec::new();
        let image = halves(40, 20);
        let mut encoder = PngEncoder::new(&mut out);
        encoder.set_exif_metadata(exif(6)).expect("exif");
        encoder.write_image(image.as_raw(), 40, 20, image::ExtendedColorType::Rgb8).expect("png");

        let part = sanitize_within(&out, &SMALL).expect("sanitizes");
        assert_eq!((part.width, part.height), (20, 40));
        assert!(!contains(&bytes_of(&part), b"eXIf"));
        assert!(!contains(&bytes_of(&part), b"SECRETCAMERA"));
    }

    /// The header claims more than the limit: refused before any pixel is
    /// allocated — the fixture's one IDAT holds a few bytes, not 30 GB.
    #[test]
    fn a_decompression_bomb_is_refused_by_its_header() {
        let mut file = Vec::new();
        let mut writer = png::Encoder::new(&mut file, 100_000, 100_000).write_header().expect("header");
        writer.write_chunk(png::chunk::IDAT, &[0x78, 0x9c, 0x03, 0x00]).expect("idat");
        drop(writer);
        let err = sanitize(&file).expect_err("refused");
        assert_eq!(err, ImageError::DimensionsTooLarge { width: 100_000, height: 100_000, limit: 8192 });
    }

    #[test]
    fn only_png_and_jpeg_are_taken() {
        let heic = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic".to_vec();
        let cases: [(&[u8], Option<&str>); 6] = [
            (b"GIF89a\x01\0\x01\0\0\0\0;", Some("Gif")),
            (b"RIFF\x1a\0\0\0WEBPVP8L\x0d\0\0\0", Some("WebP")),
            (b"II*\0\x08\0\0\0", Some("Tiff")),
            (b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>", None),
            (&heic, None),
            (b"", None),
        ];
        for (file, detected) in cases {
            let err = sanitize_within(file, &SMALL).expect_err("refused");
            assert_eq!(err, ImageError::UnsupportedFormat { detected: detected.map(String::from) });
        }
    }

    /// The name means nothing: a PNG is a PNG whatever is around it.
    #[test]
    fn a_format_is_known_by_its_content() {
        let part = sanitize_within(&png_of(DynamicImage::ImageRgb8(halves(8, 8))), &SMALL).expect("png");
        assert_eq!(part.media_type, ImageMediaType::Png);
        let jpeg = jpeg_with_metadata(&halves(8, 8), 1);
        assert_eq!(sanitize_within(&jpeg, &SMALL).expect("jpeg").media_type, ImageMediaType::Jpeg);
    }

    /// Over the input limit is refused before parsing: zeros that would
    /// otherwise be "unsupported" say "too large".
    #[test]
    fn too_many_bytes_are_refused_before_parsing() {
        let file = vec![0u8; SMALL.max_input_bytes + 1];
        assert_eq!(
            sanitize_within(&file, &SMALL),
            Err(ImageError::TooLarge { bytes: SMALL.max_input_bytes + 1, limit: SMALL.max_input_bytes })
        );
        let at_limit = vec![0u8; SMALL.max_input_bytes];
        assert!(matches!(sanitize_within(&at_limit, &SMALL), Err(ImageError::UnsupportedFormat { .. })));
    }

    #[test]
    fn a_large_picture_is_scaled_down_and_a_small_one_is_not_scaled_up() {
        let big = png_of(DynamicImage::ImageRgb8(halves(400, 300)));
        let part = sanitize_within(&big, &SMALL).expect("sanitizes");
        assert_eq!((part.width, part.height), (100, 75));
        assert_eq!(decoded(&part).dimensions(), (100, 75));
        // The longer side decides: a strip only its width is over is scaled too.
        let strip = png_of(DynamicImage::ImageRgb8(halves(400, 40)));
        assert_eq!(sanitize_within(&strip, &SMALL).map(|p| (p.width, p.height)), Ok((100, 10)));

        let exact = png_of(DynamicImage::ImageRgb8(halves(100, 60)));
        assert_eq!(sanitize_within(&exact, &SMALL).map(|p| (p.width, p.height)), Ok((100, 60)));
        let small = png_of(DynamicImage::ImageRgb8(halves(80, 60)));
        assert_eq!(sanitize_within(&small, &SMALL).map(|p| (p.width, p.height)), Ok((80, 60)));
    }

    #[test]
    fn a_side_at_the_limit_is_taken() {
        let edge = png_of(DynamicImage::ImageRgb8(halves(SMALL.max_side, 4)));
        assert!(sanitize_within(&edge, &SMALL).is_ok());
        let over = png_of(DynamicImage::ImageRgb8(halves(4, SMALL.max_side + 1)));
        assert_eq!(
            sanitize_within(&over, &SMALL),
            Err(ImageError::DimensionsTooLarge { width: 4, height: SMALL.max_side + 1, limit: SMALL.max_side })
        );
    }

    #[test]
    fn an_alpha_channel_is_kept_only_when_used() {
        let opaque = RgbaImage::from_pixel(6, 6, Rgba([10, 20, 30, 255]));
        let part = sanitize_within(&png_of(DynamicImage::ImageRgba8(opaque)), &SMALL).expect("opaque");
        assert_eq!(decoded(&part).color(), image::ColorType::Rgb8);

        let mut clear = RgbaImage::from_pixel(6, 6, Rgba([10, 20, 30, 255]));
        clear.put_pixel(0, 0, Rgba([0, 0, 0, 254]));
        let part = sanitize_within(&png_of(DynamicImage::ImageRgba8(clear)), &SMALL).expect("translucent");
        assert_eq!(decoded(&part).color(), image::ColorType::Rgba8);
    }

    #[test]
    fn sixteen_bits_and_grey_become_eight_bit_rgb() {
        let grey16 = image::ImageBuffer::<image::Luma<u16>, _>::from_pixel(5, 5, image::Luma([40_000u16]));
        let part = sanitize_within(&png_of(DynamicImage::ImageLuma16(grey16)), &SMALL).expect("sanitizes");
        assert_eq!(decoded(&part).color(), image::ColorType::Rgb8);
    }

    /// An animated PNG goes out as its first frame, and not as an animation.
    #[test]
    fn an_animated_png_becomes_its_first_frame() {
        let mut file = Vec::new();
        let mut encoder = png::Encoder::new(&mut file, 4, 4);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(2, 0).expect("animated");
        let mut writer = encoder.write_header().expect("header");
        writer.write_image_data(&[255, 0, 0].repeat(16)).expect("frame 1");
        writer.write_image_data(&[0, 0, 255].repeat(16)).expect("frame 2");
        writer.finish().expect("finish");
        assert!(contains(&file, b"acTL"), "the fixture is animated");

        let part = sanitize_within(&file, &SMALL).expect("sanitizes");
        assert!(!contains(&bytes_of(&part), b"acTL") && !contains(&bytes_of(&part), b"fcTL"));
        assert_eq!(decoded(&part).to_rgb8().get_pixel(1, 1).0, [255, 0, 0]);
    }

    /// A PNG over the output limit goes out as JPEG, its transparency on white.
    #[test]
    fn a_png_too_large_goes_out_as_jpeg_on_white() {
        let bounds = Bounds { max_output_bytes: 20_000, ..SMALL };
        let file = png_of(DynamicImage::ImageRgba8(noise(90, 90)));
        assert!(file.len() > bounds.max_output_bytes, "the fixture is over the limit as PNG");

        let part = sanitize_within(&file, &bounds).expect("sanitizes");
        assert_eq!(part.media_type, ImageMediaType::Jpeg);
        assert!(bytes_of(&part).len() <= bounds.max_output_bytes);
        let pixel = decoded(&part).to_rgb8().get_pixel(45, 45).0;
        assert!(pixel.iter().all(|&c| c > 235), "fully transparent comes out white: {pixel:?}");
    }

    /// Nothing fits: refused rather than sent over the provider's limit.
    #[test]
    fn a_picture_that_fits_no_format_is_refused() {
        let bounds = Bounds { max_output_bytes: 200, ..SMALL };
        let file = png_of(DynamicImage::ImageRgba8(noise(90, 90)));
        assert!(matches!(sanitize_within(&file, &bounds), Err(ImageError::TooLarge { limit: 200, .. })));
    }

    #[test]
    fn white_is_what_transparency_becomes() {
        let mut image = RgbaImage::from_pixel(2, 1, Rgba([0, 0, 0, 0]));
        image.put_pixel(1, 0, Rgba([0, 0, 0, 255]));
        let flat = over_white(&DynamicImage::ImageRgba8(image));
        assert_eq!(flat.get_pixel(0, 0).0, [255, 255, 255]);
        assert_eq!(flat.get_pixel(1, 0).0, [0, 0, 0]);
    }
}
