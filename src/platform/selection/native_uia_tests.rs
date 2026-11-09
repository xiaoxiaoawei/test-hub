//! Real Windows UIA integration, with a live STA control owner and MTA client.
//! This never calls the native EDIT capture shortcut or uses COM test doubles.

use super::{chain_text, probe, range_matches_gesture, selected_text, UiaNode, UiaSession};
use std::{
    mem::size_of,
    ptr,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::Win32::{
    System::{
        Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
        Ole::{
            SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetElemsize,
            SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayGetVartype, SafeArrayUnaccessData,
        },
        Variant::VT_R8,
    },
    UI::Accessibility::{
        IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange,
        SupportedTextSelection_None, UIA_TextPatternId,
    },
};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, GetLastError, HWND},
    System::{
        LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
        Threading::GetCurrentThreadId,
    },
    UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, PostThreadMessageW,
        SendMessageTimeoutW, SendMessageW, TranslateMessage, ES_MULTILINE, ES_NOHIDESEL,
        ES_PASSWORD, MSG, SMTO_ABORTIFHUNG, SMTO_BLOCK, WM_QUIT, WS_CHILD, WS_CLIPCHILDREN,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
    },
};

const SOURCE: &str = "before native selection after\r\nsecond line";
const EXPECTED: &str = "native selection";
const START: usize = 7;
const END: isize = 23;
const EM_SETSEL: u32 = 0x00B1;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn is_wine() -> bool {
    // Wine 10 returns E_NOTIMPL even for ElementFromHandleBuildCache. Do not
    // turn missing UIA functionality on native Windows into a successful skip.
    unsafe {
        let ntdll = GetModuleHandleW(wide("ntdll.dll").as_ptr());
        !ntdll.is_null() && GetProcAddress(ntdll, c"wine_get_version".as_ptr().cast()).is_some()
    }
}

#[derive(Clone, Copy)]
struct Windows {
    rich: usize,
    edit: usize,
    password: usize,
    thread: u32,
}

struct Fixture {
    windows: Windows,
    owner: Option<JoinHandle<()>>,
}

impl Fixture {
    fn start() -> Self {
        let (ready, result) = mpsc::sync_channel(1);
        let owner = thread::Builder::new()
            .name("native-uia-sta-owner".into())
            .spawn(move || unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                    .ok()
                    .expect("initialize the real control owner's STA");
                let module = LoadLibraryW(wide("Msftedit.dll").as_ptr());
                assert!(!module.is_null(), "load Msftedit.dll: {}", GetLastError());
                let root = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    wide("STATIC").as_ptr(),
                    wide("LingoLeaf real UIA fixture").as_ptr(),
                    WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN,
                    40,
                    40,
                    620,
                    300,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    GetModuleHandleW(ptr::null()),
                    ptr::null(),
                );
                assert!(!root.is_null(), "create UIA fixture: {}", GetLastError());
                let rich = make_control(root, "RICHEDIT50W", 10, false);
                let edit = make_control(root, "EDIT", 105, false);
                let password = make_control(root, "EDIT", 200, true);
                for window in [rich, edit] {
                    SendMessageW(window, EM_SETSEL, START, END);
                }
                ready
                    .send(Windows {
                        rich: rich as usize,
                        edit: edit as usize,
                        password: password as usize,
                        thread: GetCurrentThreadId(),
                    })
                    .expect("publish live native controls");
                let mut message: MSG = std::mem::zeroed();
                while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                DestroyWindow(root);
                FreeLibrary(module);
                CoUninitialize();
            })
            .expect("start the real control owner's STA thread");
        let windows = result
            .recv_timeout(Duration::from_secs(10))
            .expect("the real native UIA fixture must start; failure is not a skip");
        Self {
            windows,
            owner: Some(owner),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.windows.thread, WM_QUIT, 0, 0);
        }
        if let Some(owner) = self.owner.take() {
            owner
                .join()
                .expect("native UIA fixture owner exited cleanly");
        }
    }
}

unsafe fn make_control(root: HWND, class: &str, y: i32, password: bool) -> HWND {
    let window = unsafe {
        CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(SOURCE).as_ptr(),
            WS_CHILD
                | WS_VISIBLE
                | ES_NOHIDESEL as u32
                | if password {
                    ES_PASSWORD as u32
                } else {
                    ES_MULTILINE as u32
                },
            10,
            y,
            590,
            85,
            root,
            ptr::null_mut(),
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        )
    };
    assert!(!window.is_null(), "create live {class}: {}", unsafe {
        GetLastError()
    });
    window
}

unsafe fn cached_node(session: &UiaSession, window: usize, class: &str) -> UiaNode {
    let element: IUIAutomationElement = unsafe {
        session.automation.ElementFromHandleBuildCache(
            windows::Win32::Foundation::HWND(window as _),
            &session.cache,
        )
    }
    .unwrap_or_else(|error| panic!("real {class} ElementFromHandleBuildCache failed: {error}"));
    let node = unsafe { UiaNode::cached(element) };
    eprintln!(
        "real UIA {class}: hwnd={:#x}, cached pid={}, password={:?}, text={:?}, text2={:?}, bounds={}",
        window,
        node.pid,
        node.password,
        node.text_available,
        node.text2_available,
        node.metadata.bounds.is_some(),
    );
    assert_eq!(
        node.native as usize, window,
        "cached native HWND for {class}"
    );
    assert!(node.pid > 0, "cached process identity for {class}");
    assert!(
        node.metadata.edit || node.metadata.document,
        "real {class} exposes a UIA text control type"
    );
    node
}

unsafe fn first_rectangle(range: &IUIAutomationTextRange) -> [f64; 4] {
    let array = unsafe { range.GetBoundingRectangles() }
        .expect("real selected range must return its UIA screen geometry");
    assert!(!array.is_null());
    unsafe {
        assert_eq!(SafeArrayGetDim(array), 1);
        assert_eq!(SafeArrayGetElemsize(array), size_of::<f64>() as u32);
        assert_eq!(SafeArrayGetVartype(array).unwrap(), VT_R8);
        let lower = SafeArrayGetLBound(array, 1).unwrap();
        let upper = SafeArrayGetUBound(array, 1).unwrap();
        let length = i64::from(upper) - i64::from(lower) + 1;
        assert!((4..=4096).contains(&length) && length % 4 == 0);
        let mut data = ptr::null_mut();
        SafeArrayAccessData(array, &mut data).unwrap();
        assert!(!data.is_null());
        let rectangle: [f64; 4] = std::slice::from_raw_parts(data.cast::<f64>(), 4)
            .try_into()
            .unwrap();
        SafeArrayUnaccessData(array).unwrap();
        SafeArrayDestroy(array).unwrap();
        assert!(rectangle.iter().all(|value| value.is_finite()));
        assert!(rectangle[2] > 0.0 && rectangle[3] > 0.0);
        rectangle
    }
}

#[test]
fn native_windows_uia_reads_live_selection_geometry_and_password_metadata() {
    if is_wine() {
        eprintln!("SKIP only on detected Wine: real UIA BuildCache/Pattern APIs are E_NOTIMPL; this is not Windows UIA validation.");
        return;
    }
    let fixture = Fixture::start();
    // UiaSession initializes this test's client thread as MTA; the window owner
    // remains in its own pumping STA for all cross-thread UIA provider calls.
    let session = UiaSession::new().expect("initialize real Windows UIA on the MTA client");
    let mut readable = 0;
    for (class, window) in [
        ("RICHEDIT50W", fixture.windows.rich),
        ("EDIT (real UIA proxy)", fixture.windows.edit),
    ] {
        let node = unsafe { cached_node(&session, window, class) };
        assert_eq!(node.password, Some(false), "cached {class} password safety");
        assert!(!unsafe { node.element.CurrentIsPassword() }
            .unwrap_or_else(|error| panic!("real {class} CurrentIsPassword failed: {error}"))
            .as_bool());
        let pattern = match unsafe {
            node.element
                .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        } {
            Ok(pattern) => pattern,
            Err(error) => {
                eprintln!("real UIA {class}: TextPattern unavailable: {error}; metadata alone is not a capture pass");
                assert_ne!(
                    node.text_available,
                    Some(true),
                    "{class} advertises TextPattern but cannot return it: {error}"
                );
                continue;
            }
        };
        assert_ne!(
            unsafe { pattern.SupportedTextSelection() }.unwrap(),
            SupportedTextSelection_None
        );
        let ranges = unsafe { pattern.GetSelection() }.expect("real UIA GetSelection");
        assert_eq!(
            unsafe { ranges.Length() }.unwrap(),
            1,
            "{class} selected range count"
        );
        let range = unsafe { ranges.GetElement(0) }.unwrap();
        assert_eq!(unsafe { range.GetText(128) }.unwrap().to_string(), EXPECTED);
        let rectangle = unsafe { first_rectangle(&range) };
        let point = (
            (rectangle[0] + rectangle[2] / 2.0) as i32,
            (rectangle[1] + rectangle[3] / 2.0) as i32,
        );
        let gesture = probe::Gesture {
            origin: Some(point),
            end: point,
        };
        assert_eq!(
            unsafe { range_matches_gesture(&range, gesture) },
            Some(true)
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        assert_eq!(
            unsafe { selected_text(&pattern, Some(gesture), deadline) }
                .unwrap()
                .as_deref(),
            Some(EXPECTED)
        );
        assert_eq!(
            unsafe { chain_text(std::slice::from_ref(&node), None, Some(gesture), deadline) }
                .unwrap()
                .as_deref(),
            Some(EXPECTED)
        );
        let mut delivered = 0;
        assert_ne!(
            unsafe {
                SendMessageTimeoutW(
                    window as HWND,
                    EM_SETSEL,
                    START,
                    START as isize,
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    1000,
                    &mut delivered,
                )
            },
            0,
            "collapse the real {class} selection"
        );
        assert_eq!(
            unsafe { selected_text(&pattern, None, Instant::now() + Duration::from_secs(3)) }
                .unwrap(),
            None,
            "collapsed {class} caret must not become document text"
        );
        readable += 1;
        eprintln!("real UIA {class}: exact selected text, native range geometry, production chain reader, and empty-selection rejection passed");
    }
    assert!(readable > 0, "neither live RichEdit nor the native EDIT UIA proxy exposed readable TextPattern; the native capture integration is unverified and must fail");
    let password = unsafe { cached_node(&session, fixture.windows.password, "password EDIT") };
    assert_eq!(password.password, Some(true));
    assert!(unsafe { password.element.CurrentIsPassword() }
        .unwrap()
        .as_bool());
    assert!(
        matches!(
            unsafe {
                chain_text(
                    &[password],
                    None,
                    None,
                    Instant::now() + Duration::from_secs(3),
                )
            },
            Err(super::UiaFailure::Protected)
        ),
        "production UIA chain reader must reject the real password control"
    );
}
