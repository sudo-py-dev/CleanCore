//! Program icons for the Uninstaller and Autorun lists: resolved and decoded on a background thread
//! (PNG / ICO / BMP with `image`, SVG with `resvg`, Windows `.exe` / `.dll` icons through
//! the shell), then turned into textures as they arrive.

use crate::icons::{self, IconSource};
use crate::uninstaller::Program;
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};

/// Decoded size; drawn at 20–34 px, so this stays sharp on high-DPI screens.
const SIZE: u32 = 48;

#[derive(Default)]
pub struct IconCache {
    textures: HashMap<String, TextureHandle>,
    receivers: Vec<Receiver<(String, ColorImage)>>,
    /// Ids already requested, so a refresh only loads new programs.
    requested: std::collections::HashSet<String>,
}

impl IconCache {
    /// Start loading the icons of programs not seen before.
    pub fn load(&mut self, ctx: &egui::Context, programs: &[Program]) {
        let todo: Vec<Program> = programs.iter().filter(|p| self.requested.insert(p.id.clone())).cloned().collect();
        self.load_with(ctx, todo, |p| (p.id.clone(), icons::resolve(p)));
    }

    /// The same for autorun entries, by entry id.
    pub fn load_autorun(&mut self, ctx: &egui::Context, entries: &[crate::autorun::AutorunEntry]) {
        let todo: Vec<crate::autorun::AutorunEntry> = entries.iter().filter(|e| self.requested.insert(e.id.clone())).cloned().collect();
        self.load_with(ctx, todo, |e| (e.id.clone(), icons::for_autorun(e)));
    }

    fn load_with<T: Send + Sync + 'static>(&mut self, ctx: &egui::Context, todo: Vec<T>, resolve: fn(&T) -> (String, Option<IconSource>)) {
        if todo.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        self.receivers.push(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // Several programs often share one icon file: decode each source once.
            let sources: Vec<(String, Option<IconSource>)> = todo.par_iter().map(resolve).collect();
            let mut by_source: HashMap<IconSource, Vec<String>> = HashMap::new();
            for (id, src) in sources {
                if let Some(s) = src {
                    by_source.entry(s).or_default().push(id);
                }
            }
            let jobs: Vec<(IconSource, Vec<String>)> = by_source.into_iter().collect();
            jobs.par_iter().for_each(|(src, ids)| {
                if let Some(img) = decode(src) {
                    for id in ids {
                        let _ = tx.send((id.clone(), img.clone()));
                    }
                    ctx.request_repaint();
                }
            });
        });
    }

    /// Turn decoded icons into textures (call once per frame).
    pub fn poll(&mut self, ctx: &egui::Context) {
        // Texture uploads also cost UI time. Spread large batches over several frames.
        let mut budget = 16;
        self.receivers.retain(|rx| loop {
            if budget == 0 {
                break true;
            }
            match rx.try_recv() {
                Ok((id, img)) => {
                    budget -= 1;
                    if self.requested.contains(&id) {
                        let tex = ctx.load_texture(crate::tf!("app-icon:{id}"), img, TextureOptions::LINEAR);
                        self.textures.insert(id, tex);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break false,
            }
        });
        if budget == 0 {
            ctx.request_repaint();
        }
    }

    /// Drop icons for removed entries after a complete scan, including late worker results.
    pub fn retain_ids<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) {
        let ids: std::collections::HashSet<_> = ids.into_iter().collect();
        self.textures.retain(|id, _| ids.contains(id.as_str()));
        self.requested.retain(|id| ids.contains(id.as_str()));
    }

    pub fn get(&self, id: &str) -> Option<&TextureHandle> {
        self.textures.get(id)
    }
}

fn decode(src: &IconSource) -> Option<ColorImage> {
    match src {
        IconSource::File(path) => decode_file(path),
        IconSource::Resource { file, index } => {
            #[cfg(windows)]
            {
                let (w, h, px) = crate::platform::icon_rgba(file, *index)?;
                fit(image::RgbaImage::from_raw(w, h, px)?)
            }
            #[cfg(not(windows))]
            {
                let _ = index;
                decode_file(file)
            }
        }
    }
}

fn decode_file(path: &Path) -> Option<ColorImage> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "svg" || ext == "svgz" {
        return decode_svg(path);
    }
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    fit(img.to_rgba8())
}

/// Scale into a SIZE×SIZE square, keeping the aspect ratio.
fn fit(img: image::RgbaImage) -> Option<ColorImage> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let img = if w > SIZE || h > SIZE {
        let s = SIZE as f32 / w.max(h) as f32;
        image::imageops::resize(&img, ((w as f32 * s).round() as u32).max(1), ((h as f32 * s).round() as u32).max(1), image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let (w, h) = img.dimensions();
    Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw()))
}

fn decode_svg(path: &Path) -> Option<ColorImage> {
    use resvg::{tiny_skia, usvg};
    let data = std::fs::read(path).ok()?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = SIZE as f32 / size.width().max(size.height());
    let mut pm = tiny_skia::Pixmap::new(SIZE, SIZE)?;
    let (dx, dy) = ((SIZE as f32 - size.width() * scale) / 2.0, (SIZE as f32 - size.height() * scale) / 2.0);
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy), &mut pm.as_mut());
    Some(ColorImage::from_rgba_premultiplied([SIZE as usize, SIZE as usize], pm.data()))
}

/// CleanCore's white mark (assets/logo-mark.svg) at `px` pixels square, for the logo tile.
pub fn logo_mark(px: u32) -> Option<ColorImage> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_data(include_bytes!("../../assets/logo-mark.svg"), &usvg::Options::default()).ok()?;
    let mut pm = tiny_skia::Pixmap::new(px, px)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(px as f32 / tree.size().width(), px as f32 / tree.size().height()), &mut pm.as_mut());
    Some(ColorImage::from_rgba_premultiplied([px as usize, px as usize], pm.data()))
}

/// The app's window icon, rendered from assets/logo.svg.
pub fn window_icon() -> Option<egui::IconData> {
    use resvg::{tiny_skia, usvg};
    const PX: u32 = 256;
    let tree = usvg::Tree::from_data(include_bytes!("../../assets/logo.svg"), &usvg::Options::default()).ok()?;
    let mut pm = tiny_skia::Pixmap::new(PX, PX)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(PX as f32 / tree.size().width(), PX as f32 / tree.size().height()), &mut pm.as_mut());
    let rgba = pm.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    });
    Some(egui::IconData { rgba: rgba.collect(), width: PX, height: PX })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_png_and_svg() {
        let dir = std::env::temp_dir().join(crate::tf!("cleancore-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        image::RgbaImage::from_pixel(128, 64, image::Rgba([255, 0, 0, 255])).save(&png).unwrap();
        let img = decode_file(&png).unwrap();
        assert_eq!(img.size, [48, 24]);
        let svg = dir.join("b.svg");
        std::fs::write(&svg, r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="blue"/></svg>"#).unwrap();
        let img = decode_file(&svg).unwrap();
        assert_eq!(img.size, [48, 48]);
        assert_eq!(img.pixels[48 * 24 + 24], egui::Color32::from_rgb(0, 0, 255));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod incremental_tests {
    use super::*;

    #[test]
    fn incremental_batches_keep_all_icon_receivers() {
        let ctx = egui::Context::default();
        let mut cache = IconCache::default();
        for id in ["first", "second"] {
            let (tx, rx) = channel();
            cache.receivers.push(rx);
            cache.requested.insert(id.into());
            tx.send((id.into(), ColorImage::new([1, 1], vec![egui::Color32::WHITE]))).unwrap();
        }
        cache.poll(&ctx);
        assert!(cache.get("first").is_some());
        assert!(cache.get("second").is_some());
        assert!(cache.receivers.is_empty());
    }
}
