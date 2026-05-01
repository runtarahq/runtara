#!/usr/bin/env python3
"""Refresh checked-in Bedrock AI Agent model metadata.

This script is intentionally opt-in. Normal builds do not contact AWS and do
not require AWS credentials. Release tooling can run it after configuring AWS:

    python3 scripts/update-bedrock-models.py --region us-east-1
"""

from __future__ import annotations

import argparse
import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "crates/runtara-ai/src/providers/bedrock_models.generated.json"


SUPPORTED_STRUCTURED_OUTPUT_MODELS: dict[str, tuple[str, str]] = {
    # AWS Bedrock structured output docs, checked 2026-05-01.
    "anthropic.claude-haiku-4-5-20251001-v1:0": ("Anthropic", "Claude Haiku 4.5"),
    "anthropic.claude-sonnet-4-5-20250929-v1:0": ("Anthropic", "Claude Sonnet 4.5"),
    "anthropic.claude-sonnet-4-6": ("Anthropic", "Claude Sonnet 4.6"),
    "anthropic.claude-opus-4-5-20251101-v1:0": ("Anthropic", "Claude Opus 4.5"),
    "anthropic.claude-opus-4-6-v1": ("Anthropic", "Claude Opus 4.6"),
    "qwen.qwen3-235b-a22b-2507-v1:0": ("Qwen", "Qwen3 235B A22B 2507"),
    "qwen.qwen3-32b-v1:0": ("Qwen", "Qwen3 32B"),
    "qwen.qwen3-coder-30b-a3b-v1:0": ("Qwen", "Qwen3-Coder-30B-A3B-Instruct"),
    "qwen.qwen3-coder-480b-a35b-v1:0": ("Qwen", "Qwen3 Coder 480B A35B Instruct"),
    "qwen.qwen3-next-80b-a3b": ("Qwen", "Qwen3 Next 80B A3B"),
    "qwen.qwen3-vl-235b-a22b": ("Qwen", "Qwen3 VL 235B A22B"),
    "openai.gpt-oss-120b-1:0": ("OpenAI", "gpt-oss-120b"),
    "openai.gpt-oss-20b-1:0": ("OpenAI", "gpt-oss-20b"),
    "openai.gpt-oss-safeguard-120b": ("OpenAI", "GPT OSS Safeguard 120B"),
    "openai.gpt-oss-safeguard-20b": ("OpenAI", "GPT OSS Safeguard 20B"),
    "deepseek.v3-v1:0": ("DeepSeek", "DeepSeek-V3.1"),
    "google.gemma-3-12b-it": ("Google", "Gemma 3 12B IT"),
    "google.gemma-3-27b-it": ("Google", "Gemma 3 27B IT"),
    "minimax.minimax-m2": ("MiniMax", "MiniMax M2"),
    "mistral.magistral-small-2509": ("Mistral AI", "Magistral Small 2509"),
    "mistral.ministral-3-3b-instruct": ("Mistral AI", "Ministral 3B"),
    "mistral.ministral-3-8b-instruct": ("Mistral AI", "Ministral 3 8B"),
    "mistral.ministral-3-14b-instruct": ("Mistral AI", "Ministral 14B 3.0"),
    "mistral.mistral-large-3-675b-instruct": ("Mistral AI", "Mistral Large 3"),
    "mistral.voxtral-mini-3b-2507": ("Mistral AI", "Voxtral Mini 3B 2507"),
    "mistral.voxtral-small-24b-2507": ("Mistral AI", "Voxtral Small 24B 2507"),
    "moonshot.kimi-k2-thinking": ("Moonshot AI", "Kimi K2 Thinking"),
    "nvidia.nemotron-nano-12b-v2": ("NVIDIA", "NVIDIA Nemotron Nano 12B v2 VL BF16"),
    "nvidia.nemotron-nano-9b-v2": ("NVIDIA", "NVIDIA Nemotron Nano 9B v2"),
}


def supports_ai_agent(model_id: str) -> bool:
    """Models with Bedrock structured output support are safe for AI Agent."""
    return model_id in SUPPORTED_STRUCTURED_OUTPUT_MODELS


def load_models(region: str) -> list[dict]:
    command = [
        "aws",
        "bedrock",
        "list-foundation-models",
        "--by-output-modality",
        "TEXT",
        "--region",
        region,
        "--output",
        "json",
    ]
    result = subprocess.run(command, check=True, text=True, capture_output=True)
    payload = json.loads(result.stdout)
    return payload.get("modelSummaries", [])


def normalize(summary: dict) -> dict:
    model_id = summary["modelId"]
    lifecycle = summary.get("modelLifecycle") or {}
    active = lifecycle.get("status", "ACTIVE") == "ACTIVE"
    ai_agent_supported = active and supports_ai_agent(model_id)
    provider, model_name = SUPPORTED_STRUCTURED_OUTPUT_MODELS.get(
        model_id,
        (summary.get("providerName"), summary.get("modelName")),
    )
    return {
        "provider": provider,
        "modelName": model_name,
        "modelId": model_id,
        "lifecycleStatus": lifecycle.get("status", "ACTIVE"),
        "inputModalities": summary.get("inputModalities", []),
        "outputModalities": summary.get("outputModalities", []),
        "supportsConverse": ai_agent_supported,
        "supportsSystemPrompt": ai_agent_supported,
        "supportsToolUse": ai_agent_supported,
        "supportsStructuredOutput": ai_agent_supported,
        "recommendedForAiAgent": ai_agent_supported,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--region", default="us-east-1")
    args = parser.parse_args()

    models = [normalize(summary) for summary in load_models(args.region)]
    models = [m for m in models if m["recommendedForAiAgent"]]
    models.sort(key=lambda m: (m.get("provider") or "", m.get("modelName") or ""))

    catalog = {
        "generatedAt": datetime.now(timezone.utc).isoformat(),
        "source": f"aws bedrock list-foundation-models --by-output-modality TEXT --region {args.region}",
        "sourceUrls": [
            "https://docs.aws.amazon.com/bedrock/latest/userguide/structured-output.html",
            "https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-anthropic-claude-sonnet-4-6.html",
            "https://docs.aws.amazon.com/bedrock/latest/userguide/tool-use.html",
            "https://docs.aws.amazon.com/bedrock/latest/APIReference/API_ListFoundationModels.html",
        ],
        "models": models,
    }

    OUT.write_text(json.dumps(catalog, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote {len(models)} Bedrock AI Agent models to {OUT}")


if __name__ == "__main__":
    main()
