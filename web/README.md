# GolemSculpt on the web

The browser version: the desktop app's own sculpting engine (its `src/mesh`,
`src/sculpt`, `src/camera.rs` and `src/remesh.rs`, compiled here unchanged,
plus the tool crates for Tubes, Muscles and Straps) with a small panel, a
small renderer and files in and out through the browser. The plan and what
was left out are in `docs/web-plan.md`.

## Build it

Once, on a new machine:

```
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129
rustup toolchain install nightly --component rust-src --target wasm32-unknown-unknown
```

(The nightly compiler is only for the many-threads build: WebAssembly
threads need Rust's standard library rebuilt, which only nightly does.)

Then, from the repository's top folder:

```
powershell -ExecutionPolicy Bypass -File web\build.ps1
```

That makes `web\dist`, the whole website version, with three builds of the
app side by side. A visitor downloads only one: `index.html` picks it.

| File | What it is | Size (compressed) |
| --- | --- | --- |
| `index.html` | The page: loading screen, then picks a build | |
| `golem_web_mt.js`, `golem_web_mt_bg.wasm`, `snippets\` | Many threads, WebGPU; where the page is cross-origin isolated (below) and the browser has WebGPU | 9.8 MB (3.6 MB) |
| `golem_web_gpu.js`, `golem_web_gpu_bg.wasm` | One thread, WebGPU; where the browser has WebGPU | 9.8 MB (3.6 MB) |
| `golem_web.js`, `golem_web_bg.wasm` | One thread, WebGPU or WebGL2; every other browser, and the fallback if another build will not load | 12.3 MB (4.5 MB) |

The WebGL2 build is a fifth bigger because drawing with WebGL2 brings
wgpu's own WebGPU implementation and shader translator along; a browser
with WebGPU has those built in.

`-Quick` builds only `golem_web` (it runs everywhere, so it is enough for
trying a change) and removes the other two from the folder; `-NoThreads`
skips just the many-threads build, the slowest to make (it rebuilds Rust's
standard library). `-Dev` builds quicker still, with bigger, slower files.
`-Name x` builds into `web\dist-x` with its own target folders, so several
people can build at once.

Try it on this computer with `python web\dist\serve.py`, then open
http://localhost:8642. `python web\dist\serve.py 8643 --one-thread` serves
it without the two headers, to try the one-thread build. Opening
`index.html` straight from the disk does not work: browsers only run
WebAssembly a server hands them.

## Put it on a website

Upload everything in `web\dist` except `serve.py` (the `snippets` folder
too) to one folder on the site, say `/sculpt/`, and link to
`/sculpt/index.html`.

`THIRD-PARTY-LICENSES.txt` must go up with it: the libraries' licences ask
for their notices to travel with the program, and the Settings tab's About
card opens it. `build.ps1` writes it fresh each build (`web\licences.py`,
from `cargo metadata`), so it always matches what was built.

What the server needs to do, which almost every host already does:

- Send `.wasm` files as `application/wasm` (else the browser cannot compile
  the download while it arrives, and starting takes longer).
- Compress with gzip or Brotli (cuts the download from about 10 MB to about
  3.6; Brotli does a little better than gzip).
- HTTPS. Browsers only offer WebGPU, pen pressure and threads on secure
  pages (and on localhost).

### The two headers for the many-threads build

The many-threads build uses every core for brushes, Remesh and Subdivide
(on this computer: Remesh twice as fast, and the pauses a big brush makes
on a 1.4M-triangle model five times shorter; see below). Browsers only allow a page threads that share memory
when the page is "cross-origin isolated", which the server turns on by
sending these two headers with `index.html` (sending them with every file
in the folder is simplest and harmless):

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

Without them nothing breaks: the page sees it is not isolated and loads the
one-thread build, which works everywhere, only slower on big models. The
browser's console says which build is running ("GolemSculpt: golem_web_mt,
16 threads").

How to send them depends on the host:

- Apache (most shared hosting): a file called `.htaccess` in the same folder,
  containing

  ```
  Header set Cross-Origin-Opener-Policy "same-origin"
  Header set Cross-Origin-Embedder-Policy "require-corp"
  AddType application/wasm .wasm
  ```

- Netlify or Cloudflare Pages: a file called `_headers` in the folder,
  containing

  ```
  /*
    Cross-Origin-Opener-Policy: same-origin
    Cross-Origin-Embedder-Policy: require-corp
  ```

- nginx: `add_header Cross-Origin-Opener-Policy same-origin;` and
  `add_header Cross-Origin-Embedder-Policy require-corp;` in the folder's
  `location` block.

What the headers change for the rest of the page: `require-corp` stops it
loading pictures, scripts or frames from other sites unless those sites
allow it. The app's page loads nothing from other sites, so it is
unaffected; send the headers only for the app's folder, not the whole site,
so other pages (with videos, adverts or embeds from elsewhere) are not
touched.

Shown inside another page with an iframe, the app gets threads only if the
outer page is isolated too (it sends the same two headers) and the iframe
allows it:

```html
<iframe src="/sculpt/index.html" style="width:100%;height:80vh;border:0"
        allow="fullscreen; cross-origin-isolated"></iframe>
```

Otherwise it runs, one-thread. Linking straight to `/sculpt/index.html`
avoids the question.

### itch.io

Zip the contents of `web\dist` (not the folder itself, and without
`serve.py`; keep `snippets` inside), upload it as an HTML game, tick "This
file will be played in the browser", and tick "SharedArrayBuffer support"
under Embed options: that makes itch.io send the two headers, so the
many-threads build is used. Without the tick the one-thread build runs.

Before a public launch, add a third-party licence file for the Rust crates
compiled in (as the desktop itch.io zip has). The two egui fonts kept
(`slim-fonts/fonts`) carry their own licences.

## What it runs on

Chrome, Edge and recent Safari and Firefox. WebGPU is used where the browser
has it, WebGL2 otherwise. A model is kept in the browser between visits
(IndexedDB), per browser and per site.

Measured 2026-11-15 on this machine (i9-13900K, Chrome), the many-threads
build with 16 threads against the one-thread WebGPU build, same files,
same steps, back to back:

| | One thread | 16 threads |
| --- | --- | --- |
| Remesh at 384 (120k-triangle sphere to 1.39M triangles) | 0.71-0.74 s | 0.31-0.35 s |
| Clay Strips, 334 px, Mirror X, on 1.39M triangles: an ordinary frame | 1.0 ms | 1.2 ms |
| the same stroke: the pause every half second or so | 230-290 ms | 40-55 ms |
| the same stroke: letting go | 18-21 ms | 6-8 ms |
| main-thread time for a 2.5-second stroke | 1.5-1.7 s | 0.87-1.08 s |

Ordinary frames are quick either way; threads take the long pauses down.
A small brush (60 px) is smooth either way.

## Keeping it building

`src/lib.rs` pulls the desktop's files in with `#[path]`. If a desktop change
makes `src/mesh` or `src/sculpt` reach into the rest of the desktop app
(`crate::scene`, `crate::app`, ...), this stops compiling. Check with:

```
cd web
cargo check --target wasm32-unknown-unknown --target-dir ../target/w/web
```

and the WebGPU-only and many-threads builds (which leave out WebGL2, and
change how rayon and the memory work) with:

```
cargo check --target wasm32-unknown-unknown --target-dir ../target/w/web-gpu --no-default-features
$env:RUSTFLAGS = '-C target-feature=+atomics,+bulk-memory,+mutable-globals'
cargo +nightly check --target wasm32-unknown-unknown --target-dir ../target/w/web-mt --no-default-features --features threads -Z build-std=std,panic_abort
$env:RUSTFLAGS = ''
```

(in PowerShell). Anything shared between threads must be `Send` and `Sync`
there; wgpu's own objects are not, so they stay on the page's main thread,
as they do now.

The desktop files carry a few `#[cfg(target_arch = "wasm32")]` lines for
this build (the clock in `src/sculpt`, tool jobs run in place in
`crates/toolkit/src/jobs.rs`, tooltip translations fetched by the
page rather than built in, `crates/toolkit/src/translate.rs` and
`fonts.rs`); they change nothing on the desktop.
