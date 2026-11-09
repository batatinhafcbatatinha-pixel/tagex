//! Thumbnails and icons from the Windows shell (`IShellItemImageFactory`), with a disk cache.
//!
//! Workers take the *newest* request first, so whatever is on screen right now wins over
//! things that already scrolled away.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{ColorImage, TextureHandle, TextureOptions};

pub const THUMB_SIZE: u32 = 256;
const ICON_SIZE: u32 = 128;
const MAX_QUEUE: usize = 400;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    File(i64),
    Ext(String),
}

struct Request {
    key: Key,
    path: String,
    size: u64,
    mtime: i64,
    thumb: bool,
    decodable: bool,
}

enum Done {
    Ok(Key, ColorImage),
    Failed(Key),
}

enum State {
    Loading,
    Ready(TextureHandle),
    Failed,
}

struct Slot {
    state: State,
    last_used: u64,
}

pub struct Thumbs {
    queue: Arc<(Mutex<Vec<Request>>, Condvar)>,
    rx: Receiver<Done>,
    slots: HashMap<Key, Slot>,
    frame: u64,
}

pub enum Lookup<'a> {
    Ready(&'a TextureHandle),
    Pending,
    Failed,
}

impl Thumbs {
    pub fn new(ctx: egui::Context) -> Self {
        let cache_dir = cache_dir();
        let _ = std::fs::create_dir_all(&cache_dir);
        let queue: Arc<(Mutex<Vec<Request>>, Condvar)> = Arc::default();
        let (tx, rx) = unbounded();
        let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(4, 12);
        for i in 0..workers {
            let queue = queue.clone();
            let tx: Sender<Done> = tx.clone();
            let ctx = ctx.clone();
            let cache_dir = cache_dir.clone();
            std::thread::Builder::new()
                .name(format!("thumb-{i}"))
                .spawn(move || worker(queue, tx, ctx, cache_dir))
                .expect("spawn thumb worker");
        }
        Self { queue, rx, slots: HashMap::new(), frame: 0 }
    }

    /// Call once per frame before drawing: uploads finished images and evicts old textures.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        for done in self.rx.try_iter() {
            match done {
                Done::Ok(key, img) => {
                    let tex = ctx.load_texture(format!("{key:?}"), img, TextureOptions::LINEAR);
                    if let Some(slot) = self.slots.get_mut(&key) {
                        slot.state = State::Ready(tex);
                    }
                }
                Done::Failed(key) => {
                    if let Some(slot) = self.slots.get_mut(&key) {
                        slot.state = State::Failed;
                    }
                }
            }
        }
        if self.frame % 120 == 0 && self.slots.len() > 3000 {
            let cutoff = self.frame.saturating_sub(600);
            self.slots.retain(|k, s| matches!(k, Key::Ext(_)) || s.last_used >= cutoff);
        }
    }

    pub fn get(&mut self, key: Key, path: &str, size: u64, mtime: i64, thumb: bool, decodable: bool) -> Lookup<'_> {
        let frame = self.frame;
        if !self.slots.contains_key(&key) {
            let mut q = self.queue.0.lock().unwrap();
            q.push(Request { key: key.clone(), path: path.to_owned(), size, mtime, thumb, decodable });
            if q.len() > MAX_QUEUE {
                let excess = q.len() - MAX_QUEUE;
                let stale: Vec<Request> = q.drain(..excess).collect();
                for r in stale {
                    self.slots.remove(&r.key);
                }
            }
            drop(q);
            self.queue.1.notify_one();
            self.slots.insert(key.clone(), Slot { state: State::Loading, last_used: frame });
        }
        let slot = self.slots.get_mut(&key).unwrap();
        slot.last_used = frame;
        match &slot.state {
            State::Ready(t) => Lookup::Ready(t),
            State::Loading => Lookup::Pending,
            State::Failed => Lookup::Failed,
        }
    }

    /// Drops a cached thumbnail (e.g. after rename) so it is fetched again.
    pub fn forget(&mut self, key: &Key) {
        self.slots.remove(key);
    }
}

fn cache_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "Tagex")
        .map(|d| d.cache_dir().join("thumbs"))
        .unwrap_or_else(|| std::env::temp_dir().join("tagex-thumbs"))
}

fn worker(queue: Arc<(Mutex<Vec<Request>>, Condvar)>, tx: Sender<Done>, ctx: egui::Context, cache_dir: PathBuf) {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    loop {
        let req = {
            let mut q = queue.0.lock().unwrap();
            loop {
                if let Some(r) = q.pop() {
                    break r;
                }
                q = queue.1.wait(q).unwrap();
            }
        };
        let img = if req.thumb {
            make_thumb(&req, &cache_dir)
        } else {
            shell::image(&req.path, ICON_SIZE, shell::Mode::Icon)
        };
        let msg = match img {
            Some(img) => Done::Ok(req.key, img),
            None => Done::Failed(req.key),
        };
        if tx.send(msg).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in *p {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h ^= 0xff;
    }
    h
}

fn make_thumb(req: &Request, cache_dir: &Path) -> Option<ColorImage> {
    let key = fnv1a(&[
        req.path.to_lowercase().as_bytes(),
        &req.size.to_le_bytes(),
        &req.mtime.to_le_bytes(),
    ]);
    let cache_file = cache_dir.join(format!("{key:016x}.png"));
    if let Ok(bytes) = std::fs::read(&cache_file) {
        if let Ok(img) = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png) {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            return Some(ColorImage::from_rgba_premultiplied(size, rgba.as_raw()));
        }
    }

    let img = shell::image(&req.path, THUMB_SIZE, shell::Mode::Thumbnail).or_else(|| {
        if !req.decodable {
            return None;
        }
        let img = image::open(&req.path).ok()?.thumbnail(THUMB_SIZE, THUMB_SIZE).to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        // Store premultiplied so the cache and the shell path share one format.
        let mut px = img.into_raw();
        for c in px.chunks_exact_mut(4) {
            let a = c[3] as u16;
            for v in &mut c[..3] {
                *v = ((*v as u16 * a + 127) / 255) as u8;
            }
        }
        Some(ColorImage::from_rgba_premultiplied(size, &px))
    })?;

    save_png(&cache_file, &img);
    Some(img)
}

fn save_png(path: &Path, img: &ColorImage) {
    use image::ImageEncoder;
    let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
    let Ok(file) = std::fs::File::create(path) else { return };
    let enc = image::codecs::png::PngEncoder::new_with_quality(
        std::io::BufWriter::new(file),
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    );
    let _ = enc.write_image(&raw, img.size[0] as u32, img.size[1] as u32, image::ExtendedColorType::Rgba8);
}

mod shell {
    use egui::ColorImage;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, GetDIBits, GetObjectW, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
        SIIGBF_THUMBNAILONLY,
    };
    use windows::core::HSTRING;

    pub enum Mode {
        Thumbnail,
        Icon,
    }

    pub fn image(path: &str, size: u32, mode: Mode) -> Option<ColorImage> {
        unsafe {
            let factory: IShellItemImageFactory =
                SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
            let flags = match mode {
                Mode::Thumbnail => SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK,
                Mode::Icon => SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
            };
            let sz = SIZE { cx: size as i32, cy: size as i32 };
            // E_PENDING: the shell is still extracting the icon on another thread; retry.
            const E_PENDING: windows::core::HRESULT = windows::core::HRESULT(0x8000_000Au32 as i32);
            let mut attempt = 0;
            let hbmp = loop {
                match factory.GetImage(sz, flags) {
                    Ok(h) => break h,
                    Err(e) if e.code() == E_PENDING && attempt < 20 => {
                        attempt += 1;
                        std::thread::sleep(std::time::Duration::from_millis(15 * attempt));
                    }
                    Err(_) => return None,
                }
            };
            let img = bitmap_to_image(hbmp, size);
            let _ = DeleteObject(HGDIOBJ(hbmp.0));
            img
        }
    }

    unsafe fn bitmap_to_image(hbmp: HBITMAP, max: u32) -> Option<ColorImage> {
        unsafe {
            let mut bm = BITMAP::default();
            if GetObjectW(HGDIOBJ(hbmp.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
                return None;
            }
            let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
            if w <= 0 || h <= 0 {
                return None;
            }
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let dc = CreateCompatibleDC(None);
            let lines = GetDIBits(dc, hbmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
            let _ = DeleteDC(dc);
            if lines == 0 {
                return None;
            }
            let opaque = buf.chunks_exact(4).all(|p| p[3] == 0);
            for p in buf.chunks_exact_mut(4) {
                p.swap(0, 2); // BGRA -> RGBA
                if opaque {
                    p[3] = 255;
                }
            }
            let img = image::RgbaImage::from_raw(w as u32, h as u32, buf)?;
            // Shell may hand back something bigger than asked (BIGGERSIZEOK); shrink it.
            let img = if w as u32 > max || h as u32 > max {
                image::DynamicImage::ImageRgba8(img).thumbnail(max, max).to_rgba8()
            } else {
                img
            };
            Some(ColorImage::from_rgba_premultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    /// Set TAGEX_TEST_DIR to a folder with main.rs/db.rs/app.rs/notepad.exe; run with --ignored.
    #[test]
    #[ignore]
    fn shell_icons() {
        let dir = std::env::var("TAGEX_TEST_DIR").unwrap_or_else(|_| "src".into());
        let handles: Vec<_> = ["main.rs", "db.rs", "app.rs", "notepad.exe"]
            .iter()
            .map(|f| {
                let p = format!("{dir}/{f}").replace('/', "\\");
                std::thread::spawn(move || unsafe {
                    let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
                    let r = super::shell::image(&p, 128, super::shell::Mode::Icon);
                    (p, r.map(|i| i.size))
                })
            })
            .collect();
        for h in handles {
            let (p, r) = h.join().unwrap();
            eprintln!("{p}: {r:?}");
        }
    }
}
