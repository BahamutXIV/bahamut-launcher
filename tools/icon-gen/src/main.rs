//! Generate Tauri PNG, ICNS, and ICO files from `icon-source.png`.
//! Run with `cargo run --release --manifest-path tools/icon-gen/Cargo.toml`.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result, bail};
use image::{ImageBuffer, Rgba, RgbaImage, imageops::FilterType};

/// Blue channel level that marks the border against the dark vignette.
const BORDER_BLUE_MIN: u8 = 128;
/// Source pixels kept outside the border so its outer glow is not clipped.
const GLOW_MARGIN: u32 = 2;
/// Mask supersampling factor per axis.
const MASK_SUPERSAMPLE: u32 = 4;
/// macOS icon grid: the body spans 824 of the 1024 canvas.
const MACOS_BODY_NUMERATOR: u32 = 824;
const MACOS_CANVAS_DENOMINATOR: u32 = 1024;
/// Tauri uses the first ICO entry for the window; keep the largest first.
/// https://github.com/tauri-apps/tauri/blob/tauri-codegen-v2.6.2/crates/tauri-codegen/src/image.rs#L57
const ICO_SIZES: &[u32] = &[256, 16, 24, 32, 48, 64];
/// Linux hicolor theme sizes, written under `packaging/linux/icons`.
const LINUX_SIZES: &[u32] = &[48, 128, 256];

fn main() -> Result<()> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project_dir = manifest_dir
        .parent()
        .and_then(Path::parent)
        .context("manifest has no repository root")?;
    let icons = project_dir.join("src-tauri").join("icons");
    let source = icons.join("icon-source.png");
    let out_png = icons.join("icon.png");
    let out_icns = icons.join("icon.icns");
    let out_ico = icons.join("icon.ico");

    if !source.exists() {
        bail!("source image not found: {}", source.display());
    }

    println!("==> Masking icon-source.png");
    let body = load_and_mask(&source)?;
    println!("    body: {}x{}", body.width(), body.height());

    println!("==> Writing {}", out_png.display());
    body.save(&out_png)
        .with_context(|| format!("write {}", out_png.display()))?;

    println!("==> Writing {}", out_icns.display());
    write_icns(&body, &out_icns)?;

    println!("==> Writing {}", out_ico.display());
    write_ico(&body, &out_ico)?;

    let hicolor = project_dir.join("packaging/linux/icons/hicolor");
    for &size in LINUX_SIZES {
        let out = hicolor
            .join(format!("{size}x{size}"))
            .join("apps")
            .join("bahamut-launcher.png");
        println!("==> Writing {}", out.display());
        write_linux_png(&body, size, &out)?;
    }

    println!("done.");
    Ok(())
}

struct Body {
    inset: u32,
    radius: u32,
}

fn measure_body(img: &RgbaImage) -> Result<Body> {
    let (w, h) = img.dimensions();
    if w != h {
        bail!("source image must be square; got {w}x{h}");
    }
    let is_border = |x: u32, y: u32| img.get_pixel(x, y).0[2] >= BORDER_BLUE_MIN;

    let inset = (0..w)
        .find(|&x| is_border(x, h / 2))
        .context("no border found on the middle row")?;
    let diagonal = (0..w)
        .find(|&i| is_border(i, i))
        .context("no border found on the diagonal")?;
    if diagonal <= inset {
        bail!("diagonal hit {diagonal} is not outside the axis inset {inset}");
    }
    // A corner arc of radius r inset by d meets the diagonal at
    // d + r - r / sqrt(2), so r = (hit - d) * 2 / (2 - sqrt(2)).
    let radius =
        ((diagonal - inset) as f64 * 2.0 / (2.0 - std::f64::consts::SQRT_2)).round() as u32;
    if inset < GLOW_MARGIN {
        bail!("border inset {inset} leaves no room for the glow margin");
    }
    Ok(Body { inset, radius })
}

fn load_and_mask(source: &Path) -> Result<RgbaImage> {
    let src = image::open(source).with_context(|| format!("open {}", source.display()))?;
    let mut img = src.to_rgba8();
    let body = measure_body(&img)?;
    println!(
        "    border inset {} px, corner radius {} px",
        body.inset, body.radius
    );

    let side = img.width();
    let d = body.inset - GLOW_MARGIN;
    let r = body.radius + GLOW_MARGIN;
    let coverage = rounded_rect_coverage(side, d, r);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let index = (y * side + x) as usize;
        let alpha = f64::from(px.0[3]) * coverage[index];
        px.0[3] = alpha.round().clamp(0.0, 255.0) as u8;
    }

    let body_side = side - 2 * d;
    Ok(image::imageops::crop_imm(&img, d, d, body_side, body_side).to_image())
}

/// Per-pixel coverage of the rounded rectangle inset by `d` with corner
/// radius `r`, supersampled so the arc edge is anti-aliased.
fn rounded_rect_coverage(side: u32, d: u32, r: u32) -> Vec<f64> {
    let (lo, hi, r) = (f64::from(d), f64::from(side - d), f64::from(r));
    let samples = f64::from(MASK_SUPERSAMPLE * MASK_SUPERSAMPLE);
    let inside = |x: f64, y: f64| {
        if x < lo || x > hi || y < lo || y > hi {
            return false;
        }
        let cx = if x < lo + r {
            lo + r
        } else if x > hi - r {
            hi - r
        } else {
            return true;
        };
        let cy = if y < lo + r {
            lo + r
        } else if y > hi - r {
            hi - r
        } else {
            return true;
        };
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };

    let mut coverage = Vec::with_capacity((side * side) as usize);
    for y in 0..side {
        for x in 0..side {
            let mut hits = 0u32;
            for j in 0..MASK_SUPERSAMPLE {
                for i in 0..MASK_SUPERSAMPLE {
                    let sx = f64::from(x) + (f64::from(i) + 0.5) / f64::from(MASK_SUPERSAMPLE);
                    let sy = f64::from(y) + (f64::from(j) + 0.5) / f64::from(MASK_SUPERSAMPLE);
                    if inside(sx, sy) {
                        hits += 1;
                    }
                }
            }
            coverage.push(f64::from(hits) / samples);
        }
    }
    coverage
}

/// Resample through premultiplied alpha so the vignette colour left under
/// the transparent corners never bleeds into the edge pixels.
fn resize(body: &RgbaImage, size: u32) -> RgbaImage {
    let mut premultiplied = body.clone();
    for px in premultiplied.pixels_mut() {
        let alpha = u32::from(px.0[3]);
        for channel in &mut px.0[..3] {
            *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    let mut resized = image::imageops::resize(&premultiplied, size, size, FilterType::Lanczos3);
    for px in resized.pixels_mut() {
        let alpha = u32::from(px.0[3]);
        for channel in &mut px.0[..3] {
            *channel = (u32::from(*channel) * 255 + alpha / 2)
                .checked_div(alpha)
                .map_or(0, |value| value.min(255) as u8);
        }
    }
    resized
}

/// Place the body on a transparent canvas at the macOS icon-grid proportion.
fn on_macos_grid(body: &RgbaImage, canvas: u32) -> RgbaImage {
    let body_size = (canvas * MACOS_BODY_NUMERATOR).div_ceil(MACOS_CANVAS_DENOMINATOR);
    let offset = i64::from((canvas - body_size) / 2);
    let mut out: RgbaImage = ImageBuffer::from_pixel(canvas, canvas, Rgba([0u8, 0, 0, 0]));
    image::imageops::overlay(&mut out, &resize(body, body_size), offset, offset);
    out
}

fn write_icns(body: &RgbaImage, out: &Path) -> Result<()> {
    use icns::{IconFamily, IconType, Image as IcnsImage, PixelFormat};

    // (canvas pixel size, macOS icon OSType); the `_2x` variants carry the
    // retina image for the logical size in the type.  macOS does not decode
    // PNG payloads in the `icp4`/`icp5` slots (iconutil extracts them as
    // opaque noise), so the 1x 16 and 32 slots use the RGB24 types, which
    // the crate writes as the `is32`/`s8mk` and `il32`/`l8mk` pairs.
    let spec: &[(u32, IconType)] = &[
        (16, IconType::RGB24_16x16),
        (32, IconType::RGBA32_16x16_2x),
        (32, IconType::RGB24_32x32),
        (64, IconType::RGBA32_32x32_2x),
        (128, IconType::RGBA32_128x128),
        (256, IconType::RGBA32_128x128_2x),
        (256, IconType::RGBA32_256x256),
        (512, IconType::RGBA32_256x256_2x),
        (512, IconType::RGBA32_512x512),
        (1024, IconType::RGBA32_512x512_2x),
    ];

    let mut family = IconFamily::new();
    for &(size, icon_type) in spec {
        let canvas = on_macos_grid(body, size);
        let img = IcnsImage::from_data(PixelFormat::RGBA, size, size, canvas.into_raw())?;
        family.add_icon_with_type(&img, icon_type)?;
    }

    let file = File::create(out).with_context(|| format!("create {}", out.display()))?;
    family.write(BufWriter::new(file))?;
    Ok(())
}

fn write_linux_png(body: &RgbaImage, size: u32, out: &Path) -> Result<()> {
    let dir = out.parent().context("icon path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    resize(body, size)
        .save(out)
        .with_context(|| format!("write {}", out.display()))
}

fn write_ico(body: &RgbaImage, out: &Path) -> Result<()> {
    use ico::{IconDir, IconDirEntry, IconImage, ResourceType};

    let mut dir = IconDir::new(ResourceType::Icon);
    for &size in ICO_SIZES {
        let img = IconImage::from_rgba_data(size, size, resize(body, size).into_raw());
        dir.add_entry(IconDirEntry::encode(&img)?);
    }

    let file = File::create(out).with_context(|| format!("create {}", out.display()))?;
    dir.write(BufWriter::new(file))?;
    Ok(())
}
