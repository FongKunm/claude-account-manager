//! Round the native window on Windows 10; newer Windows keeps DWM's own corners.
//! This clips the whole window, including WebView2 and native dialog backdrops.
use std::{ffi::c_void, mem::size_of, ptr::null_mut, sync::Mutex};
use tauri::{WebviewWindow, WindowEvent};

type Handle = *mut c_void;

#[repr(C)]
#[derive(Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetWindowRect(window: Handle, rect: *mut Rect) -> i32;
    fn GetDpiForWindow(window: Handle) -> u32;
    fn IsZoomed(window: Handle) -> i32;
    fn IsIconic(window: Handle) -> i32;
    fn SetWindowRgn(window: Handle, region: Handle, redraw: i32) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateRoundRectRgn(
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        width: i32,
        height: i32,
    ) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(window: Handle, attribute: u32, value: *const u32, size: u32) -> i32;
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Square,
    Rounded {
        width: i32,
        height: i32,
        diameter: i32,
    },
}

fn update(window: &WebviewWindow, previous: &Mutex<Option<Shape>>) {
    let Ok(handle) = window.hwnd() else { return };
    let hwnd = handle.0 as Handle;
    // Minimized dimensions are not useful; restore always produces a resize.
    if unsafe { IsIconic(hwnd) } != 0 {
        return;
    }
    let shape = if unsafe { IsZoomed(hwnd) } != 0 || window.is_fullscreen().unwrap_or(false) {
        Shape::Square
    } else {
        let mut rect = Rect::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            return;
        }
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        if width <= 0 || height <= 0 {
            return;
        }
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        Shape::Rounded {
            width,
            height,
            diameter: ((20 * dpi + 48) / 96) as i32,
        }
    };
    {
        let Ok(mut last) = previous.lock() else {
            return;
        };
        if *last == Some(shape) {
            return;
        }
        // SetWindowRgn sends window-position messages. Cache before calling it,
        // and release the lock, so unchanged/reentrant events cannot loop.
        *last = Some(shape);
    }
    let region = match shape {
        Shape::Square => null_mut(),
        Shape::Rounded {
            width,
            height,
            diameter,
        } => unsafe {
            // GDI excludes the bottom/right outline from a rounded rectangle.
            CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter)
        },
    };
    let created = matches!(shape, Shape::Square) || !region.is_null();
    if !created || unsafe { SetWindowRgn(hwnd, region, 1) } == 0 {
        // Ownership transfers to Windows only on success.
        if !region.is_null() {
            unsafe {
                DeleteObject(region);
            }
        }
        if let Ok(mut last) = previous.lock() {
            *last = None;
        }
    }
}

pub(crate) fn install(window: &WebviewWindow) {
    let Ok(handle) = window.hwnd() else { return };
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;
    // Supported starting with Windows 11. Never set a custom region there:
    // regions disable DWM's native rounding and its maximized/snapped policy.
    if unsafe {
        DwmSetWindowAttribute(
            handle.0 as Handle,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &DWMWCP_ROUND,
            size_of::<u32>() as u32,
        )
    } >= 0
    {
        return;
    }

    // Tao's Win10 DWM shadow leaves invisible side/bottom insets. Rounding the
    // outer HWND would miss the visible client corners. Without that frame Tao
    // supplies hit testing on all four edges; keep the requested client size.
    let size = window.inner_size().ok();
    if window.set_shadow(false).is_err() {
        return;
    }
    if let Some(size) = size {
        let _ = window.set_size(size);
    }
    let previous = Mutex::new(None);
    update(window, &previous);
    let owned_window = window.clone();
    window.on_window_event(move |event| {
        if matches!(
            event,
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }
        ) {
            update(&owned_window, &previous);
        }
    });
}
