# Lupus Tools — Implementation Plan

**Status:** Reviewed — answers locked in below, ready to start Phase 1
**Date:** 2026-04-09 (reviewed 2026-04-09)
**Scope:** Wire `fetch_page`, `crawl_index`, and the IPFS layer so the agent loop can actually fetch web/HVYM content and contribute to the cooperative index.
**Out of scope (this round):**
- `search_local_index` — needs an embedding model + vector store, separate work
- `search_subnet` — the cooperative search surface doesn't exist yet (see §6, question 3); this tool stays as a structured "not yet built" sentinel until the cooperative ships either a `heavymeta.art` search API or an off-chain indexer over `hvym-roster` JOIN events

---

## 1. Current state — what's stub vs real

| Module | File | Status |
|---|---|---|
| Agent loop (planner → executor → joinner) | `daemon/src/agent/*` | ✅ real |
| `scan_security` (heuristics + Qwen v0.3) | `daemon/src/tools/scan_security.rs` + `daemon/src/security.rs` | ✅ real |
| `extract_content` (title + strip-tags + classify) | `daemon/src/tools/extract_content.rs` | ✅ basic but real |
| `search_subnet` | `daemon/src/tools/search_subnet.rs:55` | ❌ returns `[]` |
| `search_local_index` | `daemon/src/tools/search_local.rs:57` | ❌ returns `[]` (out of scope this round) |
| `fetch_page` | `daemon/src/tools/fetch_page.rs` | ❌ returns `"not_implemented"` |
| `crawl_index` | `daemon/src/tools/crawl_index.rs` | ❌ returns `indexed: false` |
| `IpfsClient` (Iroh) | `daemon/src/ipfs.rs` | ❌ state-machine only, fetch errors out |
| `Crawler` | `daemon/src/crawler.rs` | ❌ scaffolding |

---

## 2. The architectural decision that shapes everything

**Question:** When `fetch_page` runs, who actually puts the bytes on the wire?

The agent's `fetch_page` tool can be implemented three ways:

### Option A — Daemon does its own fetching

| For | Against |
|---|---|
| Self-contained daemon, no browser dependency | Need to add `reqwest` (~big tree) |
| Standard pattern, simple to reason about | Need to **reimplement HVYM resolution** in Rust: Soroban RPC + XDR encoder + JSON-SCVal parser + Ed25519 JWT + WSS tunnel client. That's `HvymResolver.sys.mjs` in Lepus, ~weeks of work, and risks drift from the canonical implementation |
|  | Different HTTP client than the browser → different cookies, different UA, different proxy/VPN behavior, different anti-bot fingerprints |
|  | Daemon needs internet directly, even if user's network only routes the browser through a proxy |

### Option B — Daemon delegates fetching back to the browser

| For | Against |
|---|---|
| **No duplication of HVYM resolver** — Lepus already has the entire Soroban + tunnel stack working (`browser/components/hvym/HvymResolver.sys.mjs`, 67+ mochitest assertions passing) | Requires bidirectional IPC — currently the protocol is browser→daemon only |
| Cookies, sessions, UA, proxy, certificate trust all match what the user sees | Daemon can't fetch when disconnected from the browser (acceptable: the agent only runs in response to browser-driven queries anyway) |
| Single network stack to maintain | Tools become async over a network round-trip (already are) |
| Daemon's binary stays smaller — no `reqwest`, no Stellar SDK, no tunnel code |  |

### Option C — Hybrid (HTTPS direct, HVYM delegated)

Daemon adds `reqwest` for HTTPS, delegates HVYM to the browser. Cleanest-sounding compromise but ends up with **two code paths to maintain**, two sets of failure modes, and you still don't get cookie/session/proxy consistency for HTTPS — which is the part the agent uses 95% of the time.

### Decision: **Option B (delegate everything to the browser) — LOCKED IN**

Rationale:
1. **HVYM resolver is heavy** — reimplementing Soroban + tunnel in the daemon is weeks of work that adds zero user-visible value, since Lepus already has it.
2. **Network behavior consistency** — anything the agent fetches on the user's behalf should look like the user fetched it. Anti-bot detection on big sites will treat a `reqwest` UA differently from Firefox; results will diverge from what the user would see in a real tab.
3. **Cookie / auth reuse** — if the user is logged into a site, the agent should be able to fetch authenticated pages. Only the browser has those cookies.
4. **Smaller daemon binary** — drops `reqwest` (and its TLS stack) + any Stellar SDK + tunnel client from the build.
5. **One canonical fetch path** — fewer divergence bugs.
6. **No auth complexity for HVYM fetches** — to *serve* content over an HVYM tunnel you must be a cooperative member (the JWT/Stellar auth flow in `hvym_tunnler` is for content publishers, not consumers). To *fetch* HVYM content is public — anyone can connect to the relay and ask for `alice@gallery`. So delegating fetch to the browser doesn't drag in any new authentication surface; the browser just opens the WSS tunnel and reads the bytes.

**Trade-off accepted:** the daemon's `fetch_page` only works while the browser is connected. That's already the operational model — the agent loop only runs in response to a browser request anyway, so the browser is always there when fetches happen.

**`search_subnet` — separate question, separate answer:** see §4.2 below. The short version is that the cooperative search surface doesn't exist yet, so this tool stays a structured "not yet built" sentinel for this round.

**One genuine exception that stays in the daemon:** the Iroh IPFS client for **background indexing/sync** (`crawl_index`, opt-in cooperative contribution). This is daemon-initiated background work that has nothing to do with browser-driven fetches, and Iroh is the right tool for peer-to-peer content distribution. The browser does not have an IPFS client.

---

## 3. Protocol changes needed for Option B

The current IPC (`daemon/src/protocol.rs`, `daemon/src/server.rs`) is request-response from browser to daemon. We need to add a **reverse direction**: daemon→browser request, browser→daemon response, correlated by id.

### New message direction

| Direction | Message | Purpose |
|---|---|---|
| Daemon → Browser | `{id, method, params}` | Daemon asks the browser to do something on its behalf |
| Browser → Daemon | `{id, status, result | error}` | Browser replies (same envelope shape as the existing daemon-side responses) |

The envelope shape stays identical to the existing browser→daemon protocol — just flipped direction. The id namespace must be partitioned so daemon-originated and browser-originated requests don't collide (e.g. `daemon-req-N` vs `req-N`, or use UUIDs).

### New methods (daemon → browser)

| Method | Purpose | Params | Result |
|---|---|---|---|
| `host_fetch` | Browser fetches a URL on behalf of the daemon. Handles `https://`, `http://`, AND `hvym://` (browser routes to its existing resolvers). | `{ url, headers?, method?, body? }` | `{ url, final_url, status, content_type, body, fetched_at }` |

`host_fetch` is the only new surface needed this round. `host_search_registry` was originally proposed but has been dropped — see §4.2 (the cooperative search surface doesn't exist yet, so there's nothing for the browser to query).

**Body-size cap:** `host_fetch` responses are capped at **8 MB** by default. Covers ~99% of real pages (big Wikipedia articles, e-commerce category pages with embedded thumbnails, long PDFs). The browser side enforces the cap by truncating the body and setting a `truncated: true` flag in the response. The daemon-side handler logs a warning when truncation occurs so we can spot pages that hit the limit. The cap is configurable via the daemon config so it can be raised for unusual workflows.

### Lepus-side work (`browser/components/lupus/LupusClient.sys.mjs`)

The current client (read at `browser/components/lupus/LupusClient.sys.mjs`) only handles incoming responses to outgoing requests. It needs:

1. **Inbound request dispatch** — when an incoming WebSocket message has `method` set instead of `status`, treat it as a daemon-initiated request and route to a handler.
2. **`host_fetch` handler** — wraps `fetch()` (Web API) for HTTPS, routes `hvym://` URLs through the existing `HvymProtocolHandler.sys.mjs` / `HvymResolver.sys.mjs` path. Enforces the 8 MB body cap and sets `truncated: true` if exceeded. No auth handling — fetching HVYM tunnels is public.
3. **Response encoder** — sends `{id, status: "ok", result}` or `{id, status: "error", error}` back over the same WebSocket.
4. **Error mapping** — network errors, CSP failures, certificate errors all need to be translated into stable daemon-side error codes.

This is a non-trivial Lepus-side change. It belongs in a separate Lepus PR after the daemon side is wired and tested with a mock browser client.

---

## 4. Per-tool implementation plan

### 4.1 `fetch_page` (`daemon/src/tools/fetch_page.rs`)

**Current:** Returns hardcoded `"not_implemented"` for both `https://` and `hvym://`.

**Target behavior:**
1. Validate URL scheme (`https://`, `http://`, `hvym://`, or `name@service` form)
2. Build a `host_fetch` request: `{url, method: "GET"}`
3. Send via the new daemon→browser RPC channel
4. Await the browser's response (with timeout)
5. Return `{url, content_type, body, status}` to the agent loop

**Daemon-side surface needed:**
- A new `crate::host_rpc` module that owns the daemon→browser request/response correlation table
- An async `host_rpc::fetch(url) -> Result<HostFetchResponse, LupusError>` helper
- Threaded into `Daemon` so tool dispatch can reach it (same global pattern as `security::CLASSIFIER`, or via tool context — see open question 1)

### 4.2 `search_subnet` (`daemon/src/tools/search_subnet.rs`) — DEFERRED

**Current:** Returns `[]`.

**Investigation result (2026-04-09):** The cooperative search surface does not exist yet anywhere in the Heavymeta stack. Verified across:

- **`pintheon_contracts`** — no `hvym-search` contract; `hvym-roster` has no `list_members`/`query_content` method (members are stored per-key by Address with no on-chain enumeration); `hvym-registry` is a contract-name → contract-address directory, not a content registry.
- **`hvym_tunnler`** (Warren) — pure connection broker. Endpoints are `/health`, `/info`, `/api/tunnels`, `/api/tunnel/{address}`, `/api/stats`, `/proxy/{path}`. No search, no tag/content discovery. Mirrors `hvym-roster` JOIN events into a local SQLite via `roster_sync.py` but doesn't expose the table over HTTP.
- **`heavymeta_collective`** (heavymeta.art portal) — NiceGUI website with public profile pages and a token-gated bot API for *exact-identifier* member lookup (`/api/bot/member/{identifier}`). No `/search`, no `/datapods`, no content metadata schema beyond per-user IPNS linktree entries (`label`, `url`, `icon_cid`, `qr_cid`).

So there is currently nothing the browser can query on the daemon's behalf. The cooperative would need to first build either:
- a search index on `heavymeta.art` over linktree/profile data, or
- an off-chain indexer over `hvym-roster` JOIN events that fetches each member's IPNS linktree and indexes the contents

**Decision for this round:** keep `search_subnet` in the dispatch table (the trained planner LoRA expects it in the toolset — removing it would invalidate ~21/22 of the eval), but have it return a structured "not built yet" sentinel instead of an empty result. The joinner can read the sentinel and produce a graceful "I can't search the cooperative directly yet" message instead of "no results found".

**Sentinel shape:**
```json
{ "matches": [], "status": "not_implemented", "reason": "cooperative search surface not yet built — see docs/LUPUS_TOOLS.md §4.2" }
```

The trained planner picks `search_subnet` for ~5/22 eval cases. With this sentinel, those cases will route through the joinner and produce an honest answer rather than fabricating results from an empty list.

**Re-enable when:** the cooperative ships either of the two indexer paths above. At that point this section gets a follow-up PR adding `host_search_cooperative` (or similar) to the daemon→browser RPC surface and pointing the tool at it.

### 4.3 `crawl_index` (`daemon/src/tools/crawl_index.rs`)

**Current:** Returns `{indexed: false, ...}`.

**Target behavior:**
1. Resolve the source: if it's a CID, fetch via `IpfsClient`; if it's a URL, fetch via `host_rpc::fetch`
2. Run `extract_content` against the body to get title + clean text + content_type
3. (Optional) Generate an embedding — **not in this round** (depends on `search_local_index` work)
4. Add an entry to the local `SearchIndex`
5. If `index.contribution_mode != "off"`, publish the entry via `IpfsClient::publish` (opt-in cooperative sync)

For this round: implement steps 1, 2, 4. Defer 3 (embedding) and 5 (cooperative publish) until the embedding model and Iroh integration land.

### 4.4 `IpfsClient` (`daemon/src/ipfs.rs`)

**Current:** State machine works (`Disconnected → Connected`); `fetch()` errors out, `publish()` is a no-op.

**Target behavior:**
- Replace stub state machine with a real `iroh` client (latest stable, exact version TBD — see open question 4)
- `fetch(cid)`: Iroh blob get with local cache hit-check (cache_dir is already config-plumbed)
- `publish(key, data)`: Iroh blob put + (optional) gossip/discovery announce
- `connect()`: Iroh node startup, connect to cooperative gateway as a known peer

**Open questions:** Iroh API surface to use (blobs vs docs vs spaces), gateway connection model — see section 6.

### 4.5 `search_local_index` and `Crawler`

**Out of scope this round.** Both depend on having an embedding model wired up and a vector index implementation. Tracked as future work.

---

## 5. Phased implementation order

**Phase 1 — Daemon-side host RPC plumbing** (no Lepus changes yet)

1. Add `crate::host_rpc` module with the daemon→browser correlation table (lazy global pattern, same as `security::CLASSIFIER` — `OnceLock<Mutex<HostRpcState>>`)
2. Extend `daemon/src/server.rs` to dispatch incoming messages by shape: `method` set → response from a daemon-initiated request; `status` set → reply to a browser-initiated request
3. Add a mock `host_rpc` test harness (an in-process WebSocket peer that responds to `host_fetch` with canned data)
4. Add `host_fetch` types to `daemon/src/protocol.rs`. Use `daemon-req-N` id prefix to keep daemon-originated and browser-originated id namespaces partitioned.

**Phase 2 — Wire `fetch_page` to host RPC + `search_subnet` sentinel**

1. `fetch_page::execute` → `host_rpc::fetch` (handles `https://`, `http://`, `hvym://`, and bare `name@service` form)
2. `search_subnet::execute` → return the structured "not built yet" sentinel from §4.2 (no host RPC call)
3. Integration test: spin up the mock browser peer + the daemon, run a subset of the 22-case eval that exercises `fetch_page` and confirm the agent loop produces non-empty observations
4. Confirm the joinner gracefully handles the `search_subnet` sentinel on the eval cases that pick it

**Phase 3 — Wire `crawl_index` (without IPFS publish)**

1. `crawl_index::execute` → `host_rpc::fetch` + `extract_content` + `SearchIndex::add`
2. Defer embedding generation (depends on the deferred `search_local_index` work)
3. Defer `IpfsClient::publish` until phase 5

**Phase 4 — Lepus-side `LupusClient` extension**

1. Inbound request dispatch in `LupusClient.sys.mjs`
2. `host_fetch` handler — HTTPS via `fetch()`, HVYM via existing `HvymProtocolHandler.sys.mjs` / `HvymResolver.sys.mjs`. Enforces 8 MB body cap with `truncated: true` flag.
3. Mochitests for both code paths
4. End-to-end test: real browser + real daemon + a fixture page → agent loop completes with real data

**Phase 5 — Iroh / IPFS integration**

1. Choose latest stable Iroh version + API surface (blobs at minimum; docs if we need them for sync)
2. `IpfsClient::connect` → real Iroh node startup, **gossip-layer discovery** (no dedicated gateway peer yet — confirmed)
3. `IpfsClient::fetch` → real blob get with the existing local cache hit-check
4. `IpfsClient::publish` → real blob put + gossip announce
5. `crawl_index` opt-in cooperative publish path

Each phase ends in a green build + a runnable test that proves the phase works in isolation. Phases 1-3 land in Lupus only. Phase 4 is Lepus-side and lands in a separate PR. Phase 5 lands in Lupus.

---

## 6. Resolved decisions (was: open questions)

All eight original questions have been answered. Locked in below — these are the contracts subsequent phases will rely on.

1. **Architecture:** ✅ **Option B — delegate fetching to the browser.** Daemon keeps Iroh for background indexing only.

2. **Tool dispatch context:** ✅ **Lazy global, same pattern as the security classifier.** `OnceLock<Mutex<HostRpcState>>`. Reuses the existing pattern, no refactor of `tools::execute` signature needed.

3. **Cooperative registry API:** ✅ **Doesn't exist yet** (verified across `pintheon_contracts`, `hvym_tunnler`, `heavymeta_collective` — see §4.2 for the full investigation). `search_subnet` returns a structured "not built yet" sentinel this round and gets re-wired when the cooperative ships an indexer. Auth note: HVYM *fetching* is public — the JWT/Stellar auth in `hvym_tunnler` is for content publishers (cooperative members serving tunnels), not consumers. The browser doesn't need any auth to fetch `alice@gallery`.

4. **Iroh:** ✅ **Use latest stable, gossip-layer discovery.** No dedicated cooperative gateway peer to bootstrap from yet — the daemon's Iroh node will discover peers via the gossip layer. Specific version + blobs-vs-docs decision happens at the start of Phase 5.

5. **Daemon → browser request id namespace:** ✅ **`daemon-req-N` prefix.** Simple, debuggable, avoids id collisions with the browser's `req-N` namespace.

6. **`host_fetch` body size limit:** ✅ **8 MB default cap, configurable.** Covers ~99% of real pages including big Wikipedia articles and e-commerce pages with embedded thumbnails. Browser truncates and sets a `truncated: true` flag in the response. Daemon logs a warning when this fires so we can spot pages that hit the limit. Configurable via the daemon config for unusual workflows. IPC is local so the bottleneck isn't the wire — it's the agent's text processing on huge bodies.

7. **Timeout policy:** ✅ **30 s per-fetch inner timeout.** The agent loop's outer timeout still applies on top.

8. **Per-tool feature flag:** ✅ **Always on, no feature flag.** The new RPC plumbing ships as part of the standard daemon build. The mock browser peer in Phase 1 is enough to test the daemon standalone.

---

## 7. What this plan does NOT do

To keep the scope honest:

- **Does not train or wire any embedding model** — `search_local_index` stays a stub. Embeddings are a separate work item.
- **Does not implement the full `Crawler`** — the `index_page` IPC handler will work end-to-end for individual pages (`crawl_index` tool feeds the local index), but background crawling is deferred.
- **Does not change the security model path** — that's already done.
- **Does not touch model distribution / first-run download** — those are part of the build/distribution work, separate from this.
- **Does not solve the cross-platform build pipeline** — that's the next conversation after tools are real.

---

## 8. Risks I can see

| Risk | Impact | Mitigation |
|---|---|---|
| `host_fetch` round-trips inflate per-tool latency | Agent loop wall time grows | Profile after Phase 2; the bottleneck today is planner inference (~3s on debug build), not network |
| Lepus-side `LupusClient` changes block daemon work | Integration delays | Mock browser peer in Phase 1 lets daemon work proceed independently |
| Cooperative registry doesn't exist yet | `search_subnet` can't be wired | Land it returning empty if needed; add real wiring in a follow-up |
| Iroh version churn between 2025 and 2026 | Build breaks, API drift | Pin exact version in Cargo.toml; verify against Iroh changelog |
| The new daemon→browser direction adds a class of bug (deadlocks if the browser handler calls the daemon recursively) | Hangs during agent loop | Document that browser-side handlers must not call daemon methods that themselves call back; add a depth counter as defense in depth |

---

## 9. Sign-off checklist

- [x] Option B (delegated fetching) — confirmed
- [x] Tool dispatch context (lazy global) — confirmed
- [x] Daemon → browser id namespace (`daemon-req-N`) — confirmed
- [x] `host_fetch` body cap (8 MB) — confirmed
- [x] Timeout (30 s) — confirmed
- [x] No feature flag — confirmed
- [x] `search_subnet` deferred to sentinel — investigated and locked in
- [x] Iroh: gossip-layer discovery, version TBD at Phase 5 start

**Ready to start Phase 1.**
