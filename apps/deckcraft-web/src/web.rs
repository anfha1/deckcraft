//! The browser shell: web `Services`, drag-and-drop, and the eframe web runner.

use deckcraft_engine::Session;
use deckcraft_ui_egui::{Inbox, InboxEvent, InboxFile, Services, SlideApp};
use wasm_bindgen::JsCast as _;

const PRESENTATION_EXTS: &[&str] = &["deckcraft", "pptx", "potx", "ppsx", "txt", "md"];
const PICTURE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"];
const AUDIO_EXTS: &[&str] = &["wav", "mp3", "m4a", "aac", "flac", "ogg", "opus", "aif", "aiff"];
const VIDEO_EXTS: &[&str] = &["mp4", "m4v", "mov", "webm"];
const CANVAS_ID: &str = "deckcraft_canvas";
const LOADING_ID: &str = "deckcraft_loading";

/// Start the browser shell: install logging, locate the canvas, launch the eframe runner,
/// build the sample or blank deck, and replace the loading element when launch finishes.
pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{CANVAS_ID}\">");
            return;
        };
        let mut options = eframe::WebOptions::default();
        if query().contains("webgl")
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
        {
            create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(move |cc| {
                    if let Some(rs) = &cc.wgpu_render_state {
                        log::info!("deckcraft-web {}: wgpu backend {:?}", env!("CARGO_PKG_VERSION"), rs.adapter.get_info().backend);
                    }
                    deckcraft_ui_egui::install_ime_pass_hooks(&cc.egui_ctx);
                    let inbox: Inbox = Inbox::default();
                    let mut app = SlideApp::new(Session::new(), services(inbox.clone(), cc.egui_ctx.clone()));
                    let q = query();
                    if !q.contains("blank") {
                        if let Err(e) = deckcraft_engine::sample::open_sample(&mut app.session) {
                            log::error!("sample deck: {e}");
                        }
                    } else if let Err(e) = app.run("file.new", serde_json::json!({})) {
                        log::error!("new presentation: {e}");
                    }
                    if q.contains("show") {
                        app.start_show(0, false);
                    }
                    CTX.with(|c| *c.borrow_mut() = Some(cc.egui_ctx.clone()));
                    let queue: PendingQueue = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                    listen(queue.clone());
                    // The embedder cannot tell when the wasm bridge starts listening: the frame's
                    // `load` event fires long before the 30 MB wasm boots. Announce it, so a host
                    // page can send commands immediately instead of guessing a delay.
                    announce_ready();
                    Ok(Box::new(WebShell { app, inbox, queue }))
                }),
            )
            .await;
        if let Some(el) = document.get_element_by_id(LOADING_ID) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => el.set_inner_html(&format!("<p>DeckCraft failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>")),
            }
        }
    });
}

fn query() -> String {
    web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}

/// One command posted by the embedding page, applied on the next logic pass.
struct Pending {
    /// Caller's correlation id, echoed back in the result.
    request: String,
    /// Command id, the same verbs the CLI uses (`show.start`, `slide.go`, ...).
    id: String,
    params: serde_json::Value,
}

/// `postMessage` protocol, both directions. The page sends
/// `{type:"deckcraft:cmd", request, id, params}`; DeckCraft answers
/// `{type:"deckcraft:result", request, ok, data|error}`.
const MSG_CMD: &str = "deckcraft:cmd";
const MSG_RESULT: &str = "deckcraft:result";
/// Sent once, to the embedding page, the moment the command listener is installed.
const MSG_READY: &str = "deckcraft:ready";

/// Commands the host page may not drive: anything that would close the tab, download, or start a
/// second slide show. Everything else goes through `SlideApp::run`.
const HOST_DENIED: &[&str] = &["file.new", "file.close", "window.close", "app.quit", "view.newWindow"];

type PendingQueue = std::rc::Rc<std::cell::RefCell<Vec<Pending>>>;

// The egui context, so the `message` listener can wake the event loop: eframe only runs a pass
// when something asks for one, and a message arriving while the app is idle would otherwise sit
// in the queue until the user moved the mouse.
thread_local! {
    static CTX: std::cell::RefCell<Option<egui::Context>> = const { std::cell::RefCell::new(None) };
}

/// Install the `message` listener. Only the embedding page (our parent) may drive us: Chrome hands
/// `event.source` back as a cross-realm proxy, so identity is compared with `Object.is` against the
/// `Window` we hold, not by downcasting it.
fn listen(queue: PendingQueue) {
    let Some(window) = web_sys::window() else { return };
    let parent = window.parent().ok().flatten();
    let sink = queue;
    let handler = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
        let from: wasm_bindgen::JsValue = match ev.source() {
            Some(o) => o.into(),
            None => wasm_bindgen::JsValue::NULL,
        };
        let src_ok = match &parent {
            Some(p) => js_sys::Object::is(&wasm_bindgen::JsValue::from(p.clone()), &from),
            // No parent (opened directly): only a same-window message could be ours.
            None => js_sys::Object::is(&from, &wasm_bindgen::JsValue::NULL),
        };
        if !src_ok {
            return;
        }
        let Ok(obj) = ev.data().dyn_into::<js_sys::Object>() else { return };
        let field = |k: &str| js_sys::Reflect::get(&obj, &wasm_bindgen::JsValue::from_str(k)).ok();
        if field("type").and_then(|v| v.as_string()).as_deref() != Some(MSG_CMD) {
            return;
        }
        let request = field("request").and_then(|v| v.as_string()).unwrap_or_default();
        let id = field("id").and_then(|v| v.as_string()).unwrap_or_default();
        if id.is_empty() {
            return;
        }
        let params = field("params").and_then(|v| v.as_string()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(serde_json::Value::Null);
        sink.borrow_mut().push(Pending { request, id, params });
        CTX.with(|c| {
            if let Some(ctx) = c.borrow().as_ref() {
                ctx.request_repaint();
            }
        });
    });
    window.add_event_listener_with_callback("message", handler.as_ref().unchecked_ref()).ok();
    // The page lives as long as the tab; leaking one closure is the point.
    handler.forget();
}

/// Tell the embedding page that commands are accepted from now on. Sent once, after `listen`.
fn announce_ready() {
    let Some(window) = web_sys::window() else { return };
    let target = window.parent().ok().flatten().unwrap_or(window.clone());
    let msg = js_sys::Object::new();
    js_sys::Reflect::set(&msg, &wasm_bindgen::JsValue::from_str("type"), &wasm_bindgen::JsValue::from_str(MSG_READY)).ok();
    target.post_message(&msg, "*").ok();
}

/// Answer one queued command from `logic`, where the app is `&mut`.
fn drain(app: &mut SlideApp, queue: &PendingQueue, ctx: &egui::Context) {
    let batch = std::mem::take(&mut *queue.borrow_mut());
    if batch.is_empty() {
        return;
    }
    let Some(window) = web_sys::window() else { return };
    // Answer the embedder, not ourselves: a page that frames DeckCraft listens on its own window.
    let target = window.parent().ok().flatten().unwrap_or(window.clone());
    for p in batch {
        // `id` is the page's correlation token, `request` is the command name: dispatch on `request`.
        let denied = HOST_DENIED.contains(&p.request.as_str());
        let result = if denied { Err(format!("command {} is not available to the host page", p.request)) } else { app.run(&p.request, p.params) };
        let msg = js_sys::Object::new();
        let set = |k: &str, v: wasm_bindgen::JsValue| js_sys::Reflect::set(&msg, &wasm_bindgen::JsValue::from_str(k), &v).ok();
        set("type", wasm_bindgen::JsValue::from_str(MSG_RESULT));
        set("request", wasm_bindgen::JsValue::from_str(&p.request));
        set("id", wasm_bindgen::JsValue::from_str(&p.id));
        match result {
            Ok(data) => {
                set("ok", wasm_bindgen::JsValue::TRUE);
                set("data", js_sys::JSON::parse(&data.to_string()).unwrap_or(wasm_bindgen::JsValue::NULL));
            }
            Err(e) => {
                set("ok", wasm_bindgen::JsValue::FALSE);
                set("error", wasm_bindgen::JsValue::from_str(&e));
            }
        }
        target.post_message(&msg, "*").ok();
        ctx.request_repaint();
    }
}

/// Wraps the app to read dropped files asynchronously (browsers can't read them synchronously)
/// and feed them through the inbox.
struct WebShell {
    app: SlideApp,
    inbox: Inbox,
    queue: PendingQueue,
}

impl eframe::App for WebShell {
    /// Delegate one eframe logic pass to the wrapped application.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.logic(ctx);
    }

    /// Consume browser drops with fresh input, capture the identity before the asynchronous read,
    /// and deliver the id, name, and bytes through the inbox.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Commands posted by the embedding page land here, not in `logic`: whichever app hook runs
        // first owns them, and the host must not need the tab to be painting to be answered.
        drain(&mut self.app, &self.queue, ctx);
        // Browsers cannot read dropped files synchronously. Consume this pass's drops here, with
        // fresh input, and capture the document/slide before the asynchronous read finishes.
        for f in std::mem::take(&mut raw.dropped_files) {
            let id = self.app.begin_external_file("drop");
            let inbox = self.inbox.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let name = f.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dropped".into());
                match f.bytes_async().await {
                    Ok(bytes) => {
                        push_inbox(&inbox, InboxEvent::File(InboxFile { id, name, bytes }));
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        log::error!("couldn't read dropped file {name}: {e}");
                        push_inbox(&inbox, InboxEvent::Cancelled { id });
                        ctx.request_repaint();
                    }
                }
            });
        }
        self.app.raw_input_hook(ctx, raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }
}

/// Web services: no file paths, so `pick_open`/`pick_save`/`read` stay `None` and the UI falls
/// back to `open_async` (browser picker → inbox) and `download`.
fn services(inbox: Inbox, ctx: egui::Context) -> Services {
    let open_inbox = inbox.clone();
    Services {
        open_async: Some(Box::new(move |purpose: &str, request: u64| {
            let inbox = open_inbox.clone();
            let ctx = ctx.clone();
            let dialog = match purpose {
                "picture" => rfd::AsyncFileDialog::new().add_filter("Pictures", PICTURE_EXTS),
                "audio" => rfd::AsyncFileDialog::new().add_filter("Audio", AUDIO_EXTS),
                "video" => rfd::AsyncFileDialog::new().add_filter("Video", VIDEO_EXTS),
                _ => rfd::AsyncFileDialog::new().add_filter("Presentations", PRESENTATION_EXTS),
            };
            wasm_bindgen_futures::spawn_local(async move {
                let Some(file) = dialog.pick_file().await else {
                    push_inbox(&inbox, InboxEvent::Cancelled { id: request });
                    ctx.request_repaint();
                    return;
                };
                let bytes = file.read().await;
                push_inbox(&inbox, InboxEvent::File(InboxFile { id: request, name: file.file_name(), bytes }));
                ctx.request_repaint();
            });
        })),
        write: Some(Box::new(|name: &str, bytes: &[u8]| download(name, bytes))),
        download: Some(Box::new(|name: &str, bytes: &[u8]| {
            if let Err(e) = download(name, bytes) {
                log::error!("download of {name} failed: {e}");
            }
        })),
        inbox: Some(inbox),
        ..Default::default()
    }
}

fn push_inbox(inbox: &Inbox, event: InboxEvent) {
    inbox.lock().unwrap_or_else(|e| e.into_inner()).push(event);
}

/// Trigger a browser download of `bytes` named after the last component of `path`.
fn download(path: &str, bytes: &[u8]) -> Result<(), String> {
    let js = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "deckcraft".into());
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime_for(&name));
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(js)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js)?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(js)?.dyn_into().map_err(|_| "not an anchor")?;
    a.set_href(&url);
    a.set_download(&name);
    a.style().set_property("display", "none").map_err(js)?;
    let body = document.body().ok_or("no body")?;
    body.append_child(&a).map_err(js)?;
    a.click();
    a.remove();
    // Revoke after the click has been dispatched; the download keeps its own reference.
    let revoke = wasm_bindgen::closure::Closure::once_into_js(move || {
        web_sys::Url::revoke_object_url(&url).ok();
    });
    window.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000).map_err(js)?;
    Ok(())
}

fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("pdf") => "application/pdf",
        Some("gif") => "image/gif",
        Some("txt") => "text/plain",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Some("deckcraft") => "application/zip",
        _ => "application/octet-stream",
    }
}
