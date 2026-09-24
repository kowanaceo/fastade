use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const MAX_SESSION_WINDOWS: usize = 10;
const SESSION_WINDOW_PREFIX: &str = "session-";

#[cfg(target_os = "macos")]
pub use display_fit::{
    fit_window_after_resize, fit_windows_after_display_change, watch_display_changes,
};

#[cfg(not(target_os = "macos"))]
pub fn watch_display_changes<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) {}

#[cfg(not(target_os = "macos"))]
pub fn fit_window_after_resize<R: tauri::Runtime>(_window: &tauri::Window<R>) {}

#[cfg(not(target_os = "macos"))]
pub fn fit_windows_after_display_change<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) {}

/// When a display is disconnected or reconnected, AppKit moves a native
/// fullscreen window to another screen but can leave it with the previous
/// screen's frame, so the window spills past the edges of the screen it's
/// now on. Snap every window back onto its current screen.
#[cfg(target_os = "macos")]
mod display_fit {
    use std::ptr::NonNull;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2_app_kit::{
        NSApplicationDidChangeScreenParametersNotification, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue, NSRect};
    use tauri::{AppHandle, Manager, Runtime};

    // AppKit keeps rearranging windows and fullscreen spaces for a while after
    // the screen-parameters notification, so a single immediate pass is not
    // enough to see the final frame.
    const REFIT_DELAYS_MS: [u64; 4] = [0, 400, 1200, 3000];

    pub fn watch_display_changes<R: Runtime>(app: &AppHandle<R>) {
        let handle = app.clone();
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            fit_windows_after_display_change(&handle);
        });
        let center = NSNotificationCenter::defaultCenter();
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &block,
            )
        };
        // The observer lives for the whole app lifetime.
        std::mem::forget(observer);
    }

    pub fn fit_windows_after_display_change<R: Runtime>(app: &AppHandle<R>) {
        for delay in REFIT_DELAYS_MS {
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(delay));
                let main_thread_app = app.clone();
                let _ = app.run_on_main_thread(move || fit_all_windows(&main_thread_app));
            });
        }
    }

    fn fit_all_windows<R: Runtime>(app: &AppHandle<R>) {
        for window in app.webview_windows().values() {
            let Ok(pointer) = window.ns_window() else {
                continue;
            };
            // SAFETY: Tauri returns the live NSWindow backing this window, and
            // we're on the main thread.
            let ns_window = unsafe { &*(pointer as *const NSWindow) };
            fit_to_screen(ns_window);
        }
    }

    fn fit_to_screen(window: &NSWindow) {
        let Some(screen) = window.screen() else {
            return;
        };
        let frame = window.frame();
        let target = if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
            screen.frame()
        } else {
            clamp_into(frame, screen.visibleFrame())
        };
        if !same_rect(frame, target) {
            window.setFrame_display(target, true);
        }
        fit_content_to_window(window);
    }

    /// The WKWebView follows the window only through autoresizing masks,
    /// which apply size *deltas*. When a fullscreen window crosses displays
    /// with different backing scales, the webview can pick up a frame in the
    /// wrong units (2x the screen) and the deltas preserve that error from
    /// then on, so the page renders past the window edges. Reset the content
    /// view and its webview to the window's actual content size.
    fn fit_content_to_window(window: &NSWindow) {
        let Some(content_view) = window.contentView() else {
            return;
        };
        let content_size = window.contentRectForFrameRect(window.frame()).size;
        let current = content_view.frame();
        if (current.size.width - content_size.width).abs() >= 0.5
            || (current.size.height - content_size.height).abs() >= 0.5
        {
            content_view.setFrameSize(content_size);
        }
        let bounds = content_view.bounds();
        for subview in content_view.subviews().iter() {
            if !same_rect(subview.frame(), bounds) {
                subview.setFrame(bounds);
            }
        }
    }

    pub fn fit_window_after_resize<R: Runtime>(window: &tauri::Window<R>) {
        let Ok(pointer) = window.ns_window() else {
            return;
        };
        // SAFETY: see fit_all_windows; window events are delivered on the main thread.
        let ns_window = unsafe { &*(pointer as *const NSWindow) };
        if !ns_window.styleMask().contains(NSWindowStyleMask::FullScreen) {
            return;
        }
        fit_content_to_window(ns_window);
        // Tauri's own resize handling can run after this handler; check again
        // once the event has settled.
        let app = window.app_handle().clone();
        let label = window.label().to_string();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let main_thread_app = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Some(window) = main_thread_app.get_webview_window(&label) {
                    if let Ok(pointer) = window.ns_window() {
                        fit_content_to_window(unsafe { &*(pointer as *const NSWindow) });
                    }
                }
            });
        });
    }

    fn clamp_into(frame: NSRect, bounds: NSRect) -> NSRect {
        let mut rect = frame;
        rect.size.width = rect.size.width.min(bounds.size.width);
        rect.size.height = rect.size.height.min(bounds.size.height);
        let max_x = bounds.origin.x + bounds.size.width - rect.size.width;
        let max_y = bounds.origin.y + bounds.size.height - rect.size.height;
        rect.origin.x = rect.origin.x.clamp(bounds.origin.x, max_x);
        rect.origin.y = rect.origin.y.clamp(bounds.origin.y, max_y);
        rect
    }

    fn same_rect(a: NSRect, b: NSRect) -> bool {
        (a.origin.x - b.origin.x).abs() < 0.5
            && (a.origin.y - b.origin.y).abs() < 0.5
            && (a.size.width - b.size.width).abs() < 0.5
            && (a.size.height - b.size.height).abs() < 0.5
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use objc2_foundation::{NSPoint, NSSize};

        fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
            NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
        }

        #[test]
        fn clamp_shrinks_and_moves_an_oversized_window_onto_the_screen() {
            let clamped = clamp_into(rect(-100.0, 50.0, 2560.0, 1440.0), rect(0.0, 0.0, 1920.0, 1055.0));
            assert!(same_rect(clamped, rect(0.0, 0.0, 1920.0, 1055.0)));
        }

        #[test]
        fn clamp_leaves_a_window_that_already_fits() {
            let frame = rect(100.0, 100.0, 1180.0, 760.0);
            assert!(same_rect(clamp_into(frame, rect(0.0, 0.0, 1920.0, 1055.0)), frame));
        }
    }
}

#[tauri::command]
pub async fn open_session_window(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    let label = format!("{SESSION_WINDOW_PREFIX}{session_id}");
    if let Some(window) = app.get_webview_window(&label) {
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }

    let open_count = app
        .webview_windows()
        .keys()
        .filter(|label| label.starts_with(SESSION_WINDOW_PREFIX))
        .count();
    if open_count >= MAX_SESSION_WINDOWS {
        return Err(format!(
            "You can open at most {MAX_SESSION_WINDOWS} session windows."
        ));
    }

    let url = WebviewUrl::App(format!("index.html?session={session_id}").into());
    WebviewWindowBuilder::new(&app, label, url)
        .title("fastade session")
        .inner_size(980.0, 720.0)
        .min_inner_size(720.0, 520.0)
        // Tauri's native drag-drop handler swallows HTML5 drag events, which
        // the sidebar's session drag-to-group relies on.
        .disable_drag_drop_handler()
        .build()
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_window_limit_is_ten() {
        assert_eq!(MAX_SESSION_WINDOWS, 10);
    }
}
