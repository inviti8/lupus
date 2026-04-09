# Lupus Tools — Implementation Plan

**Status:** Reviewed and hardened — v0.1 alpha contract locked in, ready to start Phase 1
**Date:** 2026-04-09 (reviewed + hardened 2026-04-09)
**Scope:** Wire `fetch_page`, `crawl_index`, and the local Iroh blob store so the agent loop can actually fetch web/HVYM content and build a real local search engine. Lock in the wire-format contract so Lepus can integrate against alpha without coordinated re-releases.
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
| `crawl_index` | `daemon/src/tools/crawl_index.rs` | ❌ returns `indexed: false` (Phase 3 target — see §4.3, §4.6) |
| `IpfsClient` (Iroh) | `daemon/src/ipfs.rs` | ❌ state-machine only — local blob store lands Phase 3 (see §4.4), gossip layer lands Phase 5 |
| `Crawler` | `daemon/src/crawler.rs` | ❌ scaffolding (out of scope this round) |

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

**Target behavior (revised — see §4.6):**
1. Resolve the source: if it's a CID, fetch via `IpfsClient::get_blob`; if it's a URL, fetch via `host_rpc::fetch`
2. Run `extract_content` against the body to get title + clean text + content_type
3. **Store the raw HTML body in the local Iroh blob store via `IpfsClient::add_blob` → get back a `content_cid`** (NEW — this is what makes the SearchIndex entries time-stable and cooperative-shareable later)
4. (Optional) Generate an embedding — **not in this round** (depends on `search_local_index` work)
5. Add an entry to the local `SearchIndex` with `{url, title, summary, content_type, content_cid, fetched_at}`
6. If `index.contribution_mode != "off"`, gossip-publish the index entry via `IpfsClient::publish` (opt-in cooperative sync — Phase 5 only)

For this round: implement steps 1, 2, 3, 5. Defer 4 (embedding — needs model) and 6 (cooperative gossip publish — needs Iroh discovery layer in Phase 5).

**Why step 3 moves earlier than originally planned:** see §4.6 for the full reasoning. TL;DR: writing to Iroh blob store from day 1 means every crawled page immediately has a CID, the local cache uses Iroh's blob store as its backing layer (no separate cache to manage), Iroh's built-in blob GC handles size limits, and Phase 5 just turns on gossip without any data migration.

### 4.4 `IpfsClient` (`daemon/src/ipfs.rs`)

**Current:** State machine works (`Disconnected → Connected`); `fetch()` errors out, `publish()` is a no-op.

**Target behavior — split across phases:**

**Phase 3 (local-only, NO networking yet):**
- Replace stub state machine with a real `iroh` blob store, configured for local-only operation (no gossip, no discovery)
- `add_blob(bytes) -> Cid`: hash, store locally, return the CID
- `get_blob(cid) -> Option<Bytes>`: local-only lookup
- `connect()`: spin up the local Iroh node with networking disabled
- **No gossip / no peers / no remote fetch** at this phase — we're just using Iroh as a content-addressed local blob store, which is genuinely useful even with zero peers

**Phase 5 (cooperative gossip layer):**
- Enable Iroh's discovery / gossip layer
- `get_blob(cid)` falls through to remote peer lookup if not local
- `publish(index_entry)`: gossip-announce the entry to cooperative peers
- `connect()`: bootstrap into the gossip mesh

**Storage budget / GC:** Iroh's blob store has built-in garbage collection. We configure a max size (default 5 GB per `daemon/src/config.rs::IpfsConfig::max_cache_gb`) and Iroh handles eviction. No custom eviction logic in the daemon. Confirmed with user 2026-04-09.

**Iroh version + API surface:** latest stable, blobs API at minimum. Decision deferred to start of Phase 3 (no longer Phase 5 — see §4.6).

### 4.5 `search_local_index` and `Crawler`

**Out of scope this round.** Both depend on having an embedding model wired up and a vector index implementation. Tracked as future work.

### 4.6 HTTPS pages → local IPFS index — the search engine surface

This subsection captures an architectural decision that touches §4.3, §4.4, and §5 simultaneously, rather than being a "tool" in its own right. It's the answer to "can we index HTTPS pages into the local IPFS layer?" — and the answer is **yes, and we should, from day 1**.

#### The big idea

When `crawl_index` (or any future `index_page`-driven crawl) fetches a page, it stores the **raw HTML body in the local Iroh blob store**, getting back a content-addressed CID. The `SearchIndex` entry stores BOTH the original URL AND the CID. This makes every entry a **pointer-with-cache**:

```
SearchIndex entry {
  url:           "https://en.wikipedia.org/wiki/Wolf",
  title:         "Wolf - Wikipedia",
  summary:       "...",
  content_type:  "text/html; charset=utf-8",
  content_cid:   "bafkreig...",     ← Iroh blob, has the actual HTML
  fetched_at:    1744200000,
  // (later) embedding: [f32; N]
}
```

#### What this enables

| Capability | How it works |
|---|---|
| **Local search over visited/crawled pages** | Same as Chrome history search, but actually useful — full-text + (later) semantic |
| **Time-stable references** | If `wikipedia.org/wiki/Wolf` 404s tomorrow, the CID still works. The blob is in our local Iroh store. |
| **Cooperative-shared crawl (Phase 5)** | If alice crawls 1,000 useful Wikipedia articles and contributes the entries to the gossip layer, bob's local search finds them. The CIDs let bob fetch the actual content from alice (or any other member with the blob) via Iroh's content-addressed lookup. |
| **Natural deduplication** | Two users crawling the same Wikipedia article get the same CID. The cooperative naturally deduplicates without coordination. |
| **Real search engine on day 1** | Even before any cooperative content exists, the user has a private search engine over pages they've actually browsed/crawled. This is the most under-rated property — alpha is immediately useful. |

#### Why this changes the phasing

The original LUPUS_TOOLS.md draft had `IpfsClient::add_blob` / `get_blob` deferred to Phase 5 (alongside gossip + cooperative publish). This subsection moves the **local blob store ops** to Phase 3, keeping only the **gossip / discovery / peer-lookup** layer in Phase 5. The split is clean:

| Phase | IPFS surface |
|---|---|
| Phase 3 | Local Iroh blob store (add/get by CID, GC, storage cap) — networking disabled |
| Phase 5 | Gossip discovery, remote peer fetch, cooperative publish — networking enabled |

The Phase 3 daemon has a working content-addressed cache from day 1, even with zero peers. Phase 5 just turns on the network without any data migration.

#### What's still deferred (NOT in scope this round)

- **Embeddings on stored content.** The index entry shape reserves a future `embedding` field, but nothing populates it until the embedding model lands. Keyword search over `summary` is fine for alpha. Embeddings backfill later by walking the index entries and computing them in batch.
- **Cooperative gossip publish.** Stays Phase 5. Local-only is genuinely useful first.
- **Background autonomous crawling.** The `Crawler` module stays scaffolding. For alpha, indexing is driven by either the agent loop's `crawl_index` tool or the browser's `index_page` IPC call. No proactive crawling.
- **Per-domain robots.txt enforcement.** Doesn't matter until cooperative publish exists. If we publish someone else's content to peers, we should respect their preferences. For local-only caching, browser-cache rules apply (which is to say, none).
- **Copyright / republishing policy.** Same — only matters once gossip publish is on. For local-only this is the same as any browser cache.
- **Privacy / opt-out for crawled content showing up in cooperative publish.** Off-by-default via `cooperative.contribution_mode = "off"` in the existing config. Only flips on when user explicitly enables.

#### The data model commitment (locked in for v0.1)

The `SearchIndex` entry struct is part of the wire contract — the daemon's `index_stats` IPC method exposes counts, and any future tool that searches the index returns entries in this shape. Lock in the field set now so we don't have to migrate later:

```rust
pub struct IndexEntry {
    pub url:          String,
    pub title:        String,
    pub summary:      String,
    pub content_type: String,
    pub content_cid:  String,    // empty string if not yet stored in Iroh
    pub fetched_at:   u64,       // unix seconds
    // Reserved for future fields (additive only):
    // pub embedding: Option<Vec<f32>>,
    // pub keywords:  Vec<String>,
    // pub source:    String,    // "user" / "agent" / "cooperative"
}
```

Reserved fields are documented in the struct comment but not added until they're needed. The additive-only rule (see §7) means future fields can be added without bumping the protocol version.

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

**Phase 3 — Wire `crawl_index` + local Iroh blob store (NO gossip)**

1. Add `iroh` to `daemon/Cargo.toml`. Pick latest stable, blobs API. Networking disabled in node config.
2. Implement `IpfsClient::add_blob`, `get_blob`, `connect` (local-only mode)
3. Configure Iroh's blob GC against `IpfsConfig::max_cache_gb` (default 5 GB)
4. `crawl_index::execute` → `host_rpc::fetch` + `IpfsClient::add_blob` + `extract_content` + `SearchIndex::add` (with `content_cid` populated)
5. Defer embedding generation (depends on the deferred `search_local_index` work)
6. Defer `IpfsClient::publish` (gossip layer) until Phase 5

**Phase 4 — Lepus-side `LupusClient` extension**

1. Inbound request dispatch in `LupusClient.sys.mjs`
2. `host_fetch` handler — HTTPS via `fetch()`, HVYM via existing `HvymProtocolHandler.sys.mjs` / `HvymResolver.sys.mjs`. Enforces 8 MB body cap with `truncated: true` flag.
3. Mochitests for both code paths
4. End-to-end test: real browser + real daemon + a fixture page → agent loop completes with real data, `crawl_index` produces a real `SearchIndex` entry with a real `content_cid` for the cached HTML

**Phase 5 — Iroh gossip / cooperative publish**

1. Re-enable Iroh's discovery / gossip layer in the existing client
2. `IpfsClient::get_blob` falls through to remote peer lookup if not local
3. `IpfsClient::publish(index_entry)` — gossip-announce to cooperative peers
4. `crawl_index` opt-in cooperative publish path (`cooperative.contribution_mode != "off"`)
5. **Re-visit the deferred policy questions (robots.txt, copyright, opt-out) before flipping default contribution_mode away from "off"**

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

## 7. v0.1 alpha contract — what we lock in for Lepus integration

The principle: **lock in what's expensive to change later, leave everything else iterative.** The wire format, error code vocabulary, and namespace conventions become part of the alpha contract once Lepus ships against them. Every other dimension of the daemon stays free to evolve.

### 7.1 What's locked in (NOW)

These are wire-format / namespace / vocabulary decisions. Once Lepus ships against them, changing them requires a coordinated PR release on both sides. Locking them in this round means later refactors are "add a code", "tune a number", "add a field" — never "rewrite the wire format".

| Item | Lock-in commitment |
|---|---|
| **Protocol version field** | Add `pub const PROTOCOL_VERSION: &str = "0.1";` at the top of `daemon/src/protocol.rs`. The `get_status` response includes a `protocol_version: String` field. Lepus calls `get_status` immediately on connect and refuses to use the daemon if the version doesn't match what it knows. v2 can come later without breaking v1 clients. |
| **Stable error code vocabulary** | New file `daemon/src/protocol_codes.rs` defines every error code as a `pub const &str`. Every error in the daemon uses these constants instead of inline strings. The Lepus side mirrors them in a JS file. Drift between the two sides is now grep-detectable. |
| **Tool result sentinel convention** | The pattern established for `search_subnet` (`{matches: [], status: "not_implemented", reason: "..."}`) is THE way any tool says "I exist but I'm not real yet". Documented as a contract. The joinner is taught to handle `status: "not_implemented"` gracefully. |
| **Tool name strings** | The 6 tool names `search_subnet`, `search_local_index`, `fetch_page`, `extract_content`, `scan_security`, `crawl_index` are part of the wire contract. The trained planner LoRA was trained on these names — renaming any of them invalidates ~21/22 of the eval. They are now locked. |
| **`daemon-req-N` / `req-N` id namespaces** | Already in §6 question 5. Locked. |
| **`IndexEntry` field set** | See §4.6. The struct fields are part of the wire contract. Reserved fields are documented but not added until needed. |
| **Envelope additive-only rule** | New fields are additive only. Both daemon and browser sides silently ignore unknown fields they receive. Removed fields require a `PROTOCOL_VERSION` bump. This single rule does ~80% of the work of forward compatibility. |

### 7.2 The error code vocabulary (initial set)

These are the codes that go into `daemon/src/protocol_codes.rs` for v0.1. The Lepus side mirrors this exact list. New codes can be added freely (additive); existing codes must not be renamed or removed without bumping `PROTOCOL_VERSION`.

```rust
// Model lifecycle
pub const ERR_MODEL_NOT_LOADED:   &str = "model_not_loaded";
pub const ERR_MODEL_LOAD_FAILED:  &str = "model_load_failed";
pub const ERR_INFERENCE:          &str = "inference_error";
pub const ERR_ADAPTER_NOT_FOUND:  &str = "adapter_not_found";

// Request / dispatch
pub const ERR_PARSE:              &str = "parse_error";
pub const ERR_INVALID_REQUEST:    &str = "invalid_request";
pub const ERR_UNKNOWN_METHOD:     &str = "unknown_method";

// Tools
pub const ERR_TOOL:               &str = "tool_error";
pub const ERR_NOT_IMPLEMENTED:    &str = "not_implemented";

// Host fetch (daemon → browser direction)
pub const ERR_FETCH_FAILED:       &str = "fetch_failed";
pub const ERR_FETCH_TIMEOUT:      &str = "fetch_timeout";
pub const ERR_FETCH_TOO_LARGE:    &str = "fetch_too_large";
pub const ERR_HVYM_UNRESOLVED:    &str = "hvym_unresolved";

// Index / IPFS
pub const ERR_INDEX:              &str = "index_error";
pub const ERR_IPFS:               &str = "ipfs_error";

// Plumbing
pub const ERR_CONFIG:             &str = "config_error";
pub const ERR_IO:                 &str = "io_error";
pub const ERR_JSON:               &str = "json_error";
pub const ERR_YAML:               &str = "yaml_error";
pub const ERR_WEBSOCKET:          &str = "websocket_error";
```

The existing `LupusError::code()` impl in `daemon/src/error.rs` already returns most of these as inline strings — Phase 1 of the implementation refactors that to use the constants from `protocol_codes.rs` so there's a single source of truth.

### 7.3 Mock peers — both sides ship one

To let the two halves develop independently without blocking on each other:

**Daemon side:** `daemon/src/host_rpc/mock.rs` — an in-process WebSocket peer that pretends to be the browser. Returns canned `host_fetch` responses for fixture URLs. Lets daemon Phase 1-3 land with real integration tests but zero browser dependency.

**Lepus side:** `browser/components/lupus/tests/MockLupusDaemon.jsm` (or the mochitest equivalent) — pretends to be the daemon. Returns canned `search`, `scan_page`, `get_status` responses. Lets `LupusClient.sys.mjs` changes land with real mochitests but zero Rust binary dependency.

These two mocks are the integration unblock. Both halves develop in parallel, integration is the meeting point — the first time real Lupus talks to real Lepus.

### 7.4 What's NOT locked in (deliberately free)

Everything else stays free to change without protocol pain:

- **Internal struct layouts** (anything inside `agent::*`, `security::*`, `index::*`) — totally internal, no wire impact
- **Tool implementation strategies** — each tool's `execute` body can be rewritten freely as long as the input/output JSON shape stays compatible
- **Latency / performance** — we'll discover what's slow only by running real Lepus against real Lupus
- **Body cap value** — 8 MB is a default, can be tuned in config without breaking anything
- **Embedding model choice** — when this lands, it's a new field in `IndexEntry`, not a new shape
- **Cooperative gossip layer details** — entirely Phase 5
- **Daemon → browser timeout** — 30 s default, tunable
- **Cache eviction policy** — Iroh's GC, not our code

### 7.5 Source of truth

`daemon/src/protocol.rs` is the canonical wire contract. If anything else (this doc, `LEPUS_CONNECTORS.md`, `LupusClient.sys.mjs`, the mock peers) drifts from it, the Rust file wins. Doc updates follow code, not the other way around. CI on the Rust side should grep `protocol_codes.rs` against the Lepus mirror file periodically (or as part of a release-gate check) to catch drift early.

---

## 8. What this plan does NOT do

To keep the scope honest:

- **Does not train or wire any embedding model** — `search_local_index` stays a stub. Embeddings are a separate work item.
- **Does not implement the full `Crawler`** — the `index_page` IPC handler will work end-to-end for individual pages (`crawl_index` tool feeds the local index), but background crawling is deferred.
- **Does not change the security model path** — that's already done.
- **Does not touch model distribution / first-run download** — those are part of the build/distribution work, separate from this.
- **Does not solve the cross-platform build pipeline** — that's the next conversation after tools are real.

---

## 9. Risks I can see

| Risk | Impact | Mitigation |
|---|---|---|
| `host_fetch` round-trips inflate per-tool latency | Agent loop wall time grows | Profile after Phase 2; the bottleneck today is planner inference (~3s on debug build), not network |
| Lepus-side `LupusClient` changes block daemon work | Integration delays | Mock browser peer in Phase 1 lets daemon work proceed independently |
| Cooperative registry doesn't exist yet | `search_subnet` can't be wired | Land it returning empty if needed; add real wiring in a follow-up |
| Iroh version churn between 2025 and 2026 | Build breaks, API drift | Pin exact version in Cargo.toml; verify against Iroh changelog |
| The new daemon→browser direction adds a class of bug (deadlocks if the browser handler calls the daemon recursively) | Hangs during agent loop | Document that browser-side handlers must not call daemon methods that themselves call back; add a depth counter as defense in depth |

---

## 10. Sign-off checklist

Architecture & contracts:
- [x] Option B (delegated fetching) — confirmed
- [x] Tool dispatch context (lazy global) — confirmed
- [x] Daemon → browser id namespace (`daemon-req-N`) — confirmed
- [x] `host_fetch` body cap (8 MB) — confirmed
- [x] Timeout (30 s) — confirmed
- [x] No feature flag — confirmed
- [x] `search_subnet` deferred to sentinel — investigated and locked in
- [x] Iroh: gossip-layer discovery for Phase 5; local-only blob store in Phase 3

§4.6 (HTTPS → IPFS indexing) — confirmed:
- [x] Index entries store both URL and `content_cid`
- [x] Iroh blob ops (`add_blob`/`get_blob`) move to Phase 3, gossip stays Phase 5
- [x] Iroh's built-in GC handles storage budget — no custom eviction code
- [x] `IndexEntry` field set locked as part of v0.1 contract

§7 (v0.1 alpha contract) — confirmed:
- [x] `PROTOCOL_VERSION = "0.1"` in `protocol.rs`, exposed via `get_status`
- [x] `protocol_codes.rs` module with the initial error code constants
- [x] Tool sentinel convention documented and locked
- [x] Tool name strings frozen (planner LoRA dependency)
- [x] Envelope additive-only rule documented
- [x] Mock peers ship on both sides as test infrastructure
- [x] `protocol.rs` is the canonical source of truth

**Ready to start Phase 1.**
