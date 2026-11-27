//! `ax.press` / `ax.set_value`: direct element lookup by app + role + name.
//!
//! Lookup order for a window (voice-control.md §4, jisuno.md §10 round 2
//! item 2: no AX tree traversal):
//!
//! 1. Direct attributes on the focused window: `AXDefaultButton` /
//!    `AXCancelButton` for `AXButton` (title compared to `name`).
//! 2. A bounded breadth-first walk: depth at most 4, at most 300 elements
//!    total, matching `AXRole == role` and (`AXTitle == name` or
//!    `AXDescription == name`).
//!
//! Step 2 is deliberately minimal: it exists because AppKit exposes no
//! direct attribute for arbitrary buttons, and the bound keeps the worst
//! case to a few hundred cheap attribute reads. Whether even this bounded
//! walk is acceptable under the "never a scan" rule is an open spec
//! question (see the crate report).
//!
//! `app` is a running app name, or `"focused"` for the frontmost app.
//! Missing elements fail with `AX element not found: <role> "<name>"`.

/// Bounds for the fallback walk (see module docs).
#[cfg(any(target_os = "macos", test))]
pub(crate) const MAX_AX_DEPTH: usize = 4;
#[cfg(any(target_os = "macos", test))]
pub(crate) const MAX_AX_ELEMENTS: usize = 300;

/// Press the element: `ax.press {app, role, name}`.
pub(crate) fn press(app: &str, role: &str, name: &str) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, role, name);
        Err("jisuno-platform: macOS only".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        macos::press(app, role, name)
    }
}

/// Set the element's value: `ax.set_value {app, role, name, value}`.
pub(crate) fn set_value(app: &str, role: &str, name: &str, value: &str) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, role, name, value);
        Err("jisuno-platform: macOS only".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        macos::set_value(app, role, name, value)
    }
}

#[cfg(target_os = "macos")]
pub(crate) mod macos {
    use std::ffi::{c_void, CStr};

    use super::super::ResolvedWindow;
    use super::{MAX_AX_DEPTH, MAX_AX_ELEMENTS};

    /// AXError success.
    pub(crate) const AX_OK: i32 = 0;
    /// `AXUIElement` value types for `AXValueCreate`.
    const AX_POINT: u32 = 1; // kAXValueCGPointType
    const AX_SIZE: u32 = 2; // kAXValueCGSizeType
    const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8
    const MAX_TITLE_BYTES: usize = 4096;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
        fn AXUIElementCopyAttributeValue(
            element: *const c_void,
            attribute: *const c_void,
            value: *mut *const c_void,
        ) -> i32;
        fn AXUIElementCopyAttributeValues(
            element: *const c_void,
            attribute: *const c_void,
            index: i64,
            max: i64,
            values: *mut *const c_void,
        ) -> i32;
        fn AXUIElementSetAttributeValue(
            element: *const c_void,
            attribute: *const c_void,
            value: *const c_void,
        ) -> i32;
        fn AXUIElementPerformAction(element: *const c_void, action: *const c_void) -> i32;
        fn AXValueCreate(kind: u32, value: *const c_void) -> *mut c_void;
        fn AXValueGetValue(value: *const c_void, kind: u32, out: *mut c_void) -> bool;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            s: *const i8,
            encoding: u32,
        ) -> *const c_void;
        fn CFStringGetCString(
            s: *const c_void,
            buffer: *mut i8,
            buffer_size: isize,
            encoding: u32,
        ) -> u8;
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFGetTypeID(value: *const c_void) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFRelease(value: *const c_void);
    }

    /// Owned CoreFoundation object; released on drop.
    pub(crate) struct Cf(*const c_void);

    impl Cf {
        fn take(ptr: *const c_void) -> Option<Self> {
            if ptr.is_null() {
                None
            } else {
                Some(Self(ptr))
            }
        }

        pub(crate) fn get(&self) -> *const c_void {
            self.0
        }
    }

    impl Drop for Cf {
        fn drop(&mut self) {
            // SAFETY: created by a Create/Copy call; exactly one release.
            unsafe { CFRelease(self.0) };
        }
    }

    /// `CFStringRef` for a Rust string (never null for valid UTF-8).
    pub(crate) fn cf_string(text: &str) -> Option<Cf> {
        let bytes = std::ffi::CString::new(text).ok()?;
        // SAFETY: NUL-terminated bytes, valid encoding constant.
        let ptr = unsafe { CFStringCreateWithCString(std::ptr::null(), bytes.as_ptr(), UTF8) };
        Cf::take(ptr)
    }

    /// Read a `CFStringRef` value into a Rust string.
    fn cf_to_string(ptr: *const c_void) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: pure type query.
        if unsafe { CFGetTypeID(ptr) } != unsafe { CFStringGetTypeID() } {
            return None;
        }
        let mut buffer = vec![0i8; MAX_TITLE_BYTES];
        // SAFETY: buffer is ours, encoding valid; non-zero means success.
        let ok =
            unsafe { CFStringGetCString(ptr, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) };
        if ok == 0 {
            return None;
        }
        // SAFETY: CFStringGetCString wrote a NUL-terminated C string.
        let cstr = unsafe { CStr::from_ptr(buffer.as_ptr()) };
        Some(cstr.to_string_lossy().into_owned())
    }

    /// Copy one attribute value; `None` when the attribute is absent.
    /// Returns the raw AXError so callers can spot permission failures.
    pub(crate) fn copy_attr(element: *const c_void, attribute: &str) -> Result<Option<Cf>, i32> {
        let name = cf_string(attribute).ok_or(AX_OK)?;
        let mut out: *const c_void = std::ptr::null();
        // SAFETY: element and name are valid; `out` receives a +1 reference.
        let code = unsafe { AXUIElementCopyAttributeValue(element, name.get(), &mut out) };
        if code != AX_OK {
            return Err(code);
        }
        Ok(Cf::take(out))
    }

    /// Copy an attribute array (e.g. `AXWindows`, `AXChildren`).
    pub(crate) fn copy_attr_array(
        element: *const c_void,
        attribute: &str,
    ) -> Result<Option<Cf>, i32> {
        let name = cf_string(attribute).ok_or(AX_OK)?;
        let mut out: *const c_void = std::ptr::null();
        // SAFETY: as above; at most 100 entries keeps windows/children reads
        // bounded (windows lists are small; children fan out in the BFS).
        let code = unsafe { AXUIElementCopyAttributeValues(element, name.get(), 0, 100, &mut out) };
        if code != AX_OK {
            return Err(code);
        }
        Ok(Cf::take(out))
    }

    /// Array length; `None` for a null array.
    fn array_len(array: Option<&Cf>) -> usize {
        match array {
            Some(array) => {
                // SAFETY: live CFArrayRef held by the guard.
                unsafe { CFArrayGetCount(array.get()) }.max(0) as usize
            }
            None => 0,
        }
    }

    /// Borrowed element at `index`. Valid while `array` is alive.
    fn array_get(array: &Cf, index: isize) -> *const c_void {
        // SAFETY: index is bounds-checked by the caller against CFArrayGetCount.
        unsafe { CFArrayGetValueAtIndex(array.get(), index) }
    }

    /// Read a string attribute; `None` when absent or not a string.
    pub(crate) fn attr_string(
        element: *const c_void,
        attribute: &str,
    ) -> Result<Option<String>, i32> {
        match copy_attr(element, attribute)? {
            Some(value) => Ok(cf_to_string(value.get())),
            None => Ok(None),
        }
    }

    /// Perform a parameterless AX action (`AXPress`, `AXRaise`, ...).
    pub(crate) fn perform(element: *const c_void, action: &str) -> i32 {
        let Some(name) = cf_string(action) else {
            return -1;
        };
        // SAFETY: element and action name are valid.
        unsafe { AXUIElementPerformAction(element, name.get()) }
    }

    /// Element for the app's pid (+1 reference, released by the guard).
    pub(crate) fn app_element(pid: i32) -> Cf {
        // SAFETY: pid validity only affects which app the element refers to.
        let ptr = unsafe { AXUIElementCreateApplication(pid) };
        Cf::take(ptr).expect("AXUIElementCreateApplication returned null")
    }

    /// Point/size value for `AXPosition` / `AXSize`.
    pub(crate) fn ax_value(kind: u32, pair: [f64; 2]) -> Option<Cf> {
        // SAFETY: pair is two f64s; AXValueCreate copies them synchronously.
        let ptr = unsafe { AXValueCreate(kind, pair.as_ptr() as *const c_void) };
        Cf::take(ptr)
    }

    /// Read an `AXValue`-wrapped point (`AXPosition`).
    pub(crate) fn read_point(element: *const c_void) -> Result<Option<[f64; 2]>, i32> {
        let Some(value) = copy_attr(element, "AXPosition")? else {
            return Ok(None);
        };
        let mut pair = [0f64; 2];
        // SAFETY: `out` is our two-f64 buffer; false means a type mismatch.
        let ok =
            unsafe { AXValueGetValue(value.get(), AX_POINT, pair.as_mut_ptr() as *mut c_void) };
        Ok(ok.then_some(pair))
    }

    /// Set `AXPosition` + `AXSize` in AX coordinates. Returns the raw AXError.
    pub(crate) fn set_frame_ax(element: *const c_void, x: f64, y: f64, w: f64, h: f64) -> i32 {
        let (Some(position), Some(size)) = (ax_value(AX_POINT, [x, y]), ax_value(AX_SIZE, [w, h]))
        else {
            return -1;
        };
        let (Some(pos_name), Some(size_name)) = (cf_string("AXPosition"), cf_string("AXSize"))
        else {
            return -1;
        };
        // SAFETY: element, names, and values are valid.
        let code = unsafe { AXUIElementSetAttributeValue(element, pos_name.get(), position.get()) };
        if code != AX_OK {
            return code;
        }
        unsafe { AXUIElementSetAttributeValue(element, size_name.get(), size.get()) }
    }

    /// Pid for a running app name (case-insensitive `localizedName` match),
    /// or for the frontmost app when `app` is `"focused"`.
    pub(crate) fn pid_for_app(app: &str) -> Option<i32> {
        // NOTE: call from the main thread; AppKit may misbehave otherwise.
        if app == "focused" {
            return crate::frontmost::frontmost_app().map(|info| info.pid);
        }
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        let running = workspace.runningApplications();
        for candidate in running.iter() {
            let name = candidate
                .localizedName()
                .map(|name| name.to_string())
                .unwrap_or_default();
            if name.eq_ignore_ascii_case(app) {
                return Some(candidate.processIdentifier());
            }
        }
        None
    }

    /// The app's windows as (+1) element guards, kept alive by `held`.
    /// `held` must outlive the returned pointers: windows borrow the array.
    pub(crate) struct Windows {
        _held: Vec<Cf>,
        /// Raw element pointers, valid while `self` is alive.
        pub(crate) items: Vec<*const c_void>,
    }

    /// All `AXWindow` elements of the app (role-filtered; sheets skipped).
    pub(crate) fn app_windows(pid: i32) -> Result<Windows, i32> {
        let app = app_element(pid);
        let array = copy_attr_array(app.get(), "AXWindows")?;
        let mut held = vec![app];
        let mut items = Vec::new();
        if let Some(array) = array {
            for index in 0..array_len(Some(&array)) {
                let element = array_get(&array, index as isize);
                if element.is_null() {
                    continue;
                }
                match attr_string(element, "AXRole") {
                    Ok(Some(role)) if role == "AXWindow" => items.push(element),
                    _ => {}
                }
            }
            held.push(array);
        }
        Ok(Windows { _held: held, items })
    }

    /// Window titles for matching (parallel to [`Windows::items`]).
    pub(crate) fn window_titles(windows: &Windows) -> Vec<Option<String>> {
        windows
            .items
            .iter()
            .map(|element| attr_string(*element, "AXTitle").unwrap_or(None))
            .collect()
    }

    /// First window whose title contains `filter` (or the first window when
    /// `filter` is `None`). Index into the `windows` items.
    pub(crate) fn pick_window(
        windows: &Windows,
        titles: &[Option<String>],
        filter: Option<&str>,
    ) -> Option<usize> {
        match filter {
            Some(filter) if !filter.is_empty() => titles
                .iter()
                .position(|title| title.as_ref().is_some_and(|title| title.contains(filter))),
            _ => {
                if windows.items.is_empty() {
                    None
                } else {
                    Some(0)
                }
            }
        }
    }

    /// Find one window element for `resolved`: the title match when given,
    /// else the first window. The `Windows` guard must outlive the pointer.
    pub(crate) fn find_window_element(
        resolved: &ResolvedWindow,
    ) -> Result<(Windows, *const c_void), String> {
        let Some(pid) = pid_for_app(&resolved.app) else {
            return Err(format!("no window \"{}\"", resolved.label));
        };
        let windows = app_windows(pid).map_err(|_| format!("no window \"{}\"", resolved.label))?;
        let titles = window_titles(&windows);
        let Some(index) = pick_window(&windows, &titles, resolved.title.as_deref()) else {
            return Err(format!("no window \"{}\"", resolved.label));
        };
        // Raw pointer: no borrow, so it can travel with its owner. Valid
        // while the returned `Windows` guard lives.
        let element = windows.items[index];
        Ok((windows, element))
    }

    /// Direct-attribute candidates for (role, name) on `window`: the default
    /// and cancel buttons for `AXButton`. Matched guards move into `held` so
    /// the returned pointers stay alive for the synchronous action below.
    fn direct_lookup(
        window: *const c_void,
        role: &str,
        name: &str,
        held: &mut Vec<Cf>,
    ) -> Result<Option<*const c_void>, i32> {
        if role == "AXButton" {
            for attribute in ["AXDefaultButton", "AXCancelButton"] {
                if let Some(button) = copy_attr(window, attribute)? {
                    let title = attr_string(button.get(), "AXTitle")?;
                    if title.as_deref() == Some(name) {
                        let ptr = button.get();
                        held.push(button);
                        return Ok(Some(ptr));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Bounded breadth-first walk from `window`: depth at most
    /// [`MAX_AX_DEPTH`], at most [`MAX_AX_ELEMENTS`] elements total. Returns
    /// leaked pointers (see [`button_leak`]); each is pressed or read
    /// synchronously before any guard in `held` could drop.
    fn bounded_search(
        window: *const c_void,
        role: &str,
        name: &str,
        held: &mut Vec<Cf>,
    ) -> Result<Vec<*const c_void>, i32> {
        // (element, depth) frontier. `window` itself is depth 0.
        let mut frontier = vec![(window, 0usize)];
        let mut visited = 0usize;
        let mut found = Vec::new();
        while let Some((element, depth)) = frontier.pop() {
            if visited >= MAX_AX_ELEMENTS {
                break;
            }
            visited += 1;
            if depth >= MAX_AX_DEPTH {
                continue;
            }
            let array = match copy_attr_array(element, "AXChildren") {
                Ok(array) => array,
                // Inaccessible subtrees (e.g. web views) are skipped, not fatal.
                Err(_) => continue,
            };
            let Some(array) = array else { continue };
            for index in 0..array_len(Some(&array)) {
                if visited + frontier.len() >= MAX_AX_ELEMENTS {
                    break;
                }
                let child = array_get(&array, index as isize);
                if child.is_null() {
                    continue;
                }
                match attr_string(child, "AXRole") {
                    Ok(Some(child_role)) if child_role == role => {
                        let title = attr_string(child, "AXTitle")?.unwrap_or_default();
                        let description = attr_string(child, "AXDescription")?.unwrap_or_default();
                        if title == name || description == name {
                            // Valid while `held` (which keeps `array`)
                            // outlives the synchronous action below.
                            found.push(child);
                        }
                    }
                    _ => {}
                }
                frontier.push((child, depth + 1));
            }
            held.push(array);
            if !found.is_empty() {
                break;
            }
        }
        Ok(found)
    }

    /// Elements matching (role, name) under `window`: direct attributes
    /// first, then the bounded walk. Pointers are valid while `held` lives
    /// and the window exists; every caller acts on them synchronously.
    pub(crate) fn lookup_elements(
        window: *const c_void,
        role: &str,
        name: &str,
        held: &mut Vec<Cf>,
    ) -> Result<Vec<*const c_void>, i32> {
        if let Some(element) = direct_lookup(window, role, name, held)? {
            return Ok(vec![element]);
        }
        bounded_search(window, role, name, held)
    }

    fn not_found(role: &str, name: &str) -> String {
        format!("AX element not found: {role} \"{name}\"")
    }

    pub(super) fn press(app: &str, role: &str, name: &str) -> Result<(), String> {
        if app == "focused" && pid_for_app(app).is_none() {
            return Err("no focused app".to_string());
        }
        let (windows, element) = find_window_element(&ResolvedWindow {
            label: name.to_string(),
            app: app.to_string(),
            title: None,
        })
        .map_err(|_| not_found(role, name))?;
        // Keep the window list alive: `element` and every lookup result
        // borrow from it (dropped at scope end, after the press).
        let _windows = windows;
        let mut held = Vec::new();
        let found =
            lookup_elements(element, role, name, &mut held).map_err(|_| not_found(role, name))?;
        let Some(target) = found.into_iter().next() else {
            return Err(not_found(role, name));
        };
        if perform(target, "AXPress") != AX_OK {
            return Err(not_found(role, name));
        }
        Ok(())
    }

    pub(super) fn set_value(app: &str, role: &str, name: &str, value: &str) -> Result<(), String> {
        if app == "focused" && pid_for_app(app).is_none() {
            return Err("no focused app".to_string());
        }
        let (windows, element) = find_window_element(&ResolvedWindow {
            label: name.to_string(),
            app: app.to_string(),
            title: None,
        })
        .map_err(|_| not_found(role, name))?;
        // Keep the window list alive (see `press`).
        let _windows = windows;
        let mut held = Vec::new();
        let found =
            lookup_elements(element, role, name, &mut held).map_err(|_| not_found(role, name))?;
        let Some(target) = found.into_iter().next() else {
            return Err(not_found(role, name));
        };
        let (Some(text), Some(attr)) = (cf_string(value), cf_string("AXValue")) else {
            return Err(not_found(role, name));
        };
        // SAFETY: target, name, and value are valid.
        let code = unsafe { AXUIElementSetAttributeValue(target, attr.get(), text.get()) };
        if code != AX_OK {
            return Err(not_found(role, name));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_bounds_are_strict() {
        assert_eq!(MAX_AX_DEPTH, 4);
        assert_eq!(MAX_AX_ELEMENTS, 300);
    }
}
