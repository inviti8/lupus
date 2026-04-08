"""Lupus TinyAgent planner LoRA training script.

Fine-tunes `squeeze-ai-lab/TinyAgent-1.1B` with a small LoRA adapter on the
354-example planner training dataset built by `tools/build_planner_dataset.py`.
The goal is to shift the model's prior away from BAIR's Apple-app tool surface
(`compose_new_email`, `send_sms`, `summarize_pdf`, etc.) toward Lupus's 6-tool
surface, while preserving the LLMCompiler grammar BAIR baked in (numbered plans,
$N references, join() terminator).

Why a LoRA and not a full fine-tune:
  - Cheap (~5-15 MB adapter, trains in < 30 min on a single 4090)
  - Doesn't disturb the base model's capabilities — the rank-16 constraint
    only modifies a small subspace of the q/k/v/o projection matrices
  - Hot-swappable at inference (the daemon scaffold already supports this
    via `daemon/src/agent.rs::ADAPTER_SEARCH`)

Loss masking: this is instruction tuning, so we want the loss computed only
on the assistant response tokens (the LLMCompiler plan), not on the system
prompt or user query. The PlannerJsonlDataset class handles this by tokenizing
the prompt portion separately and setting `labels = -100` for those tokens.

The canonical system prompt is built at training time via
`tinyagent_prompt_probe.build_planner_system_prompt(LUPUS_TOOLS)`, which means
any change to the system prompt automatically propagates to training without
rewriting the dataset.

Hard guardrail: the eval script's syntactic validity metric must stay at 100%
post-training. If LoRA training corrupts the LLMCompiler grammar, that metric
catches it before we ship the adapter. Validation is done by running
`tools/eval_tinyagent.py` against the held-out 22 cases AFTER pulling the
adapter — not during training (generation is too slow for an in-loop check).

Usage on the RunPod pod:
    python training/train_planner.py
    python training/train_planner.py --resume
    python training/train_planner.py --epochs 5 --batch-size 8

The dataset is committed to git (138 KB at datasets/search/planner_train.jsonl)
so the pod gets it for free when it clones the repo. No S3 round-trip needed
for the dataset; only checkpoints and the final adapter go through S3.

Environment requirements:
    .env at the repo root with S3, HF, and (optionally) W&B credentials.
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "training"))
sys.path.insert(0, str(REPO_ROOT / "tools"))
sys.path.insert(0, str(REPO_ROOT))

import torch
from torch.utils.data import Dataset

from s3_utils import (  # noqa: E402
    download_directory,
    find_latest_checkpoint,
    load_env,
    upload_directory,
)

LOG = logging.getLogger("train_planner")


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------


# The non-GGUF version (full precision weights) — needed for LoRA training.
# We'll attach the trained adapter to the GGUF version at inference time.
BASE_MODEL = "squeeze-ai-lab/TinyAgent-1.1B"

# squeeze-ai-lab/TinyAgent-1.1B contains ONLY the model weights and config —
# no tokenizer files. The model is fine-tuned from
# Doctor-Shotgun/TinyLlama-1.1B-32k-Instruct (per the model card and the
# 32K context size in config.json), so we load the tokenizer from there.
# The two repos share the same vocab (LlamaTokenizer, vocab_size=32000).
BASE_TOKENIZER = "Doctor-Shotgun/TinyLlama-1.1B-32k-Instruct"

# CRITICAL: TinyAgent's GGUF embeds a custom chat template that does flat
# concatenation of system + user + assistant content with NO role markers
# and NO separators. We extracted it from the GGUF metadata (see
# `dist/tinyagent/TinyAgent-1.1B-Q4_K_M.gguf::tokenizer.chat_template`)
# and embed it here verbatim. The Doctor-Shotgun TinyLlama tokenizer ships
# an Alpaca-style template that would NOT match inference — using it would
# train the model on a different prompt format than inference uses.
#
# Verified renders for [system, user, assistant=plan]:
#   {system}{user}{plan}\n
# For [system, user] (the prompt portion used for loss masking boundary):
#   {system}{user}
TINYAGENT_CHAT_TEMPLATE = (
    "{% if messages[0]['role'] == 'system' %}"
    "{% set system_message = messages[0]['content'] %}"
    "{% endif %}"
    "{% if system_message is defined %}{{ system_message }}{% endif %}"
    "{% for message in messages %}"
    "{% set content = message['content'] %}"
    "{% if message['role'] == 'user' %}{{ content }}"
    "{% elif message['role'] == 'assistant' %}{{ content + '\n' }}"
    "{% endif %}"
    "{% endfor %}"
)

# LoRA hyperparameters from base/config.yaml::training.search_adapter
LORA_RANK = 16
LORA_ALPHA = 32
LORA_DROPOUT = 0.05
LORA_TARGET_MODULES = ["q_proj", "v_proj", "k_proj", "o_proj"]

# Loss-masking sentinel: HuggingFace ignores tokens labeled -100
IGNORE_INDEX = -100

# S3 paths
S3_CHECKPOINT_PREFIX = "models/lupus-tinyagent/checkpoints"
S3_FINAL_PREFIX = "models/lupus-tinyagent/final"

# Local paths (on the pod)
LOCAL_OUTPUT_DIR = REPO_ROOT / "training" / "output" / "lupus-tinyagent"
LOCAL_FINAL_DIR = REPO_ROOT / "dist" / "lupus-tinyagent-search"
LOCAL_DATASET_PATH = REPO_ROOT / "datasets" / "search" / "planner_train.jsonl"


# ---------------------------------------------------------------------------
# Dataset
# ---------------------------------------------------------------------------


class PlannerJsonlDataset(Dataset):
    """Loads PlannerExample records from JSONL, prepends the canonical Lupus
    system prompt, applies the model's chat template, tokenizes, and masks
    the loss on everything before the assistant turn.

    The dataset format on disk has only [user, assistant] messages — the
    system prompt is added here so it's always in lock-step with the version
    used at inference (`tools/tinyagent_prompt_probe.py::build_planner_system_prompt`)."""

    def __init__(self, jsonl_path: Path, tokenizer, system_prompt: str, max_length: int = 2048):
        self.examples: list[dict] = []
        with jsonl_path.open("r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if "messages" not in rec or len(rec["messages"]) < 2:
                    continue
                self.examples.append(rec)
        self.tokenizer = tokenizer
        self.system_prompt = system_prompt
        self.max_length = max_length
        LOG.info("Loaded %d examples from %s", len(self.examples), jsonl_path)

    def __len__(self) -> int:
        return len(self.examples)

    def __getitem__(self, idx: int) -> dict:
        rec = self.examples[idx]

        # Prepend the canonical system prompt to the [user, assistant] turns
        # we have on disk. This is the SAME system prompt the inference path
        # builds via `build_planner_system_prompt(LUPUS_TOOLS)`.
        messages = [{"role": "system", "content": self.system_prompt}]
        for m in rec["messages"]:
            messages.append({"role": m["role"], "content": m["content"]})

        # Render the prompt portion (system + user only) so we know where the
        # assistant response starts. add_generation_prompt=True appends the
        # template's "I'm about to generate the assistant turn" marker, which
        # is exactly what the model sees at inference.
        prompt_messages = messages[:-1]
        prompt_text = self.tokenizer.apply_chat_template(
            prompt_messages,
            tokenize=False,
            add_generation_prompt=True,
        )

        # Render the full conversation including the assistant response.
        full_text = self.tokenizer.apply_chat_template(
            messages,
            tokenize=False,
            add_generation_prompt=False,
        )

        # Tokenize both. Crucially, the prompt_text MUST be a strict prefix
        # of full_text (modulo the assistant turn) — apply_chat_template
        # guarantees this for any well-formed chat template.
        prompt_tokens = self.tokenizer(
            prompt_text, add_special_tokens=False
        )["input_ids"]
        full_tokens = self.tokenizer(
            full_text,
            add_special_tokens=False,
            truncation=True,
            max_length=self.max_length,
        )["input_ids"]

        # Build labels: -100 for prompt tokens (no loss), real ids for the
        # assistant tokens. If truncation chopped into the prompt itself
        # (shouldn't happen with our 354 small examples + max_length=2048),
        # the label list is just all -100 and the example contributes nothing.
        labels = list(full_tokens)
        prompt_len = min(len(prompt_tokens), len(full_tokens))
        for i in range(prompt_len):
            labels[i] = IGNORE_INDEX

        return {
            "input_ids": full_tokens,
            "labels": labels,
            "attention_mask": [1] * len(full_tokens),
        }


# ---------------------------------------------------------------------------
# Padding collator with label masking
# ---------------------------------------------------------------------------


class PaddedCollator:
    """Pad input_ids, labels, and attention_mask to the longest in the batch.
    Padded label positions are set to IGNORE_INDEX so they don't contribute
    to the loss."""

    def __init__(self, pad_token_id: int):
        self.pad_token_id = pad_token_id

    def __call__(self, features: list[dict]) -> dict:
        max_len = max(len(f["input_ids"]) for f in features)
        out_input_ids: list[list[int]] = []
        out_labels: list[list[int]] = []
        out_attention: list[list[int]] = []
        for f in features:
            pad_len = max_len - len(f["input_ids"])
            out_input_ids.append(f["input_ids"] + [self.pad_token_id] * pad_len)
            out_labels.append(f["labels"] + [IGNORE_INDEX] * pad_len)
            out_attention.append(f["attention_mask"] + [0] * pad_len)
        return {
            "input_ids": torch.tensor(out_input_ids, dtype=torch.long),
            "labels": torch.tensor(out_labels, dtype=torch.long),
            "attention_mask": torch.tensor(out_attention, dtype=torch.long),
        }


# ---------------------------------------------------------------------------
# S3 checkpoint callback
# ---------------------------------------------------------------------------


def make_s3_callback(s3_prefix: str):
    from transformers import TrainerCallback

    class S3CheckpointCallback(TrainerCallback):
        def on_save(self, args, state, control, **kwargs):
            ckpt_dir = Path(args.output_dir) / f"checkpoint-{state.global_step}"
            if not ckpt_dir.exists():
                LOG.warning("on_save fired but checkpoint dir not found: %s", ckpt_dir)
                return
            try:
                upload_directory(
                    ckpt_dir,
                    f"{s3_prefix}/checkpoint-{state.global_step}",
                )
                LOG.info("Uploaded checkpoint-%d to S3", state.global_step)
            except Exception as e:
                LOG.error("Failed to upload checkpoint to S3: %s", e)

    return S3CheckpointCallback


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--epochs", type=int, default=3)
    parser.add_argument("--batch-size", type=int, default=4)
    parser.add_argument("--learning-rate", type=float, default=2e-4)
    parser.add_argument("--warmup-ratio", type=float, default=0.1)
    parser.add_argument("--max-grad-norm", type=float, default=1.0)
    parser.add_argument("--max-length", type=int, default=2048,
                        help="Tokenizer max length (system prompt is ~700 tokens)")
    parser.add_argument("--save-steps", type=int, default=50)
    parser.add_argument("--eval-steps", type=int, default=25)
    parser.add_argument("--logging-steps", type=int, default=10)
    parser.add_argument("--val-frac", type=float, default=0.1,
                        help="Fraction of training data held out for eval_loss tracking")
    parser.add_argument("--lora-rank", type=int, default=LORA_RANK)
    parser.add_argument("--lora-alpha", type=int, default=LORA_ALPHA)
    parser.add_argument("--lora-dropout", type=float, default=LORA_DROPOUT)
    parser.add_argument("--resume", action="store_true",
                        help="Resume from the latest S3 checkpoint if any")
    parser.add_argument("--no-wandb", action="store_true")
    parser.add_argument("--no-s3", action="store_true",
                        help="Skip S3 checkpoint upload (for local testing)")
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )
    load_env()

    # Late imports — heavy ML deps come after env is loaded
    from peft import LoraConfig, TaskType, get_peft_model
    from transformers import (
        AutoModelForCausalLM,
        AutoTokenizer,
        Trainer,
        TrainingArguments,
        set_seed,
    )

    # Import the canonical system prompt builder. This is the same prompt
    # the inference eval (tools/eval_tinyagent.py) uses, so training and
    # inference are guaranteed to be in lock-step.
    from tinyagent_prompt_probe import LUPUS_TOOLS, build_planner_system_prompt

    set_seed(args.seed)

    # ------------------------------------------------------------------
    # GPU sanity check
    # ------------------------------------------------------------------
    if not torch.cuda.is_available():
        LOG.warning("No CUDA device. Training will be CPU-only and slow.")
    else:
        LOG.info(
            "CUDA: %s (%d GB)",
            torch.cuda.get_device_name(0),
            torch.cuda.get_device_properties(0).total_memory // (1024 ** 3),
        )

    # ------------------------------------------------------------------
    # Tokenizer (loaded from BASE_TOKENIZER, NOT BASE_MODEL — see notes
    # at the top of this file). Override the chat template with the exact
    # one embedded in TinyAgent's GGUF so training and inference use the
    # same prompt format.
    # ------------------------------------------------------------------
    LOG.info("Loading tokenizer: %s", BASE_TOKENIZER)
    tokenizer = AutoTokenizer.from_pretrained(BASE_TOKENIZER)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token
    LOG.info("Overriding tokenizer.chat_template with TinyAgent GGUF template")
    tokenizer.chat_template = TINYAGENT_CHAT_TEMPLATE

    # ------------------------------------------------------------------
    # Base model
    # ------------------------------------------------------------------
    LOG.info("Loading base model: %s", BASE_MODEL)
    model = AutoModelForCausalLM.from_pretrained(
        BASE_MODEL,
        torch_dtype=torch.bfloat16 if torch.cuda.is_available() else torch.float32,
        trust_remote_code=True,
    )
    model.config.pad_token_id = tokenizer.pad_token_id

    # ------------------------------------------------------------------
    # LoRA wrap
    # ------------------------------------------------------------------
    LOG.info(
        "Applying LoRA: rank=%d alpha=%d dropout=%.2f targets=%s",
        args.lora_rank, args.lora_alpha, args.lora_dropout, LORA_TARGET_MODULES,
    )
    lora_config = LoraConfig(
        task_type=TaskType.CAUSAL_LM,
        r=args.lora_rank,
        lora_alpha=args.lora_alpha,
        lora_dropout=args.lora_dropout,
        target_modules=LORA_TARGET_MODULES,
        bias="none",
    )
    model = get_peft_model(model, lora_config)
    model.print_trainable_parameters()

    # ------------------------------------------------------------------
    # System prompt — built once, shared across all examples
    # ------------------------------------------------------------------
    system_prompt = build_planner_system_prompt(LUPUS_TOOLS)
    sys_tokens = len(tokenizer.encode(system_prompt, add_special_tokens=False))
    LOG.info("System prompt: %d chars / %d tokens", len(system_prompt), sys_tokens)

    # ------------------------------------------------------------------
    # Dataset
    # ------------------------------------------------------------------
    if not LOCAL_DATASET_PATH.exists():
        LOG.error("Dataset not found at %s", LOCAL_DATASET_PATH)
        LOG.error("  Run: python tools/build_planner_dataset.py")
        return 1

    full_ds = PlannerJsonlDataset(
        LOCAL_DATASET_PATH,
        tokenizer,
        system_prompt,
        max_length=args.max_length,
    )

    # 90/10 train/val split. The 22 eval cases in tools/eval_tinyagent.py
    # are entirely separate — they're the held-out generalization test, run
    # AFTER training. The val set here is just for tracking eval_loss during
    # training to detect overfitting / catastrophic forgetting.
    n = len(full_ds)
    n_val = max(1, int(n * args.val_frac))
    n_train = n - n_val
    LOG.info("Train/val split: %d / %d (val_frac=%.2f)", n_train, n_val, args.val_frac)

    rng = torch.Generator().manual_seed(args.seed)
    train_ds, eval_ds = torch.utils.data.random_split(
        full_ds, [n_train, n_val], generator=rng
    )

    # ------------------------------------------------------------------
    # Trainer setup
    # ------------------------------------------------------------------
    LOCAL_OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    report_to = "wandb"
    if args.no_wandb or os.environ.get("WANDB_DISABLED", "").lower() == "true":
        report_to = "none"

    # save_steps must be a multiple of eval_steps when load_best_model_at_end=True.
    # Auto-round up so the user can pass arbitrary values.
    save_steps = args.save_steps
    if save_steps % args.eval_steps != 0:
        rounded = ((save_steps // args.eval_steps) + 1) * args.eval_steps
        LOG.info(
            "Auto-rounding save_steps from %d to %d (multiple of eval_steps=%d)",
            save_steps, rounded, args.eval_steps,
        )
        save_steps = rounded

    training_args = TrainingArguments(
        output_dir=str(LOCAL_OUTPUT_DIR),
        num_train_epochs=args.epochs,
        per_device_train_batch_size=args.batch_size,
        per_device_eval_batch_size=args.batch_size,
        learning_rate=args.learning_rate,
        warmup_ratio=args.warmup_ratio,
        max_grad_norm=args.max_grad_norm,
        logging_steps=args.logging_steps,
        eval_strategy="steps",
        eval_steps=args.eval_steps,
        save_strategy="steps",
        save_steps=save_steps,
        save_total_limit=3,
        load_best_model_at_end=True,
        metric_for_best_model="eval_loss",
        greater_is_better=False,
        report_to=report_to,
        run_name="lupus-tinyagent-planner",
        seed=args.seed,
        bf16=torch.cuda.is_available(),
        dataloader_num_workers=2,
        remove_unused_columns=False,  # we have custom column names
    )

    callbacks = []
    if not args.no_s3:
        callbacks.append(make_s3_callback(S3_CHECKPOINT_PREFIX)())

    collator = PaddedCollator(pad_token_id=tokenizer.pad_token_id)

    # In transformers >= 4.46, the Trainer's tokenizer kwarg was renamed to
    # processing_class; in 5.x the old name was removed. Try the new name
    # first, fall back to the old.
    trainer_kwargs = dict(
        model=model,
        args=training_args,
        train_dataset=train_ds,
        eval_dataset=eval_ds,
        data_collator=collator,
        callbacks=callbacks,
    )
    try:
        trainer = Trainer(**trainer_kwargs, processing_class=tokenizer)
    except TypeError as e:
        if "processing_class" in str(e):
            trainer = Trainer(**trainer_kwargs, tokenizer=tokenizer)
        else:
            raise

    # ------------------------------------------------------------------
    # Resume from S3 if requested
    # ------------------------------------------------------------------
    resume_from = None
    if args.resume and not args.no_s3:
        latest_s3_ckpt = find_latest_checkpoint(S3_CHECKPOINT_PREFIX)
        if latest_s3_ckpt:
            step = int(latest_s3_ckpt.rsplit("-", 1)[-1])
            local_resume = LOCAL_OUTPUT_DIR / f"checkpoint-{step}"
            if not local_resume.exists():
                LOG.info("Pulling checkpoint from s3://.../%s", latest_s3_ckpt)
                download_directory(latest_s3_ckpt, local_resume)
            resume_from = str(local_resume)
            LOG.info("Resuming from %s", resume_from)
        else:
            LOG.info("--resume requested but no S3 checkpoint found; starting fresh")

    # ------------------------------------------------------------------
    # Train
    # ------------------------------------------------------------------
    LOG.info("Starting training...")
    trainer.train(resume_from_checkpoint=resume_from)

    # ------------------------------------------------------------------
    # Final eval (loss only — generation-based eval is run separately by
    # tools/eval_tinyagent.py against the held-out 22 cases)
    # ------------------------------------------------------------------
    LOG.info("Final loss-based evaluation on the held-out val set...")
    final_metrics = trainer.evaluate()
    LOG.info("Final metrics:")
    for k, v in sorted(final_metrics.items()):
        if isinstance(v, float):
            LOG.info("  %-30s %.4f", k, v)

    # ------------------------------------------------------------------
    # Save the LoRA adapter (NOT the merged model — we want to ship the
    # small adapter alongside the base GGUF)
    # ------------------------------------------------------------------
    LOCAL_FINAL_DIR.mkdir(parents=True, exist_ok=True)
    LOG.info("Saving LoRA adapter to %s", LOCAL_FINAL_DIR)
    model.save_pretrained(str(LOCAL_FINAL_DIR))
    tokenizer.save_pretrained(str(LOCAL_FINAL_DIR))

    (LOCAL_FINAL_DIR / "eval_metrics.json").write_text(
        json.dumps(final_metrics, indent=2),
        encoding="utf-8",
    )

    if not args.no_s3:
        LOG.info("Uploading final adapter to S3...")
        upload_directory(LOCAL_FINAL_DIR, S3_FINAL_PREFIX)

    LOG.info("Done.")
    LOG.info(
        "Next: pull the adapter locally with `python training/pull_model.py "
        "--model tinyagent`, convert to GGUF with llama.cpp's "
        "`convert_lora_to_gguf.py`, attach to dist/tinyagent/TinyAgent-1.1B-Q4_K_M.gguf "
        "via llama-cpp-python's `lora_path` parameter, and re-run "
        "`tools/eval_tinyagent.py` against the held-out 22 cases."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
