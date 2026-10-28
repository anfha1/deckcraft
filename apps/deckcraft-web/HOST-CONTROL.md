# Điều khiển DeckCraft web từ trang cha (`postMessage`)

Kênh điều khiển cho bản web, để một trang khác (console VCTA) nhúng DeckCraft trong `<iframe>`
và lái màn chiếu mà không cần app desktop.

## Gửi lệnh

```js
frame.contentWindow.postMessage({
  type: 'deckcraft:cmd',
  request: 'slide.go',      // TÊN LỆNH — engine chạy theo trường này
  id: 'c1',                 // mã tương quan của bạn, được trả lại nguyên vẹn
  params: JSON.stringify({ index: 2 }),
}, '*')
```

`params` là **chuỗi JSON** (không phải object), vì ranh giới WASM nhận chuỗi.

> ⚠️ `request` là tên lệnh, `id` chỉ để ghép kết quả. Bản trước tài liệu này ghi ngược hai trường
> và `web.rs` cũng chạy theo `id` — hậu quả: mọi lệnh trả `unknown command \`c1\`` và danh sách
> `HOST_DENIED` mất tác dụng. Khi viết smoke test, **đừng** đặt `id` trùng tên lệnh, nếu không lỗi
> này không bao giờ lộ.

## Chờ sẵn sàng

Trang HTML của iframe load xong rất sớm, nhưng wasm 30 MB còn phải khởi động (~1,2 s) rồi mới gắn
listener `message`. Lệnh gửi trước đó **mất im lặng** — `postMessage` không báo lỗi.

Engine phát một message khi đã sẵn sàng:

```js
{ type: 'deckcraft:ready' }
```

Chờ đúng message này rồi mới gửi lệnh đầu tiên. Đừng đoán bằng `load` + một khoảng delay.

## Nhận kết quả

```js
window.addEventListener('message', (e) => {
  if (e.source !== frame.contentWindow) return
  const d = e.data
  if (d.type !== 'deckcraft:result') return
  if (d.request !== 'slide.go' || d.id !== 'c1') return   // khớp cả tên lệnh lẫn mã
  console.log(d.ok ? d.data : d.error)
})
```

## Lệnh dùng được

Mọi lệnh của `slide`/`show`/`view` mà CLI có, ví dụ:

| Lệnh | Việc |
|---|---|
| `show.start` | vào chế độ trình chiếu từ slide đầu |
| `show.end` | thoát trình chiếu, về editor |
| `slide.go {index}` | nhảy tới slide (0-based) |
| `slide.next` / `slide.first` / `slide.last` | chuyển slide |
| `view.notes`, `view.grayscale`, … | như CLI |

Không có `slide.prev`: lùi một slide = `slide.go {index: index - 1}`.

**Bị chặn** (`file.new`, `file.close`, `window.close`, `app.quit`, `view.newWindow`) — trang cha không
được xoá tài liệu hay đóng tab.

## Bảo mật

Chỉ **parent window** gửi được (`Object.is(event.source, window.parent)`); trang khác nhúng
DeckCraft hoặc mở nó trực tiếp đều không điều khiển được. Không có cookie/token nào đi qua kênh này.

## Nạp file .pptx

`file.openBytes {name, data(base64)}` nạp thẳng bytes vào engine, rồi `show.start`. Không cần người
dùng bấm File → Open trong iframe.

## Build

```bash
# Rust + target wasm32-unknown-unknown + trunk
cd apps/deckcraft-web
trunk build --release           # ra dist/web
```

Trên Windows của máy dev này, toolchain nằm ngoài `%USERPROFILE%`:

```powershell
$env:RUSTUP_HOME='D:\toolchain\rust'; $env:CARGO_HOME='D:\toolchain\cargo'
$env:PATH='D:\toolchain\cargo\bin;D:\toolchain\trunk;'+$env:PATH
trunk build --release --dist D:\deckcraft\site --public-url /
```

Không có MSVC Build Tools thì dùng host toolchain GNU (`stable-x86_64-pc-windows-gnu`, đã là mặc định).
