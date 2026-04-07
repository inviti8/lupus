"""Lupus security model training script.

Fine-tunes Qwen2.5-Coder-0.5B as a 4-class URL classifier
(safe / phishing / malware / suspicious) using a classification head
on top of the base model.

Stage 1 design (URL features only — no HTML body):
  - Input: the URL string itself, tokenized (max 128 tokens)
  - Output: 4-class classification head with weighted CrossEntropy loss
  - Training: full fine-tune (model is small enough)
  - Evaluation: per-class precision/recall/F1, false positive rate

Class imbalance is handled via class weights in the loss function and
optionally also via WeightedRandomSampler at the data layer.

Checkpoints are saved every --save-steps to local disk and uploaded to
S3 immediately so spot-instance interruptions are recoverable.

Usage on the RunPod pod:
    python training/train_security.py
    python training/train_security.py --resume
    python training/train_security.py --epochs 5 --batch-size 64

Environment requirements:
    .env at the repo root with S3, HF, and (optionally) W&B credentials.
    See .env.example.
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import shutil
import sys
from pathlib import Path
from typing import Optional

# Local imports
REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "training"))
sys.path.insert(0, str(REPO_ROOT / "datasets" / "security"))

import numpy as np
import torch
from torch.utils.data import Dataset, WeightedRandomSampler

from s3_utils import (  # noqa: E402
    download_directory,
    find_latest_checkpoint,
    load_env,
    upload_directory,
)
from schema import Label, SecurityExample  # noqa: E402

LOG = logging.getLogger("train_security")


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------


BASE_MODEL = "Qwen/Qwen2.5-Coder-0.5B"

# Label mapping: classification head order
LABEL_LIST = ["safe", "phishing", "malware", "suspicious"]
LABEL_TO_ID = {name: idx for idx, name in enumerate(LABEL_LIST)}
ID_TO_LABEL = {idx: name for idx, name in enumerate(LABEL_LIST)}

# S3 paths
S3_DATASET_PREFIX = "datasets/security"
S3_CHECKPOINT_PREFIX = "models/lupus-security/checkpoints"
S3_FINAL_PREFIX = "models/lupus-security/final"

# Local paths (on the pod)
LOCAL_OUTPUT_DIR = REPO_ROOT / "training" / "output" / "lupus-security"
LOCAL_FINAL_DIR = REPO_ROOT / "dist" / "lupus-security"


# ---------------------------------------------------------------------------
# Dataset
# ---------------------------------------------------------------------------


class SecurityJsonlDataset(Dataset):
    """A torch Dataset that streams SecurityExample records from JSONL.

    For Stage 1 (URL-only) training, we tokenize just the URL string
    plus a small format prefix to give the model a consistent input
    structure. HTML body is ignored at this stage.
    """

    def __init__(self, jsonl_path: Path, tokenizer, max_length: int = 128):
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
                self.examples.append({
                    "url": rec["url"],
                    "domain": rec.get("domain", ""),
                    "label_id": LABEL_TO_ID[rec["label"]],
                })
        self.tokenizer = tokenizer
        self.max_length = max_length
        LOG.info("Loaded %d examples from %s", len(self.examples), jsonl_path)

    def __len__(self) -> int:
        return len(self.examples)

    def __getitem__(self, idx: int) -> dict:
        ex = self.examples[idx]
        # Format the input: a short prefix plus the URL.
        # The prefix gives the model a stable signal that this is a URL
        # to be classified, and helps with token boundary alignment.
        text = f"URL: {ex['url']}"
        encoding = self.tokenizer(
            text,
            truncation=True,
            max_length=self.max_length,
            padding=False,
            return_tensors=None,
        )
        encoding["labels"] = ex["label_id"]
        return encoding

    def label_counts(self) -> dict[int, int]:
        counts: dict[int, int] = {}
        for ex in self.examples:
            counts[ex["label_id"]] = counts.get(ex["label_id"], 0) + 1
        return counts

    def sample_weights(self) -> list[float]:
        """Per-example weights for WeightedRandomSampler."""
        counts = self.label_counts()
        n = len(self.examples)
        # Inverse-frequency weight
        return [n / counts[ex["label_id"]] / len(counts) for ex in self.examples]


# ---------------------------------------------------------------------------
# Metrics
# ---------------------------------------------------------------------------


def compute_metrics(eval_pred) -> dict[str, float]:
    """Compute per-class precision/recall/F1 + safe-class FPR."""
    from sklearn.metrics import (
        accuracy_score,
        confusion_matrix,
        precision_recall_fscore_support,
    )

    predictions, labels = eval_pred
    if isinstance(predictions, tuple):
        predictions = predictions[0]
    preds = np.asarray(predictions).argmax(axis=-1)
    labels = np.asarray(labels)

    metrics: dict[str, float] = {}
    metrics["accuracy"] = accuracy_score(labels, preds)

    precision, recall, f1, _ = precision_recall_fscore_support(
        labels, preds,
        labels=list(range(len(LABEL_LIST))),
        average=None,
        zero_division=0,
    )
    for i, name in enumerate(LABEL_LIST):
        metrics[f"precision_{name}"] = float(precision[i])
        metrics[f"recall_{name}"] = float(recall[i])
        metrics[f"f1_{name}"] = float(f1[i])

    # Macro F1 — equally weights all classes (important for imbalanced data)
    metrics["f1_macro"] = float(f1.mean())

    # False positive rate on safe class:
    # of all the actually-safe URLs, how many did we wrongly flag as a threat?
    cm = confusion_matrix(labels, preds, labels=list(range(len(LABEL_LIST))))
    safe_idx = LABEL_TO_ID["safe"]
    safe_total = cm[safe_idx].sum()
    if safe_total > 0:
        # All non-safe predictions for actually-safe rows = false positives
        false_positives = cm[safe_idx].sum() - cm[safe_idx, safe_idx]
        metrics["false_positive_rate"] = float(false_positives / safe_total)
    else:
        metrics["false_positive_rate"] = 0.0

    return metrics


# ---------------------------------------------------------------------------
# Class-weighted Trainer
# ---------------------------------------------------------------------------


def make_trainer_class(class_weights: torch.Tensor):
    """Create a Trainer subclass with class-weighted CrossEntropyLoss."""
    from transformers import Trainer

    class WeightedTrainer(Trainer):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            self._class_weights = class_weights.to(self.args.device)

        def compute_loss(self, model, inputs, return_outputs=False, **kwargs):
            labels = inputs.pop("labels")
            outputs = model(**inputs)
            logits = outputs.logits
            loss_fn = torch.nn.CrossEntropyLoss(weight=self._class_weights)
            loss = loss_fn(logits.view(-1, len(LABEL_LIST)), labels.view(-1))
            return (loss, outputs) if return_outputs else loss

    return WeightedTrainer


# ---------------------------------------------------------------------------
# S3 checkpoint callback
# ---------------------------------------------------------------------------


def make_s3_callback(s3_prefix: str):
    """Create a TrainerCallback that uploads each saved checkpoint to S3."""
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
    parser.add_argument("--batch-size", type=int, default=32)
    parser.add_argument("--learning-rate", type=float, default=5e-5)
    parser.add_argument("--warmup-ratio", type=float, default=0.05)
    parser.add_argument("--max-length", type=int, default=128,
                        help="Tokenizer max length (URLs are short)")
    parser.add_argument("--save-steps", type=int, default=200)
    parser.add_argument("--eval-steps", type=int, default=100)
    parser.add_argument("--logging-steps", type=int, default=20)
    parser.add_argument("--resume", action="store_true",
                        help="Resume from the latest S3 checkpoint if any")
    parser.add_argument("--no-wandb", action="store_true",
                        help="Disable wandb even if WANDB_API_KEY is set")
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

    # Late import — heavy ML deps come after env is loaded
    import torch
    from transformers import (
        AutoModelForSequenceClassification,
        AutoTokenizer,
        DataCollatorWithPadding,
        TrainingArguments,
        set_seed,
    )

    set_seed(args.seed)

    # ------------------------------------------------------------------
    # Sanity check: GPU
    # ------------------------------------------------------------------
    if not torch.cuda.is_available():
        LOG.warning("No CUDA device detected. Training will be CPU-only and slow.")
    else:
        LOG.info("CUDA device: %s (%d GB)",
                 torch.cuda.get_device_name(0),
                 torch.cuda.get_device_properties(0).total_memory // (1024 ** 3))

    # ------------------------------------------------------------------
    # Tokenizer + model
    # ------------------------------------------------------------------
    LOG.info("Loading tokenizer: %s", BASE_MODEL)
    tokenizer = AutoTokenizer.from_pretrained(BASE_MODEL, trust_remote_code=True)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token

    LOG.info("Loading model with classification head: %s", BASE_MODEL)
    model = AutoModelForSequenceClassification.from_pretrained(
        BASE_MODEL,
        num_labels=len(LABEL_LIST),
        id2label=ID_TO_LABEL,
        label2id=LABEL_TO_ID,
        torch_dtype=torch.float32,
        trust_remote_code=True,
    )
    model.config.pad_token_id = tokenizer.pad_token_id

    # ------------------------------------------------------------------
    # Datasets
    # ------------------------------------------------------------------
    train_path = REPO_ROOT / "datasets" / "security" / "examples" / "train.jsonl"
    eval_path = REPO_ROOT / "datasets" / "security" / "examples" / "eval.jsonl"

    if not train_path.exists() or not eval_path.exists():
        LOG.error("Dataset files not found. Did you run setup_pod.sh?")
        LOG.error("  Expected: %s", train_path)
        LOG.error("  Expected: %s", eval_path)
        return 1

    train_ds = SecurityJsonlDataset(train_path, tokenizer, max_length=args.max_length)
    eval_ds = SecurityJsonlDataset(eval_path, tokenizer, max_length=args.max_length)

    train_counts = train_ds.label_counts()
    LOG.info("Training class distribution:")
    for i, name in enumerate(LABEL_LIST):
        count = train_counts.get(i, 0)
        pct = 100 * count / max(len(train_ds), 1)
        LOG.info("  %-12s %6d  (%5.2f%%)", name, count, pct)

    # ------------------------------------------------------------------
    # Class weights for the loss
    # ------------------------------------------------------------------
    n = len(train_ds)
    weights = []
    for i in range(len(LABEL_LIST)):
        c = train_counts.get(i, 0)
        if c == 0:
            weights.append(0.0)
        else:
            # Inverse frequency, normalized so that mean weight = 1
            weights.append(n / (len(LABEL_LIST) * c))
    class_weights = torch.tensor(weights, dtype=torch.float32)
    LOG.info("Loss class weights: %s",
             {LABEL_LIST[i]: round(w, 2) for i, w in enumerate(weights)})

    # ------------------------------------------------------------------
    # Trainer
    # ------------------------------------------------------------------
    LOCAL_OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    report_to = "wandb"
    if args.no_wandb or os.environ.get("WANDB_DISABLED", "").lower() == "true":
        report_to = "none"

    training_args = TrainingArguments(
        output_dir=str(LOCAL_OUTPUT_DIR),
        num_train_epochs=args.epochs,
        per_device_train_batch_size=args.batch_size,
        per_device_eval_batch_size=args.batch_size * 2,
        learning_rate=args.learning_rate,
        warmup_ratio=args.warmup_ratio,
        logging_steps=args.logging_steps,
        eval_strategy="steps",
        eval_steps=args.eval_steps,
        save_strategy="steps",
        save_steps=args.save_steps,
        save_total_limit=3,
        load_best_model_at_end=True,
        metric_for_best_model="f1_macro",
        greater_is_better=True,
        report_to=report_to,
        run_name="lupus-security-stage1",
        seed=args.seed,
        bf16=torch.cuda.is_available(),
        dataloader_num_workers=2,
    )

    data_collator = DataCollatorWithPadding(tokenizer=tokenizer)

    callbacks = []
    if not args.no_s3:
        callbacks.append(make_s3_callback(S3_CHECKPOINT_PREFIX)())

    trainer_cls = make_trainer_class(class_weights)
    trainer = trainer_cls(
        model=model,
        args=training_args,
        train_dataset=train_ds,
        eval_dataset=eval_ds,
        tokenizer=tokenizer,
        data_collator=data_collator,
        compute_metrics=compute_metrics,
        callbacks=callbacks,
    )

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
    # Final evaluation
    # ------------------------------------------------------------------
    LOG.info("Running final evaluation...")
    final_metrics = trainer.evaluate()
    LOG.info("Final metrics:")
    for k, v in sorted(final_metrics.items()):
        if isinstance(v, float):
            LOG.info("  %-30s %.4f", k, v)

    # ------------------------------------------------------------------
    # Save final model and push to S3
    # ------------------------------------------------------------------
    LOCAL_FINAL_DIR.mkdir(parents=True, exist_ok=True)
    LOG.info("Saving final model to %s", LOCAL_FINAL_DIR)
    trainer.save_model(str(LOCAL_FINAL_DIR))
    tokenizer.save_pretrained(str(LOCAL_FINAL_DIR))

    # Save final metrics alongside the model
    (LOCAL_FINAL_DIR / "eval_metrics.json").write_text(
        json.dumps(final_metrics, indent=2),
        encoding="utf-8",
    )

    if not args.no_s3:
        LOG.info("Uploading final model to S3...")
        upload_directory(LOCAL_FINAL_DIR, S3_FINAL_PREFIX)

    LOG.info("Done.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
