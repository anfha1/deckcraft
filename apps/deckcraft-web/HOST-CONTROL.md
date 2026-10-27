# Điều khiển DeckCraft web từ trang cha (`postMessage`)

Bản vá này thêm một kênh điều khiển cho bản web, để một trang khác (console VCTA) nhúng
DeckCraft trong `<iframe>` và lái màn chiếu mà không cần app desktop.

## Gửi lệnh

```js
frame.contentWindow.postMessage({
  type: 'deckcraft:cmd',
  request: 'r1',            // id của bạn, được trả lại nguyên vẹn
  id: 'slide.go',           // cùng tên lệnh với deckcraft-cli
  params: JSON.stringify({ index: 2 }),
}, '*')
```

`params` là **chuỗi JSON** (không phải object), vì ranh giới WASM nhận chuỗi.

## Nhận kết quả

```js
window.addEventListener('message', (e) => {
  if (e.source !== frame.contentWindow) return
  const d = e.data
  if (d.type !== 'deckcraft:result') return
  console.log(d.request, d.ok, d.ok ? d.data : d.error)
})
```

## Lệnh dùng được

Mọi lệnh của `slide`/`show`/`view` mà CLI có, ví dụ:

| Lệnh | Việc |
|---|---|
| `show.start` | vào chế độ trình chiếu từ slide đầu |
| `show.end` | thoát trình chiếu, về editor |
| `slide.go {index}` | nhảy tới slide (0-based) |
| `slide.next` / `slide.prev` | slide kế / trước |
| `view.notes`, `view.grayscale`, … | như CLI |

**Bị chặn** (`file.new`, `file.close`, `window.close`, `app.quit`, `view.newWindow`) — trang cha không
được xoá tài liệu hay đóng tab.

## Bảo mật

Chỉ **parent window** gửi được (`Object.is(event.source, window.parent)`); trang khác nhúng
DeckCraft hoặc mở nó trực tiếp đều không điều khiển được. Không có cookie/token nào đi qua kênh này.

## Nạp file .pptx

Hiện phải để người dùng bấm File → Open **trong iframe** (dùng file picker của trình duyệt).
Chưa có lệnh nhận bytes qua `postMessage`; nếu cần, thêm một nhánh vào `drain` gọi
`SlideApp::open_or_insert_bytes(name, bytes)`.

## Build

```bash
# Rust + target wasm32-unknown-unknown + trunk + wasm-bindgen 0.2.129
cd apps/deckcraft-web
trunk build --release     # ra dist/web
```

Trên Windows không có MSVC Build Tools, dùng host toolchain GNU:

```bash
cargo +stable-x86_64-pc-windows-gnu build --target wasm32-unknown-unknown --release -p deckcraft-web
wasm-bindgen --target web --out-dir <site>/deckcraft-web \
  target/wasm32-unknown-unknown/release/deckcraft-web.wasm
```
