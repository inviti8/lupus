# Lupus Tools — Implementation Plan

**Status:** Planning — needs review before implementation
**Date:** 2026-04-09
**Scope:** Wire `fetch_page`, `search_subnet`, `crawl_index`, and the IPFS layer so the agent loop can actually fetch web/HVYM content and contribute to the cooperative index.
**Out of scope (this round):** `search_local_index` (needs an embedding model + vector store, separate work).

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

### Recommendation: **Option B (delegate everything)**

Rationale:
1. **HVYM resolver is heavy** — reimplementing Soroban + tunnel in the daemon is weeks of work that adds zero user-visible value, since Lepus already has it.
2. **Network behavior consistency** — anything the agent fetches on the user's behalf should look like the user fetched it. Anti-bot detection on big sites will treat a `reqwest` UA differently from Firefox; results will diverge from what the user would see in a real tab.
3. **Cookie / auth reuse** — if the user is logged into a site, the agent should be able to fetch authenticated pages. Only the browser has those cookies.
4. **Smaller daemon binary** — drops `reqwest` (and its TLS stack) + any Stellar SDK + tunnel client from the build.
5. **One canonical fetch path** — fewer divergence bugs.

**Trade-off accepted:** the daemon's `fetch_page` only works while the browser is connected. That's already the operational model — the agent loop only runs in response to a browser request anyway, so the browser is always there when fetches happen.

**One exception:** the cooperative registry query (`search_subnet`). It's a simple GET to `https://registry.heavymeta.art/search?q=...`. We could either delegate it (consistent with the rule above) or do it directly with `reqwest` (simpler, no protocol change). **Recommendation: also delegate** — gives us one rule, no exceptions.

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
| `host_search_registry` | Browser queries the cooperative registry. | `{ query, scope?, top_k? }` | `{ matches: [{ title, url, description, commitment }] }` |

`host_fetch` is the primary new surface. `host_search_registry` could alternatively be expressed as `host_fetch` with a registry URL — keep it as a separate method only if the registry API needs auth headers or a specific schema the browser knows about.

### Lepus-side work (`browser/components/lupus/LupusClient.sys.mjs`)

The current client (read at `browser/components/lupus/LupusClient.sys.mjs`) only handles incoming responses to outgoing requests. It needs:

1. **Inbound request dispatch** — when an incoming WebSocket message has `method` set instead of `status`, treat it as a daemon-initiated request and route to a handler.
2. **`host_fetch` handler** — wraps `fetch()` (Web API) for HTTPS, routes `hvym://` URLs through the existing `HvymProtocolHandler.sys.mjs` / `HvymResolver.sys.mjs` path.
3. **`host_search_registry` handler** — calls the registry API (which lives where? — see open questions).
4. **Response encoder** — sends `{id, status: "ok", result}` or `{id, status: "error", error}` back over the same WebSocket.
5. **Error mapping** — network errors, CSP failures, certificate errors all need to be translated into stable daemon-side error codes.

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

### 4.2 `search_subnet` (`daemon/src/tools/search_subnet.rs`)

**Current:** Returns `[]`.

**Target behavior:**
1. Send a `host_search_registry` request via `host_rpc`
2. Map the response into the existing `DatapodMatch` shape (`title`, `url`, `description`, `commitment`)
3. Return to the agent loop

**Open question:** does `https://registry.heavymeta.art` exist yet? Schema? — see open questions.

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

1. Add `crate::host_rpc` module with the daemon→browser correlation table
2. Extend `daemon/src/server.rs` to dispatch incoming messages by shape: `method` set → response from a daemon-initiated request; `status` set → reply to a browser-initiated request
3. Add a mock `host_rpc` test harness (an in-process WebSocket peer that responds to `host_fetch` with canned data)
4. Add `host_fetch` and `host_search_registry` types to `daemon/src/protocol.rs`

**Phase 2 — Wire `fetch_page` and `search_subnet` to host RPC**

1. `fetch_page::execute` → `host_rpc::fetch`
2. `search_subnet::execute` → `host_rpc::search_registry`
3. Integration test: spin up the mock browser peer + the daemon, run a 22-case eval that exercises `fetch_page` and confirm the agent loop produces non-empty observations

**Phase 3 — Wire `crawl_index` (without IPFS publish)**

1. `crawl_index::execute` → `host_rpc::fetch` + `extract_content` + `SearchIndex::add`
2. Defer embedding generation
3. Defer `IpfsClient::publish` until phase 5

**Phase 4 — Lepus-side `LupusClient` extension**

1. Inbound request dispatch in `LupusClient.sys.mjs`
2. `host_fetch` handler (HTTPS via `fetch()`, HVYM via existing protocol handler)
3. `host_search_registry` handler
4. Mochitests for both
5. End-to-end test: real browser + real daemon + a fixture page → agent loop completes with real data

**Phase 5 — Iroh / IPFS integration**

1. Choose Iroh version + API surface (blobs only, or blobs+docs?)
2. `IpfsClient::connect` → real Iroh node startup
3. `IpfsClient::fetch` → real blob get with cache
4. `IpfsClient::publish` → real blob put
5. `crawl_index` opt-in cooperative publish path

Each phase ends in a green build + a runnable test that proves the phase works in isolation. Phases 1-3 land in Lupus only. Phase 4 is Lepus-side and lands in a separate PR. Phase 5 lands in Lupus.

---

## 6. Open questions for review

I need answers to these before starting any implementation. They're listed in the order they block work.

1. **Architecture decision:** Confirm Option B (delegate fetching to the browser, daemon keeps Iroh for background indexing only). This is the load-bearing decision for everything else.

2. **Tool dispatch context:** Tools are currently stateless free functions in `daemon/src/tools/*.rs`. To call `host_rpc` they need access to either a global (`OnceLock<HostRpcClient>`, like the security classifier) or a context parameter threaded through `tools::execute`. Preference?

3. **Cooperative registry API (`registry.heavymeta.art`):**
   - Does the service exist yet?
   - What's the search endpoint shape (path, query params, response JSON)?
   - Auth — public, or does it need a Stellar JWT?
   - If it doesn't exist yet, do we ship `search_subnet` returning empty for this round and add it in the next?

4. **Iroh version + API surface:**
   - Latest stable Iroh as of 2026 (exact version)
   - Blobs only, or blobs + docs (for index entry sync)?
   - Connect to `gateway.heavymeta.art` as a known peer, or run a fully autonomous node and discover via the gossip layer?
   - Is there an existing cooperative Iroh node I should peer with for testing?

5. **Daemon → browser request id namespace:** Use prefix (`daemon-req-N` vs `req-N`) or UUIDs to avoid collisions? UUIDs are safer but bulkier in the JSON.

6. **`host_fetch` body size limit:** Some pages are huge. Cap the body the browser sends back at, say, 2 MB? Let the daemon specify in the request? Truncate silently or error?

7. **Timeout policy:** Daemon-side timeout for a `host_fetch` request? The agent loop already has its own outer timeout — this would be a per-fetch inner timeout. Default 30s?

8. **Per-tool feature flag:** Should the new daemon→browser RPC plumbing be gated behind a feature flag (`features = ["host_rpc"]`) so the daemon can still build standalone for testing without the browser? Or always-on?

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

Before I start coding I need:

- [ ] Confirm Option B (delegated fetching)
- [ ] Answers to open questions 2, 5, 6, 7 (daemon-side decisions — needed for Phase 1)
- [ ] Status of registry API (open question 3) — needed before Phase 2's `search_subnet` work
- [ ] Iroh decisions (open question 4) — needed before Phase 5
- [ ] Confirm phasing — happy to merge phases or split further
