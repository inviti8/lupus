# Lepus Connectors — Browser-side work to support Lupus tooling

**Status:** Planning — needs review before implementation
**Date:** 2026-04-09
**Audience:** Lepus maintainers / agents working in the `inviti8/lepus` repo
**Companion to:** `lupus/docs/LUPUS_TOOLS.md` (the daemon-side plan)
**Target file location after move:** `lepus/docs/LUPUS_CONNECTORS.md`

---

## 1. Why this doc exists

The Lupus daemon (`inviti8/lupus`) is finishing its tooling layer. The agent's `fetch_page` and `crawl_index` tools need to put bytes on the wire — fetch real web pages, fetch real HVYM datapod content. The agreed architecture is **delegation**: rather than the daemon adding its own HTTP client + Stellar SDK + tunnel client (a duplicate of `HvymResolver.sys.mjs` and friends), the daemon asks the browser to do the fetching on its behalf. The browser already has all the network infrastructure, all the cookies, all the certificate trust, all the proxy/VPN configuration, and — crucially — the entire HVYM resolution stack.

This requires a **new direction** in the existing Lupus IPC: today the protocol is browser → daemon (browser sends `search`, `scan_page`, etc. and gets responses). To support delegated fetching the daemon needs to be able to send requests TO the browser and receive responses. This doc spells out exactly what changes in the Lepus repo to make that work.

The boundary holds: HVYM name resolution still lives entirely in `browser/components/hvym/`. The Lupus daemon never talks to Soroban directly. The new direction is purely a "please fetch this URL for me" channel — the browser uses its existing resolvers under the hood.

---

## 2. TL;DR

Add a **second message direction** to `LupusClient.sys.mjs`. When the daemon sends a message with `method` set (instead of `status`), it's an incoming request, not a response. Route it to a handler. Implement one handler — `host_fetch` — that wraps the standard Web `fetch()` API. Reply over the same WebSocket with the same envelope shape. Done.

The HVYM half is free: `browser/components/hvym/HvymProtocolHandler.sys.mjs` already registers `hvym://` as a real Necko protocol handler with `URI_LOADABLE_BY_ANYONE`, so `fetch("hvym://alice@gallery")` Just Works through the standard fetch API. No special-casing in the connector.

---

## 3. Current state

### `browser/components/lupus/LupusClient.sys.mjs` (128 lines today)

What it does:
- Holds a single `WebSocket` to `ws://127.0.0.1:9549`
- `connect()` opens it; `disconnect()` closes it
- Outbound API: `search()`, `scanPage()`, `summarize()`, `indexPage()`, `getStatus()` — each calls `_request(method, params)` which sends a `{id, method, params}` envelope and awaits a response correlated by id
- `_handleResponse(data)` is the WebSocket `onmessage` handler — looks up `data.id` in `_pendingRequests` and resolves the matching promise

What's missing for daemon→browser direction:
- No inbound *request* dispatch (only inbound *response* dispatch)
- No `host_fetch` handler
- No way to send a reply envelope back over the same socket

### `browser/components/lupus/moz.build` (11 lines)

Currently lists only `LupusClient.sys.mjs`. No `tests/` directory under `browser/components/lupus/`. New test files for the host RPC need a new test infrastructure entry.

### Existing browser-side HVYM stack (used unchanged)

- `browser/components/hvym/HvymResolver.sys.mjs` (978 lines) — Soroban RPC + name resolution + tunnel URL construction. Verified, 67+ mochitest assertions passing.
- `browser/components/hvym/HvymProtocolHandler.sys.mjs` (359 lines) — `nsIProtocolHandler` registered for the `hvym` scheme. `URI_LOADABLE_BY_ANYONE`. Standard `fetch()` calls to `hvym://...` URLs flow through here.
- `browser/components/hvym/SubnetSelector.sys.mjs` — UI surface, not relevant to this work.

The connector work does **not** modify any of these. It only consumes them indirectly via `fetch()`.

---

## 4. The new daemon→browser direction

### 4.1 Message-shape disambiguation

The same envelope shape is used in both directions; only the field set distinguishes them:

| Message kind | Has `method` | Has `status` | Direction |
|---|---|---|---|
| Browser-initiated request | ✓ | — | Browser → Daemon |
| Daemon's reply to a browser request | — | ✓ | Daemon → Browser |
| **Daemon-initiated request** (NEW) | ✓ | — | Daemon → Browser |
| **Browser's reply to a daemon request** (NEW) | — | ✓ | Browser → Daemon |

The WebSocket `onmessage` handler dispatches on shape:

```js
_handleMessage(data) {
  if (data.method !== undefined) {
    // Daemon-initiated request — route to handler
    this._handleInboundRequest(data);
  } else {
    // Reply to a browser-initiated request
    this._handleResponse(data);
  }
}
```

Rename the existing `_handleResponse` if needed; it stays the same internally.

### 4.2 ID namespace partitioning

To avoid id collisions between the two directions:

| Origin | id format | Example |
|---|---|---|
| Browser-initiated | `req-N` (existing) | `req-1`, `req-2`, ... |
| Daemon-initiated | `daemon-req-N` (new) | `daemon-req-1`, `daemon-req-2`, ... |

The browser's `_pendingRequests` map only tracks `req-*` ids. The daemon's correlation table only tracks `daemon-req-*` ids. They don't interact.

The browser does NOT need to validate the id prefix when receiving a daemon request — the message is already disambiguated by the `method` vs `status` field. The prefix is purely a debugging convenience and a defense-in-depth against future protocol additions.

### 4.3 Reply envelope

Browser → Daemon reply uses the same envelope shape as the existing daemon → browser reply:

```json
{
  "id": "daemon-req-7",
  "status": "ok",
  "result": { ... }
}
```

or:

```json
{
  "id": "daemon-req-7",
  "status": "error",
  "error": { "code": "fetch_failed", "message": "..." }
}
```

Where `id` echoes the inbound `id` from the daemon's request.

---

## 5. The `host_fetch` handler

This is the only daemon→browser method needed for this round (`host_search_registry` was originally proposed but dropped — see `lupus/docs/LUPUS_TOOLS.md` §4.2 — the cooperative search surface doesn't exist yet).

### 5.1 Request shape (from daemon)

```json
{
  "id": "daemon-req-N",
  "method": "host_fetch",
  "params": {
    "url": "https://example.com/article",
    "method": "GET",
    "headers": {},
    "body": null
  }
}
```

| Field | Required | Notes |
|---|---|---|
| `url` | ✓ | Any scheme the browser can fetch: `https://`, `http://`, `hvym://`, or bare `name@service` form (the browser's HvymResolver will normalize the bare form). |
| `method` | — | HTTP method. Defaults to `GET`. The daemon currently only ever sends `GET` but the field is reserved. |
| `headers` | — | Extra request headers. Empty by default. **Cookies are NOT set by the daemon** — the browser uses its own cookie store automatically via `fetch()`'s default credentials behavior. |
| `body` | — | Request body for POST/PUT. Reserved; daemon doesn't send any today. |

### 5.2 Response shape (from browser)

```json
{
  "id": "daemon-req-N",
  "status": "ok",
  "result": {
    "url": "https://example.com/article",
    "final_url": "https://example.com/article?utm_source=...",
    "http_status": 200,
    "content_type": "text/html; charset=utf-8",
    "body": "<!doctype html>...",
    "truncated": false,
    "fetched_at": 1744194600
  }
}
```

| Field | Notes |
|---|---|
| `url` | Echoes the requested URL verbatim. |
| `final_url` | URL after redirects. May differ from `url`. |
| `http_status` | HTTP status code (200, 404, 500, ...). NOT the daemon-RPC `status` — the RPC `status: "ok"` just means "the fetch attempt completed without infrastructure error". A 404 from the server is `status: "ok"` with `http_status: 404`. |
| `content_type` | Verbatim from the response Content-Type header. |
| `body` | Response body as a UTF-8 string. Binary bodies (PDFs, images) should be returned as `body: ""` with `content_type` set, OR base64-encoded — see open question 1. |
| `truncated` | `true` if the body was cut at the 8 MB cap. The daemon will log a warning when this fires so we can spot pages that consistently hit the limit. |
| `fetched_at` | Unix timestamp (seconds) when the fetch completed. |

### 5.3 Body-size cap

- Default cap: **8 MB** (configurable on the daemon side, see `lupus/docs/LUPUS_TOOLS.md` §3)
- Enforcement happens **on the browser side** during streaming — read up to 8 MB worth of chunks from the response body, then stop and set `truncated: true`
- The cap is large enough to cover ~99% of real pages (big Wikipedia articles, e-commerce category pages with embedded thumbnails)
- It is small enough to keep WebSocket message frames manageable for the local IPC path

### 5.4 Implementation sketch

```js
async _handleInboundRequest(req) {
  const { id, method, params } = req;
  let result, error;
  try {
    switch (method) {
      case "host_fetch":
        result = await this._handleHostFetch(params);
        break;
      default:
        error = { code: "unknown_method", message: `unknown daemon method: ${method}` };
    }
  } catch (e) {
    error = this._mapFetchError(e);
  }
  const reply = error
    ? { id, status: "error", error }
    : { id, status: "ok", result };
  this._ws.send(JSON.stringify(reply));
}

async _handleHostFetch({ url, method = "GET", headers = {}, body = null }) {
  // The HvymProtocolHandler at browser/components/hvym/HvymProtocolHandler.sys.mjs
  // makes hvym:// a real Necko scheme, so fetch() works for both https:// and hvym://
  // with no special-casing here.
  const response = await fetch(url, {
    method,
    headers,
    body,
    redirect: "follow",
    credentials: "include", // reuse browser cookie store
  });

  // Stream the body, capping at 8 MB.
  const CAP = 8 * 1024 * 1024;
  const reader = response.body.getReader();
  const chunks = [];
  let total = 0;
  let truncated = false;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    if (total + value.length > CAP) {
      const remaining = CAP - total;
      if (remaining > 0) chunks.push(value.subarray(0, remaining));
      truncated = true;
      reader.cancel();
      break;
    }
    chunks.push(value);
    total += value.length;
  }
  const bodyBytes = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    bodyBytes.set(c, offset);
    offset += c.length;
  }
  // TODO open question 1: how to handle non-text bodies
  const bodyString = new TextDecoder("utf-8", { fatal: false }).decode(bodyBytes);

  return {
    url,
    final_url: response.url,
    http_status: response.status,
    content_type: response.headers.get("content-type") || "",
    body: bodyString,
    truncated,
    fetched_at: Math.floor(Date.now() / 1000),
  };
}
```

This is a sketch, not final code. The error mapping helper, the actual reader loop, and the binary-body decision still need pinning down — see §6.

### 5.5 Error code mapping

The daemon side recognizes these `error.code` values from `host_fetch` failures:

| Error code | When | Browser should map from |
|---|---|---|
| `fetch_failed` | Network error, DNS failure, TLS failure | `TypeError` from `fetch()`, abort errors |
| `fetch_timeout` | Browser-side timeout fired | `AbortError` after 30 s elapsed |
| `fetch_too_large` | Exceeded an even-harder limit (e.g. 64 MB streaming abort) | Reserved for the future; for now, just truncate at 8 MB and return `truncated: true` |
| `hvym_unresolved` | The HVYM name didn't resolve via Soroban | Specific error from `HvymProtocolHandler.newChannel()` |
| `hvym_unauthorized` | Reserved; HVYM fetching is currently public so this never fires today | — |
| `unknown_method` | Daemon sent a method other than `host_fetch` | Default case in dispatch switch |

The daemon's outer agent loop will receive a `LupusError::ToolError { tool: "fetch_page", message: <code+text> }` that propagates back through the joinner.

---

## 6. Files to touch (Lepus side)

| File | Change | Lines (rough) |
|---|---|---|
| `browser/components/lupus/LupusClient.sys.mjs` | Add inbound request dispatch + `host_fetch` handler | +150 |
| `browser/components/lupus/moz.build` | Add `BROWSER_CHROME_MANIFESTS` for the new test directory | +3 |
| `browser/components/lupus/tests/browser/browser.toml` | New file — test manifest | +5 |
| `browser/components/lupus/tests/browser/browser_lupus_host_fetch.js` | New file — mochitests for `host_fetch` | +200 |
| `browser/components/lupus/tests/browser/browser_lupus_host_fetch_hvym.js` | New file — HVYM-specific mochitest | +120 |

No changes needed in `browser/components/hvym/` — the HVYM layer is consumed unchanged.

---

## 7. Tests

### 7.1 `browser_lupus_host_fetch.js` (HTTPS path)

Set up a fixture HTTP server (Mozilla's `httpd.js` test server pattern), then:

1. **Happy path** — Daemon sends `host_fetch` for a 1 KB fixture page. Browser replies with `status: "ok"`, `http_status: 200`, body matches.
2. **Redirect** — Fixture serves a 302 to a second URL. Browser follows redirect, response `final_url` differs from request `url`.
3. **404** — Fixture returns 404. Browser replies with `status: "ok"` (RPC succeeded), `http_status: 404`.
4. **Network error** — Daemon sends `host_fetch` for a non-routable URL. Browser replies with `status: "error"`, `error.code: "fetch_failed"`.
5. **Body cap** — Fixture serves a 12 MB body. Browser replies with `truncated: true`, body length exactly 8 MB.
6. **Timeout** — Fixture sleeps 31 s before responding. Browser fires its 30 s timeout, replies with `error.code: "fetch_timeout"`.
7. **Cookie reuse** — Fixture sets a cookie on first request, asserts the cookie is sent on second request. Confirms `credentials: "include"` is honored.

### 7.2 `browser_lupus_host_fetch_hvym.js` (HVYM path)

1. **Happy path** — Mock Soroban resolver returns a tunnel record pointing at a fixture HTTP server. `host_fetch` for `hvym://alice@gallery` resolves and fetches.
2. **Unresolved name** — Mock resolver returns null. `host_fetch` for `hvym://nobody@anywhere` returns `error.code: "hvym_unresolved"`.
3. **Bare `name@service` form** — `host_fetch` for `alice@gallery` (no `hvym://` prefix) is normalized by the resolver and resolves correctly.

These tests need the existing `HvymResolver` mock infrastructure from `browser/components/hvym/tests/browser/browser_hvym_resolver.js` — refactor it into a shared helper if needed.

### 7.3 Manual end-to-end smoke

Once both daemon (Phase 4 of `LUPUS_TOOLS.md`) and browser are wired:

1. Run real Lupus daemon (`cargo run` from `lupus/daemon/`)
2. Launch real Lepus build
3. Type a query in the URL bar that exercises `fetch_page` (e.g. "summarize https://en.wikipedia.org/wiki/Wolf")
4. Confirm the agent loop completes and the response includes real text from the page

---

## 8. Sequence diagram — full agent fetch

```
User                 Lepus URL bar          LupusClient            Daemon
 |                       |                      |                    |
 |-- "summarize URL" -→  |                      |                    |
 |                       |--- search(query) -→  |                    |
 |                       |                      |--- search req ──→  |
 |                       |                      |                    | (planner emits)
 |                       |                      |                    |   1. fetch_page(URL)
 |                       |                      |                    |   2. extract_content($1)
 |                       |                      |                    |   3. join_finish($2)
 |                       |                      |                    |
 |                       |                      |  ←── daemon-req-1: host_fetch(URL)
 |                       |                      | (fetch via Necko,
 |                       |                      |  HvymProtocolHandler
 |                       |                      |  if hvym://)
 |                       |                      |--- response ────→  | (executes step 2)
 |                       |                      |                    | (executes step 3)
 |                       |                      |  ←── search reply (text_answer + plan + results)
 |                       |  ←── search resolves |                    |
 |  ←── render summary   |                      |                    |
```

The new arrow is the `daemon-req-1` callback in the middle. Everything else is unchanged from the existing flow.

---

## 9. Open questions

1. **Binary bodies.** When `host_fetch` is called against a PDF, image, or any non-text content, the current sketch decodes it as lossy UTF-8 — that produces garbage. Three options:
   - (a) Return `body: ""` for any non-text content type, expose a `content_type` so the daemon can route around it. The daemon's agent loop never tries to read PDFs as text, so this is fine for now.
   - (b) Base64-encode binary bodies and add `body_encoding: "base64"` to the response.
   - (c) Reject binary content with `error.code: "binary_unsupported"`.
   - **My recommendation: (a) for this round.** The agent only consumes HTML/text right now. Revisit when an extract-from-PDF tool gets added.

2. **`final_url` for HVYM fetches.** When `fetch("hvym://alice@gallery")` follows the protocol handler's fast path, the `response.url` is the resolved tunnel URL (e.g. `https://tunnel.hvym.link/...`). Should we preserve the original `hvym://` URL in `final_url` for clarity, or expose the tunnel URL? The daemon doesn't currently care, but the joinner's user-facing output may show whichever is in the response.

3. **Connection lifecycle.** Today `LupusClient` connects lazily on first request. With daemon→browser direction, the daemon may want to send `host_fetch` at times when no browser-initiated request is in flight. Does the connection stay open the entire browser session? (My assumption: yes — same as today.) Does the browser need to handle daemon disconnect/reconnect during a long-running agent loop? (My assumption: yes, with a brief retry window before erroring out.)

4. **Test infrastructure.** The hvym tests at `browser/components/hvym/tests/browser/` use a mock HvymResolver. Should the new lupus connector tests reuse that mock, or stand up their own? Reusing is cleaner but creates a dependency between the two component test suites.

5. **CSP / origin handling.** When the browser-side `fetch()` runs `host_fetch`, what origin does it run as? (System principal? Null principal? Content principal of the active tab?) This matters for cookies, CORS, mixed-content blocking, and CSP enforcement. **My assumption:** the connector runs in the parent process with system principal, so CORS and mixed-content rules don't apply — but I want a Lepus-side reviewer to confirm before we ship.

6. **Rate limiting.** Should the browser cap how many concurrent `host_fetch` requests it processes per second? The daemon's agent loop is sequential per-query so the natural rate is low, but a multi-tab user with multiple agent loops in flight could in theory queue up many. Default: no limit, revisit if it becomes a problem.

---

## 10. Phasing (Lepus-side)

This work lands as a **single Lepus PR** after the daemon-side Phase 1-3 are merged in `lupus/`. The dependency chain:

```
[lupus] Phase 1: daemon-side host RPC plumbing + mock browser test peer
                 │
                 ↓
[lupus] Phase 2: fetch_page wired to host_rpc + search_subnet sentinel
                 │
                 ↓
[lupus] Phase 3: crawl_index wired to host_rpc
                 │
                 ↓
[lepus] LUPUS_CONNECTORS work: LupusClient inbound dispatch + host_fetch handler + tests
                 │
                 ↓
        End-to-end smoke: real daemon + real browser + fixture page
                 │
                 ↓
[lupus] Phase 5: Iroh / IPFS integration  ← independent of this doc
```

The daemon's mock browser peer (Phase 1) lets the lupus side land Phases 1-3 with green tests **before** the Lepus PR is even opened. The Lepus PR is the first time the two halves meet on real infrastructure.

---

## 11. What this doc does NOT cover

- **The HVYM resolver itself.** Already shipped, see `browser/components/hvym/HvymResolver.sys.mjs`.
- **The `LupusClient.search()` browser→daemon side.** Already exists, only needs an unrelated update for the new `SearchResponse` shape (text_answer + plan + results) per `lepus/docs/LUPUS.md` §IPC Protocol — that's a separate fix and not blocking this work.
- **Process lifecycle (spawning the daemon).** Already handled by whatever currently launches the daemon process; unchanged here.
- **UI surfaces.** No new UI; this is plumbing only. Trust indicator, search results display, etc. all stay as they are.
- **Daemon → browser progress events.** Reserved for the future — could be useful for showing "fetching page 1 of 3..." in the UI, but not needed for the first cut.

---

## 12. Sign-off checklist

Before this lands as a Lepus PR:

- [ ] Confirm message-shape disambiguation rule (`method` vs `status`) is OK with Lepus reviewers
- [ ] Confirm `daemon-req-N` id namespace doesn't conflict with anything else in `LupusClient`
- [ ] Pick an answer for open question 1 (binary body handling)
- [ ] Pick an answer for open question 2 (final_url for HVYM)
- [ ] Confirm open question 5 (origin / CSP behavior of system-principal `fetch()`)
- [ ] Daemon-side Phase 1-3 must be merged in `lupus/` first
- [ ] Mochitests passing locally before pushing
