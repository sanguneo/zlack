use std::sync::LazyLock;

use tauri::image::Image;

#[cfg(target_os = "windows")]
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    Graphics::Gdi::{
        CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    },
    UI::WindowsAndMessaging::{
        CreateIconIndirect, SendMessageW, HICON, ICONINFO, ICON_BIG, ICON_SMALL, WM_SETICON,
    },
};

use crate::exe_sibling;

// --- Tray icon badges -------------------------------------------------------
// We composite a small coloured dot onto the base tray icon to mirror Slack's
// behaviour: a red badge for unread DMs / @mentions and a blue badge for other
// unread messages. The three variants are built once and reused.

const TRAY_ICON_SIZE: u32 = 32;
const WINDOW_ICON_SIZE: u32 = 256;
pub(crate) const BADGE_BLUE: [u8; 3] = [0x09, 0xc1, 0xf5];
pub(crate) const BADGE_RED: [u8; 3] = [224, 30, 90];

const BADGE_ANTIALIAS_SAMPLES: i32 = 4;
const TRAY_BADGE_RADIUS: f32 = 9.5;
const TRAY_BADGE_OUTLINE_WIDTH: f32 = 1.5;

#[cfg(target_os = "windows")]
const TASKBAR_BADGE_ICON_SIZE: i32 = 32;
#[cfg(target_os = "windows")]
const TASKBAR_BADGE_RADIUS: f32 = 13.0;
#[cfg(target_os = "windows")]
const TASKBAR_BADGE_OUTLINE_WIDTH: f32 = 2.0;

fn blend_pixel(img: &mut image::RgbaImage, x: u32, y: u32, src: [u8; 4]) {
    let src_a = src[3] as f32 / 255.0;
    if src_a <= 0.0 {
        return;
    }
    let dst = img.get_pixel(x, y).0;
    let dst_a = dst[3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return;
    }

    let mut out = [0u8; 4];
    for i in 0..3 {
        let src_c = src[i] as f32 / 255.0;
        let dst_c = dst[i] as f32 / 255.0;
        out[i] = (((src_c * src_a + dst_c * dst_a * (1.0 - src_a)) / out_a) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    out[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    img.put_pixel(x, y, image::Rgba(out));
}

fn draw_badge_circle(
    img: &mut image::RgbaImage,
    color: [u8; 3],
    cx: f32,
    cy: f32,
    outer_radius: f32,
    outline_width: f32,
) {
    let inner_radius = (outer_radius - outline_width).max(0.0);
    let min_x = (cx - outer_radius - 1.0).floor() as i32;
    let max_x = (cx + outer_radius + 1.0).ceil() as i32;
    let min_y = (cy - outer_radius - 1.0).floor() as i32;
    let max_y = (cy + outer_radius + 1.0).ceil() as i32;
    let sample_count = (BADGE_ANTIALIAS_SAMPLES * BADGE_ANTIALIAS_SAMPLES) as f32;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
                continue;
            }

            let mut a = 0.0;
            let mut r = 0.0;
            let mut g = 0.0;
            let mut b = 0.0;
            for sy in 0..BADGE_ANTIALIAS_SAMPLES {
                for sx in 0..BADGE_ANTIALIAS_SAMPLES {
                    let px = x as f32 + (sx as f32 + 0.5) / BADGE_ANTIALIAS_SAMPLES as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / BADGE_ANTIALIAS_SAMPLES as f32;
                    let dx = px - cx;
                    let dy = py - cy;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist <= inner_radius {
                        a += 1.0;
                        r += color[0] as f32;
                        g += color[1] as f32;
                        b += color[2] as f32;
                    } else if dist <= outer_radius {
                        a += 1.0;
                    }
                }
            }

            if a > 0.0 {
                let alpha = a / sample_count;
                blend_pixel(
                    img,
                    x as u32,
                    y as u32,
                    [
                        (r / a).round() as u8,
                        (g / a).round() as u8,
                        (b / a).round() as u8,
                        (alpha * 255.0).round() as u8,
                    ],
                );
            }
        }
    }
}

fn draw_badge(img: &mut image::RgbaImage, color: [u8; 3]) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let r = TRAY_BADGE_RADIUS.min(w.min(h) / 2.0).max(1.0);
    draw_badge_circle(
        img,
        color,
        w - r - 1.0,
        h - r - 1.0,
        r,
        TRAY_BADGE_OUTLINE_WIDTH,
    );
}

fn load_named_icon_image(size: u32, names: &[&str]) -> image::RgbaImage {
    let img = names
        .iter()
        .filter_map(|name| exe_sibling(name))
        .find_map(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|bytes| image::load_from_memory(&bytes).ok())
        })
        .unwrap_or_else(|| {
            image::load_from_memory(include_bytes!("../icons/icon.png"))
                .expect("Zlack: failed to decode bundled icon")
        })
        .to_rgba8();

    if img.width() == size && img.height() == size {
        img
    } else {
        image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3)
    }
}

fn load_icon_image(size: u32) -> image::RgbaImage {
    load_named_icon_image(size, &["zlack.png", "zlack.ico"])
}

#[cfg(target_os = "windows")]
fn load_taskbar_icon_image(size: u32) -> image::RgbaImage {
    load_named_icon_image(size, &["zlack-taskbar.png", "zlack.png", "zlack.ico"])
}

fn icon_from_image(img: image::RgbaImage) -> Image<'static> {
    let (w, h) = (img.width(), img.height());
    Image::new_owned(img.into_raw(), w, h)
}

fn make_icon(badge: Option<[u8; 3]>) -> Image<'static> {
    let mut img = load_icon_image(TRAY_ICON_SIZE);
    if let Some(color) = badge {
        draw_badge(&mut img, color);
    }
    icon_from_image(img)
}

#[cfg(target_os = "windows")]
fn set_windows_window_icons(window: &tauri::WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };

    unsafe {
        let icons = [
            (load_icon_image(TRAY_ICON_SIZE), ICON_SMALL),
            (load_taskbar_icon_image(WINDOW_ICON_SIZE), ICON_BIG),
        ];
        for (img, icon_type) in icons {
            let (w, h) = (img.width(), img.height());
            if let Some(hicon) = rgba_to_hicon(&img.into_raw(), w, h) {
                // WM_SETICON keeps using the HICON handle; do not destroy it here.
                let _ = SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(icon_type as usize)),
                    Some(LPARAM(hicon.0 as isize)),
                );
            }
        }
    }
}

pub(crate) fn apply_window_icon(window: &tauri::WebviewWindow) {
    let _ = window.set_icon(ICON_WINDOW.clone());
    #[cfg(target_os = "windows")]
    set_windows_window_icons(window);
}

pub(crate) static ICON_WINDOW: LazyLock<Image<'static>> =
    LazyLock::new(|| icon_from_image(load_icon_image(WINDOW_ICON_SIZE)));
pub(crate) static ICON_NORMAL: LazyLock<Image<'static>> = LazyLock::new(|| make_icon(None));
pub(crate) static ICON_BLUE: LazyLock<Image<'static>> =
    LazyLock::new(|| make_icon(Some(BADGE_BLUE))); // general unread
pub(crate) static ICON_RED: LazyLock<Image<'static>> = LazyLock::new(|| make_icon(Some(BADGE_RED))); // DM / mention

// --- Windows taskbar overlay icon ------------------------------------------
// The tray badge above covers the hidden-to-tray case. When the window is
// visible on the taskbar we also overlay a coloured dot on its button
// (red = DM/mention, blue = other unread).
#[cfg(target_os = "windows")]
pub(crate) fn overlay_icon(color: [u8; 3]) -> Image<'static> {
    let size = TASKBAR_BADGE_ICON_SIZE as u32;
    let mut img = image::RgbaImage::from_pixel(size, size, image::Rgba([0, 0, 0, 0]));
    let c = size as f32 / 2.0;
    let r = TASKBAR_BADGE_RADIUS.min(c).max(1.0);
    draw_badge_circle(&mut img, color, c, c, r, TASKBAR_BADGE_OUTLINE_WIDTH);
    icon_from_image(img)
}

#[cfg(target_os = "windows")]
fn rgba_to_hicon(rgba: &[u8], w: u32, h: u32) -> Option<HICON> {
    unsafe {
        // 32bpp top-down DIB section so the per-pixel alpha is preserved (a plain
        // CreateBitmap DDB can drop alpha and render the badge as an opaque block).
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w as i32;
        bmi.bmiHeader.biHeight = -(h as i32); // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        // biCompression stays 0 (BI_RGB) from the zeroed struct.
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let hbm_color = match CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) if !bits.is_null() => b,
            _ => return None,
        };
        // Fill the DIB with BGRA pixels.
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, rgba.len());
        let mut i = 0;
        while i + 3 < rgba.len() {
            dst[i] = rgba[i + 2];
            dst[i + 1] = rgba[i + 1];
            dst[i + 2] = rgba[i];
            dst[i + 3] = rgba[i + 3];
            i += 4;
        }
        // Monochrome AND mask, all zero (alpha channel drives transparency).
        let mask_len = ((((w + 15) & !15) / 8) * h) as usize;
        let mask = vec![0u8; mask_len];
        let hbm_mask = CreateBitmap(w as i32, h as i32, 1, 1, Some(mask.as_ptr() as *const _));
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };
        let hicon = CreateIconIndirect(&info);
        let _ = DeleteObject(hbm_color.into());
        let _ = DeleteObject(hbm_mask.into());
        hicon.ok()
    }
}
