//! Receiving files to open from outside the app.
//!
//! Files can arrive three different ways, at times we do not control:
//!
//!   * `optra photo.exr`      -- argv, before the event loop starts
//!   * dragged onto the window -- egui's `dropped_files`, mid-frame
//!   * double-clicked in Finder -- an `kAEOpenDocuments` Apple Event, which
//!     macOS may deliver *before* our `App` has been constructed
//!
//! So nothing here loads an image directly. Everything pushes onto a queue that
//! `App::update` drains once per frame, which keeps the timing question moot.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Used to wake the event loop when a file arrives while the app is idle.
static REPAINT: OnceLock<egui::Context> = OnceLock::new();

/// Queue a file to be opened on the next frame. Safe to call before the app exists.
pub fn queue(path: PathBuf) {
    PENDING.lock().unwrap().push(path);

    // egui sleeps when nothing is happening, so an Apple Event arriving while
    // the window is idle would otherwise sit in the queue unnoticed.
    if let Some(ctx) = REPAINT.get() {
        ctx.request_repaint();
    }
}

/// Take everything queued so far.
pub fn take() -> Vec<PathBuf> {
    std::mem::take(&mut *PENDING.lock().unwrap())
}

/// Give the queue a context to wake, once one exists.
pub fn set_repaint_context(ctx: egui::Context) {
    let _ = REPAINT.set(ctx);
}

#[cfg(target_os = "macos")]
mod platform {
    use super::queue;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::PathBuf;

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{ClassType, DeclaredClass, declare_class, msg_send, msg_send_id, mutability, sel};
    use objc2_app_kit::NSApplicationWillFinishLaunchingNotification;
    use objc2_foundation::{
        MainThreadMarker, NSAppleEventDescriptor, NSAppleEventManager, NSNotificationCenter,
        NSObject, NSObjectProtocol, NSString, NSURL,
    };

    // Classic Mac four-character codes, still how Apple Events are addressed.
    const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
    const AE_OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
    const KEY_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

    declare_class!(
        struct AppleEventHandler;

        unsafe impl ClassType for AppleEventHandler {
            type Super = NSObject;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "OptraAppleEventHandler";
        }

        impl DeclaredClass for AppleEventHandler {}

        unsafe impl NSObjectProtocol for AppleEventHandler {}

        unsafe impl AppleEventHandler {
            /// Apple's documented place to register Apple Event handlers: late
            /// enough that AppKit has installed its own defaults (so ours
            /// replaces them), early enough that the launch `odoc` event has not
            /// been dispatched yet.
            #[method(applicationWillFinishLaunching:)]
            fn application_will_finish_launching(&self, _notification: &AnyObject) {
                let _ = catch_unwind(AssertUnwindSafe(|| self.register_handler()));
            }

            /// Receives the `odoc` ("open documents") Apple Event that macOS
            /// sends on double-click / "Open With". The paths are NOT in argv.
            #[method(handleOpenDocuments:withReplyEvent:)]
            fn handle_open_documents(
                &self,
                event: &NSAppleEventDescriptor,
                _reply: &NSAppleEventDescriptor,
            ) {
                // This is an Objective-C entry point: a panic escaping here
                // would abort the process rather than unwind.
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    let list: Option<Retained<NSAppleEventDescriptor>> =
                        unsafe { msg_send_id![event, paramDescriptorForKeyword: KEY_DIRECT_OBJECT] };
                    let Some(list) = list else { return };

                    // Apple Event lists are 1-based.
                    for i in 1..=unsafe { list.numberOfItems() } {
                        let Some(item) = (unsafe { list.descriptorAtIndex(i) }) else {
                            continue;
                        };
                        let Some(value) = (unsafe { item.stringValue() }) else {
                            continue;
                        };
                        let value = value.to_string();

                        // Items arrive as file:// URLs; fall back to a raw path.
                        let path = if value.starts_with("file://") {
                            let ns = NSString::from_str(&value);
                            unsafe { NSURL::URLWithString(&ns) }
                                .and_then(|url| unsafe { url.path() })
                                .map(|p| PathBuf::from(p.to_string()))
                        } else {
                            Some(PathBuf::from(value))
                        };

                        if let Some(path) = path {
                            queue(path);
                        }
                    }
                }));
            }
        }
    );

    impl AppleEventHandler {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            unsafe { msg_send_id![super(mtm.alloc().set_ivars(())), init] }
        }

        fn register_handler(&self) {
            let manager = unsafe { NSAppleEventManager::sharedAppleEventManager() };
            unsafe {
                let _: () = msg_send![
                    &*manager,
                    setEventHandler: self,
                    andSelector: sel!(handleOpenDocuments:withReplyEvent:),
                    forEventClass: CORE_EVENT_CLASS,
                    andEventID: AE_OPEN_DOCUMENTS,
                ];
            }
        }
    }

    /// Arrange to receive the `odoc` (open documents) Apple Event.
    ///
    /// We deliberately do NOT install an `NSApplicationDelegate`: winit 0.30's
    /// docs claim it registers none, but 0.30.12 registers
    /// `WinitApplicationDelegate` and panics if anything replaces it.
    ///
    /// Timing is the whole difficulty. AppKit installs its own `odoc` handler
    /// (the one that shows "Optra cannot open files in the OpenEXR Image
    /// format") while launching, and dispatches the launch event before the
    /// first frame -- so registering from `App::new` is already too late. We
    /// therefore observe `NSApplicationWillFinishLaunchingNotification`, which
    /// is Apple's documented hook for exactly this, and register from there.
    ///
    /// Call from `main`, on the main thread, before the event loop starts.
    pub fn install() {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("Not on the main thread; Finder file-opening disabled");
            return;
        };

        let handler = AppleEventHandler::new(mtm);

        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &handler,
                sel!(applicationWillFinishLaunching:),
                Some(NSApplicationWillFinishLaunchingNotification),
                None,
            );
        }

        // Both the notification centre and the event manager hold the handler
        // weakly, so this must outlive the call.
        std::mem::forget(handler);
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn install() {}
}

pub use platform::install;
