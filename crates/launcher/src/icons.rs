//! Row icons, straight from the shell (IShellItemImageFactory) - the same
//! images Start and Explorer draw. Fetched on a few background COM threads,
//! requested at the exact physical pixel size, and cached for the life of the
//! process, so a row never waits: it draws without an icon for the frame or
//! two until it lands, and every later show is instant.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

/// Fetch threads. More than one, so a single slow icon (a shortcut on a
/// sleeping drive) cannot hold up every row behind it.
const WORKERS: usize = 3;

use eframe::egui::{self, ColorImage, TextureHandle, TextureId, TextureOptions};
use windows::Win32::Foundation::{LPARAM, SIZE, WPARAM};

/// "The data necessary to complete this operation is not yet available."
const E_PENDING: windows::core::HRESULT = windows::core::HRESULT(0x8000_000A_u32 as i32);
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, MsgWaitForMultipleObjects, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
    PostThreadMessageW, QS_ALLINPUT, TranslateMessage, WM_APP,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP,
    HGDIOBJ, ReleaseDC,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::{IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY};
use windows::core::{HSTRING, Interface};

/// What an icon is shared by. Ordinary files share one per extension (20
/// `.rs` hits = one fetch); executables and shortcuts carry their own.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IconKey {
    App(String),
    Path(PathBuf),
    Ext(String),
    Folder,
}

impl IconKey {
    pub fn for_file(path: &Path, is_dir: bool) -> Self {
        if is_dir {
            return Self::Folder;
        }
        match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) {
            Some(e) if !matches!(e.as_str(), "exe" | "lnk" | "ico" | "url" | "msc" | "cpl" | "scr" | "appref-ms") => {
                Self::Ext(e)
            }
            _ => Self::Path(path.to_path_buf()),
        }
    }
}

struct Request {
    key: IconKey,
    size: u32,
    /// Shell parsing name to fetch from: an AppsFolder path or a file path.
    parse: String,
}

enum Slot {
    Pending,
    Ready(TextureHandle),
    Missing,
}

pub struct Icons {
    queue: Arc<Mutex<VecDeque<Request>>>,
    /// Worker thread ids, poked with WM_APP when the queue gains work.
    workers: Vec<u32>,
    answers: Receiver<(IconKey, u32, Option<ColorImage>)>,
    cache: HashMap<(IconKey, u32), Slot>,
}

impl Icons {
    /// Shell icon handlers are apartment-threaded: under MTA some fail
    /// outright (ActivationManager, Adobe Application Manager drew blank). So
    /// each worker is an STA that keeps its message loop running between and
    /// during fetches - an STA parked in a plain wait starves the shell's
    /// callbacks into it.
    pub fn spawn(wake: impl Fn() + Send + Clone + 'static) -> Self {
        let queue: Arc<Mutex<VecDeque<Request>>> = Arc::default();
        let (answer, answers) = channel();
        let (tid_tx, tid_rx) = channel();
        for n in 0..WORKERS {
            let (queue, answer, wake, tid_tx) = (queue.clone(), answer.clone(), wake.clone(), tid_tx.clone());
            std::thread::Builder::new()
                .name(format!("icons-{n}"))
                .spawn(move || unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                    let mut msg = MSG::default();
                    let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE); // create the queue first
                    let _ = tid_tx.send(GetCurrentThreadId());
                    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                        if msg.message != WM_APP {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                            continue;
                        }
                        loop {
                            let Some(r) = queue.lock().ok().and_then(|mut q| q.pop_front()) else { break };
                            let t = std::time::Instant::now();
                            let image = fetch(&r.parse, r.size);
                            if t.elapsed().as_millis() > 500 {
                                patina_ui::log::line("launcher", &format!("slow icon ({} ms): {}", t.elapsed().as_millis(), r.parse));
                            }
                            if answer.send((r.key, r.size, image)).is_err() {
                                return;
                            }
                            wake();
                            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                                if msg.message != WM_APP {
                                    let _ = TranslateMessage(&msg);
                                    DispatchMessageW(&msg);
                                }
                            }
                        }
                    }
                })
                .expect("spawn icons thread");
        }
        drop(tid_tx);
        let workers = tid_rx.iter().take(WORKERS).collect();
        Self { queue, workers, answers, cache: HashMap::new() }
    }

    /// The icon if it is ready; otherwise asks for it (once) and returns None.
    pub fn get(&mut self, key: &IconKey, size: u32, parse: impl FnOnce() -> String) -> Option<TextureId> {
        let k = (key.clone(), size);
        match self.cache.get(&k) {
            Some(Slot::Ready(t)) => Some(t.id()),
            Some(_) => None,
            None => {
                self.cache.insert(k, Slot::Pending);
                if let Ok(mut q) = self.queue.lock() {
                    q.push_back(Request { key: key.clone(), size, parse: parse() });
                }
                // Poke every worker: whichever is free takes it, so one stuck
                // on a slow icon never holds up the rest.
                for &tid in &self.workers {
                    let _ = unsafe { PostThreadMessageW(tid, WM_APP, WPARAM(0), LPARAM(0)) };
                }
                None
            }
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut any = false;
        while let Ok((key, size, image)) = self.answers.try_recv() {
            let slot = match image {
                Some(img) => Slot::Ready(ctx.load_texture("icon", img, TextureOptions::LINEAR)),
                None => Slot::Missing,
            };
            self.cache.insert((key, size), slot);
            any = true;
        }
        any
    }
}

unsafe fn fetch(parse: &str, size: u32) -> Option<ColorImage> {
    let result: Result<ColorImage, String> = (|| unsafe {
        let item: IShellItem =
            SHCreateItemFromParsingName(&HSTRING::from(parse), None).map_err(|e| format!("parse: {e}"))?;
        let factory: IShellItemImageFactory = item.cast().map_err(|e| format!("factory: {e}"))?;
        // ICONONLY: never a thumbnail (a photo's preview would be slow and noisy here).
        // The shell extracts icons asynchronously: while its cache warms it
        // answers E_PENDING ("not yet") instead of waiting - 7-Zip and
        // ActivationManager drew blank until this retried. Pump while waiting.
        let mut tries = 0;
        let bitmap = loop {
            match factory.GetImage(SIZE { cx: size as i32, cy: size as i32 }, SIIGBF_ICONONLY) {
                Ok(b) => break b,
                Err(e) if e.code() == E_PENDING && tries < 20 => {
                    tries += 1;
                    pump_for(25);
                }
                Err(e) => return Err(format!("GetImage after {tries} retries: {e}")),
            }
        };
        let image = to_image(bitmap);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        image.ok_or_else(|| "bitmap conversion".to_string())
    })();
    match result {
        Ok(img) => Some(img),
        Err(e) => {
            patina_ui::log::line("launcher", &format!("icon failed ({e}): {parse}"));
            None
        }
    }
}

/// Wait up to `ms` while still serving this STA's messages (the shell's
/// async extraction may need to call back into it).
fn pump_for(ms: u32) {
    unsafe {
        let _ = MsgWaitForMultipleObjects(None, false, ms, QS_ALLINPUT);
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message != WM_APP {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

/// `--probe-icon <parsing name> [px]`: fetch one icon, log its alpha format.
pub fn probe(parse: &str, size: u32) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    PROBING.store(true, std::sync::atomic::Ordering::SeqCst);
    let got = unsafe { fetch(parse, size) }.is_some();
    patina_ui::log::line("launcher", &format!("probe icon {parse} @ {size}px: fetched={got}"));
}

static PROBING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 32-bit shell bitmap (BGRA) -> egui image, as STRAIGHT alpha. Icons are
/// stored straight (the .ico format) and GetImage(ICONONLY) hands them over
/// as-is: read as premultiplied, FL Studio's round icon got a pale halo.
/// Measured with --probe-icon: FL Studio has 63 and Discord 100 edge pixels
/// whose colour exceeds alpha (impossible if premultiplied); no icon measured
/// showed the opposite. Were one premultiplied, straight decoding errs dark
/// at the edge - near-invisible on a dark ground - not into a bright ring.
unsafe fn to_image(bitmap: HBITMAP) -> Option<ColorImage> {
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(HGDIOBJ(bitmap.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative: top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let rows = GetDIBits(dc, bitmap, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        if rows == 0 {
            return None;
        }
        // Old-style icons come back with no alpha at all: treat them as opaque.
        let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
        if PROBING.load(std::sync::atomic::Ordering::SeqCst) {
            let over = px.chunks_exact(4).filter(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]).count();
            let partial = px.chunks_exact(4).filter(|p| p[3] > 0 && p[3] < 255).count();
            patina_ui::log::line(
                "launcher",
                &format!(
                    "probe icon: {w}x{h}, has_alpha={has_alpha}, partly transparent px={partial}, colour>alpha px={over} ({})",
                    if over > 0 { "proves straight" } else { "either" }
                ),
            );
        }
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2);
            if !has_alpha {
                p[3] = 255;
            }
        }
        Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px))
    }
}
