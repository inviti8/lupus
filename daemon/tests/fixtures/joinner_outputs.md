# Joinner golden eval — review

**Pass:** 0/8   **Mean latency:** 10678 ms   **Scope:** URL-bar search path (fetch_page → extract_content → joinner)

_Assertions catch gross regressions. Read the `Answer` blocks below to grade quality. The `expected_class` tag describes what the case is testing for — match it against the model's posture._

---

## ❌ FAIL  `rich_summary_wikipedia`  (12231 ms)  — expected `informative`

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
 The summary content is correct.
Action: Finish(Summary)
```

**Parsed answer (7 chars):**

> Summary

**Failures:**
- answer too short: 7 chars (min 40)
- must_contain_any: none of ["canis lupus", "wolf", "canine", "eurasia", "canidae"] found in answer

---

## ❌ FAIL  `empty_extraction_abstention`  (10522 ms)  — expected `abstention`

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
 I don't need to answer a question.
Action: Finish(Summary)
```

**Parsed answer (7 chars):**

> Summary

**Failures:**
- answer too short: 7 chars (min 10)
- must_contain_any: none of ["couldn't", "unable", "no content", "empty", "nothing", "no information", "no summary", "not", "could not"] found in answer

---

## ❌ FAIL  `fetch_error_cascade`  (10753 ms)  — expected `error_report`

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
 The summary content is incorrect as it contains the content from the last tool call.
Action: Finish(Summary)
```

**Parsed answer (7 chars):**

> Summary

**Failures:**
- answer too short: 7 chars (min 10)
- must_contain_any: none of ["couldn't", "unable", "error", "failed", "not reach", "could not", "not available", "unreachable"] found in answer

---

## ❌ FAIL  `short_real_content`  (11108 ms)  — expected `informative`

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
 I don't need to answer a question.
Action: Finish(Summary)
```

**Parsed answer (7 chars):**

> Summary

**Failures:**
- answer too short: 7 chars (min 30)
- must_contain_any: none of ["rust", "1.80", "lazycell", "lazylock", "async"] found in answer

---

## ❌ FAIL  `truncated_mid_sentence`  (11219 ms)  — expected `informative`

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
 The summary content is correct.
Action: Finish(Summary)
```

**Parsed answer (7 chars):**

> Summary

**Failures:**
- answer too short: 7 chars (min 30)
- must_contain_any: none of ["photosynthesis", "light", "energy", "plants", "chemical"] found in answer

---

## ❌ FAIL  `html_noise_in_summary`  (10795 ms)  — expected `informative`

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
 I don't need to answer a question.
Action: Finish(Summary)
```

**Parsed answer (7 chars):**

> Summary

**Failures:**
- answer too short: 7 chars (min 20)
- must_contain_any: none of ["example", "illustrative", "domain", "documents", "literature"] found in answer

---

## ❌ FAIL  `factoid_from_model_knowledge`  (9319 ms)  — expected `factoid`

**Query:** `what is the capital of france`

**Scratchpad:**
```

```

**Raw joinner output:**
```
 I don't need to answer a question.
Action: Finish(Task completed!)
```

**Parsed answer (15 chars):**

> Task completed!

**Failures:**
- must_contain_any: none of ["paris"] found in answer

---

## ❌ FAIL  `abstention_no_scratchpad_hard_query`  (9480 ms)  — expected `abstention`

**Query:** `predict tomorrow's weather in Tokyo`

**Scratchpad:**
```

```

**Raw joinner output:**
```
 I don't need to answer a question.
Action: Finish(Task completed!)
```

**Parsed answer (15 chars):**

> Task completed!

**Failures:**
- must_contain_any: none of ["cannot", "can't", "unable", "don't have", "no access", "real-time", "current", "unknown", "not", "don't"] found in answer

---

