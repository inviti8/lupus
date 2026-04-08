"""Smoke test the trained Lupus security model.

Loads the trained model and runs a curated set of URLs through it,
covering all three classes plus some intentionally tricky cases.

The model location is auto-detected from these paths in order:
  1. --model-path CLI flag
  2. ./dist/lupus-security/                      (local pull from S3)
  3. ./training/output/lupus-security/final/     (local pod final save)
  4. The newest checkpoint-N under training/output/lupus-security/

For each URL, prints the predicted label, confidence, and per-class
score breakdown, with a marker if the prediction disagrees with the
obvious expected label.

Usage:
    python tools/test_security_model.py
    python tools/test_security_model.py --model-path /path/to/model
    python tools/test_security_model.py --window-mmap-fix    # use the manual state-dict loader on Windows
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

if sys.platform == "win32":
    sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[1]


# ---------------------------------------------------------------------------
# Test cases — (url, expected_label_or_None)
# ---------------------------------------------------------------------------


TEST_CASES = [
    # --- clearly safe legitimate sites ---
    ("https://google.com/",                                     "safe"),
    ("https://github.com/inviti8/lupus",                        "safe"),
    ("https://wikipedia.org/wiki/Wolf",                         "safe"),
    ("https://stackoverflow.com/questions/12345",               "safe"),
    ("https://news.ycombinator.com/item?id=12345",              "safe"),
    ("https://huggingface.co/Qwen/Qwen2.5-Coder-0.5B",          "safe"),
    ("https://www.runpod.io/console/pods",                      "safe"),

    # --- obvious phishing patterns ---
    ("https://faceb00k-login.evil.com/verify",                  "phishing"),
    ("https://roblox.com.ge/users/1654861376/profile",          "phishing"),
    ("https://paypa1-secure.support/login.php",                 "phishing"),
    ("https://amaz0n-account-update.xyz/signin",                "phishing"),
    ("https://000webhostapp.com/yahoo/login.php",               "phishing"),
    ("https://login-microsoftonline.000webhost.com/auth",       "phishing"),

    # --- obvious malware URLs (raw IPs serving binaries) ---
    ("http://221.15.91.18:49869/i",                             "malware"),
    ("http://61.53.84.216:45589/i",                             "malware"),
    ("http://117.81.38.146:42927/bin.sh",                       "malware"),

    # --- edge cases / tricky ---
    ("https://google.com.evil-site.ru/login",                   None),  # subdomain attack
    ("https://bit.ly/3xK9z",                                    None),  # URL shortener
    ("https://raw.githubusercontent.com/user/repo/main/script.sh", None),  # legit but raw script
    ("http://192.168.1.1/admin",                                None),  # local IP
    ("https://accounts.google.com.malicious.tk/signin",         None),  # subdomain trick
]


# ---------------------------------------------------------------------------
# Model location auto-detect
# ---------------------------------------------------------------------------


def find_model_path(override: str | None) -> Path:
    if override:
        p = Path(override)
        if not p.exists():
            print(f"ERROR: model not found at {p}", file=sys.stderr)
            sys.exit(1)
        return p

    candidates = [
        REPO_ROOT / "dist" / "lupus-security",
        REPO_ROOT / "training" / "output" / "lupus-security" / "final",
    ]
    for p in candidates:
        if (p / "config.json").exists():
            return p

    # Try the newest checkpoint dir
    output_dir = REPO_ROOT / "training" / "output" / "lupus-security"
    if output_dir.exists():
        ckpts = []
        for d in output_dir.iterdir():
            m = re.match(r"checkpoint-(\d+)$", d.name)
            if m:
                ckpts.append((int(m.group(1)), d))
        if ckpts:
            ckpts.sort(reverse=True)
            return ckpts[0][1]

    print("ERROR: no model found in any of:", file=sys.stderr)
    for c in candidates:
        print(f"  {c}", file=sys.stderr)
    print("  Pull from S3 with: python training/pull_model.py --model security",
          file=sys.stderr)
    sys.exit(1)


# ---------------------------------------------------------------------------
# Model loading (with Windows mmap fallback)
# ---------------------------------------------------------------------------


def load_model(model_path: Path, mmap_fix: bool):
    import torch
    from transformers import AutoConfig, AutoModelForSequenceClassification, AutoTokenizer

    tokenizer = AutoTokenizer.from_pretrained(str(model_path))

    if mmap_fix:
        # On Windows with limited swap, the safetensors mmap fails. Build
        # the model from config and load the state dict via safetensors.torch
        # which doesn't use mmap (loads into RAM directly).
        from safetensors.torch import load_file
        config = AutoConfig.from_pretrained(str(model_path))
        model = AutoModelForSequenceClassification.from_config(config)
        st_path = model_path / "model.safetensors"
        if not st_path.exists():
            print(f"ERROR: {st_path} not found", file=sys.stderr)
            sys.exit(1)
        state_dict = load_file(str(st_path))
        model.load_state_dict(state_dict)
    else:
        model = AutoModelForSequenceClassification.from_pretrained(str(model_path))

    model.eval()
    return tokenizer, model


# ---------------------------------------------------------------------------
# Inference
# ---------------------------------------------------------------------------


def predict(tokenizer, model, url: str) -> tuple[str, float, dict[str, float]]:
    import torch
    text = f"URL: {url}"
    inputs = tokenizer(text, return_tensors="pt", truncation=True, max_length=128, padding=True)
    with torch.no_grad():
        logits = model(**inputs).logits
    probs = torch.softmax(logits, dim=-1)[0]
    pred_idx = int(torch.argmax(probs).item())
    label = model.config.id2label[pred_idx]
    confidence = float(probs[pred_idx].item())
    all_scores = {model.config.id2label[i]: float(probs[i].item()) for i in range(len(probs))}
    return label, confidence, all_scores


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-path", help="Override model path (default: auto-detect)")
    parser.add_argument(
        "--window-mmap-fix",
        action="store_true",
        help="Use the manual state-dict loader (workaround for Windows safetensors mmap)",
    )
    args = parser.parse_args()

    model_path = find_model_path(args.model_path)
    print(f"Loading model from {model_path}...")

    tokenizer, model = load_model(model_path, mmap_fix=args.window_mmap_fix)
    print(f"  num_labels={model.config.num_labels}")
    print(f"  id2label={model.config.id2label}")
    print()

    print(f"Running smoke test against {len(TEST_CASES)} URLs:")
    print("=" * 90)

    correct = 0
    incorrect = 0
    unknown = 0

    for url, expected in TEST_CASES:
        label, confidence, all_scores = predict(tokenizer, model, url)

        if expected is None:
            marker = "  ?  "
            unknown += 1
        elif label == expected:
            marker = "  ok "
            correct += 1
        else:
            marker = "  X  "
            incorrect += 1

        score_str = "  ".join(f"{name}={score:.0%}" for name, score in all_scores.items())

        print(f"{marker}{url[:70]:<70}")
        print(f"     -> {label:<10} ({confidence:.0%})  [{score_str}]")
        if expected and label != expected:
            print(f"     expected: {expected}")
        print()

    print("=" * 90)
    print(f"Summary: {correct}/{correct + incorrect} on labeled cases  ({unknown} edge cases without expected label)")
    if incorrect > 0:
        print(f"WARNING: {incorrect} prediction(s) disagreed with the obvious answer")
        return 2
    print("All labeled predictions match the expected class")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
