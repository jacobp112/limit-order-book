# explainer

A step-through inspector that runs the real engine in the browser.

```
cd explainer
python -m http.server 8000      # then open http://localhost:8000
```

(Any static file server works; the page must be served over HTTP so it can
fetch `lob.wasm`.)

Load an example journal or type journal lines (`submit buy limit 101 5`,
`cancel 3`, `amend 3 - 4`, the same syntax as `lob replay`). **Step event**
applies the next event the engine returned for the current command and
highlights the order it touches; **Next command** finishes the current
command; **Run** steps on a timer.

| File | Purpose |
|---|---|
| `lob.wasm` | `crates/lob-wasm` built with `--profile wasm` |
| `lob.mjs` | wrapper around the wasm exports (write a journal line, read JSON) |
| `model.mjs` | the displayed book, updated from engine events only |
| `app.js`, `index.html`, `style.css` | the page |
| `check.mjs` | Node check run in CI (see below) |

The page has no matching logic. Each command is applied by the engine in one
call; the page then replays the returned events against its copy of the book
(`model.mjs`) and, at the end of the command, compares that copy with the
book the engine reported.

`node explainer/check.mjs` loads `lob.wasm`, replays
`examples/journals/session.journal` and requires the same state and event
digests as `lob replay`, then replays every seed journal in
`fuzz/seeds/journal/` and requires `model.mjs` to match the engine after
every command. CI runs it against a freshly built module and against the
committed one.

Rebuild the module after changing the engine:

```
cargo build -p lob-wasm --profile wasm --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/wasm/lob_wasm.wasm explainer/lob.wasm
```
