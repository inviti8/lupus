# Joinner golden eval — review

**Pass:** 8/8   **Mean latency:** 15534 ms   **Scope:** URL-bar search path (fetch_page → extract_content → joinner)

_Assertions catch gross regressions. Read the `Answer` blocks below to grade quality. The `expected_class` tag describes what the case is testing for — match it against the model's posture._

---

## ✅ PASS  `rich_summary_wikipedia`  (19529 ms)  — expected `informative`

**Query:** `summarize https://en.wikipedia.org/wiki/Wolf`

**Scratchpad:**
```
fetch_page("https://en.wikipedia.org/wiki/Wolf")
Observation: {"url":"https://en.wikipedia.org/wiki/Wolf","http_status":200,"content_type":"text/html","body":"<html><title>Wolf - Wikipedia</title>...","truncated":false}
extract_content("$1", "summary")
Observation: {"title":"Wolf - Wikipedia","summary":"The wolf (Canis lupus) is a large canine native to Eurasia and North America. More than thirty subspecies of Canis lupus have been recognized. The wolf is the largest extant member of the family Canidae and is distinguished from other species by its larger size and less pointed features.","keywords":["wolf","canis lupus","canidae"],"content_type":"article"}
```

**Raw joinner output:**
```
 The extract_content observation contains a summarized version of the wolf's information from the Wikipedia page.
Action: Finish(The wolf (Canis lupus) is a large canine native to Eurasia and North America, with more than thirty subspecies. The wolf is the largest extant member of the family Canidae and is distinguished from other species by its larger size and less pointed features. The summary is provided in the content_type field.)
```

**Parsed answer (308 chars):**

> The wolf (Canis lupus) is a large canine native to Eurasia and North America, with more than thirty subspecies. The wolf is the largest extant member of the family Canidae and is distinguished from other species by its larger size and less pointed features. The summary is provided in the content_type field.

---

## ✅ PASS  `empty_extraction_abstention`  (13647 ms)  — expected `abstention`

**Query:** `summarize https://example.com/unknown`

**Scratchpad:**
```
fetch_page("https://example.com/unknown")
Observation: {"url":"https://example.com/unknown","http_status":200,"content_type":"text/html","body":"<html></html>","truncated":false}
extract_content("$1", "summary")
Observation: {"title":"","summary":"","keywords":[],"content_type":"page"}
```

**Raw joinner output:**
```
 The content from the tool is not usable content, and the body is empty.
Action: Finish(I cannot summarize this content.)
```

**Parsed answer (32 chars):**

> I cannot summarize this content.

---

## ✅ PASS  `fetch_error_cascade`  (13735 ms)  — expected `error_report`

**Query:** `summarize https://down.example.com`

**Scratchpad:**
```
fetch_page("https://down.example.com")
Observation: Error: tool error [fetch_page]: host fetch failed: fetch_failed: DNS lookup failed
extract_content("$1", "summary")
Observation: Error: arg coercion: inference error: extract_content.html: $1 references step that errored or was a join terminator
```

**Raw joinner output:**
```
 The content was not summarized because the tool was not able to retrieve the content.
Action: Finish(I could not summarize the content from the tool.)
```

**Parsed answer (48 chars):**

> I could not summarize the content from the tool.

---

## ✅ PASS  `short_real_content`  (16444 ms)  — expected `informative`

**Query:** `summarize https://example.com/article`

**Scratchpad:**
```
fetch_page("https://example.com/article")
Observation: {"url":"https://example.com/article","http_status":200,"body":"..."}
extract_content("$1", "summary")
Observation: {"title":"Announcing Rust 1.80","summary":"Rust 1.80 adds native support for LazyCell and LazyLock, stabilizes Box<[T]> iteration, and improves diagnostics for async code paths.","keywords":["rust","programming","release"],"content_type":"article"}
```

**Raw joinner output:**
```
 The extract_content observation contains a usable summary.
Action: Finish(Rust 1.80 adds native support for LazyCell and LazyLock, stabilizes Box<[T]> iteration, and improves diagnostics for async code paths.)
```

**Parsed answer (134 chars):**

> Rust 1.80 adds native support for LazyCell and LazyLock, stabilizes Box<[T]> iteration, and improves diagnostics for async code paths.

---

## ✅ PASS  `truncated_mid_sentence`  (18509 ms)  — expected `informative`

**Query:** `summarize https://en.wikipedia.org/wiki/Photosynthesis`

**Scratchpad:**
```
fetch_page("https://en.wikipedia.org/wiki/Photosynthesis")
Observation: {"url":"https://en.wikipedia.org/wiki/Photosynthesis","http_status":200,"body":"..."}
extract_content("$1", "summary")
Observation: {"title":"Photosynthesis - Wikipedia","summary":"Photosynthesis is a biological process that converts light energy, usually from the Sun, into chemical energy stored in glucose and other organic compounds. It takes place in plants, algae, and some bacteria, and is th ... [truncated]","content_type":"article"}
```

**Raw joinner output:**
```
 The extract_content observation contains a usable summary.
Action: Finish(Photosynthesis is a biological process that converts light energy, usually from the Sun, into chemical energy stored in glucose and other organic compounds. It takes place in plants, algae, and some bacteria, and is the subject of the [content_type].)
```

**Parsed answer (250 chars):**

> Photosynthesis is a biological process that converts light energy, usually from the Sun, into chemical energy stored in glucose and other organic compounds. It takes place in plants, algae, and some bacteria, and is the subject of the [content_type].

---

## ✅ PASS  `html_noise_in_summary`  (15777 ms)  — expected `informative`

**Query:** `summarize https://example.com/noisy`

**Scratchpad:**
```
fetch_page("https://example.com/noisy")
Observation: {"url":"https://example.com/noisy","http_status":200}
extract_content("$1", "summary")
Observation: {"title":"Example Domain","summary":"Example Domain This domain is for use in illustrative examples in documents. You may use this domain in literature without prior coordination or asking for permission.","keywords":[],"content_type":"page"}
```

**Raw joinner output:**
```
 The extract_content observation contains a usable summary.
Action: Finish(The summary of the document is "Example Domain This domain is for use in illustrative examples in documents. You may use this domain in literature without prior coordination or asking for permission.")
```

**Parsed answer (200 chars):**

> The summary of the document is "Example Domain This domain is for use in illustrative examples in documents. You may use this domain in literature without prior coordination or asking for permission."

---

## ✅ PASS  `factoid_from_model_knowledge`  (11672 ms)  — expected `factoid`

**Query:** `what is the capital of france`

**Scratchpad:**
```

```

**Raw joinner output:**
```
 I can answer this from my own knowledge without using tools.
Action: Finish(The capital of France is Paris.)
```

**Parsed answer (31 chars):**

> The capital of France is Paris.

---

## ✅ PASS  `abstention_no_scratchpad_hard_query`  (14965 ms)  — expected `abstention`

**Query:** `predict tomorrow's weather in Tokyo`

**Scratchpad:**
```

```

**Raw joinner output:**
```
 I cannot predict the weather in Tokyo tomorrow.
Action: Finish(I cannot predict the weather in Tokyo tomorrow.)
```

**Parsed answer (47 chars):**

> I cannot predict the weather in Tokyo tomorrow.

---

