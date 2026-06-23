#!/usr/bin/env python3
"""Pre-download and verify VibeVoice models for Docker image."""

import os
import time
from huggingface_hub import snapshot_download

# Use HF_HOME from environment (set in Dockerfile), fallback to default
hf_home = os.environ.get('HF_HOME', '/workspace/.cache/huggingface')
os.environ['HF_HOME'] = hf_home


def download_with_retry(model_id, max_retries=3):
    for attempt in range(max_retries):
        try:
            print(f"Downloading {model_id}...")
            path = snapshot_download(model_id)
            print(f"  Success: {path}")
            return path
        except Exception as e:
            print(f"  Attempt {attempt + 1}/{max_retries} failed: {e}")
            if attempt < max_retries - 1:
                time.sleep(5)
    raise RuntimeError(f"Failed to download {model_id} after {max_retries} attempts")


def verify_models():
    """Load models to verify they work."""
    print("\nVerifying models can be loaded...")
    try:
        print("Loading VibeVoice processor...")
        from transformers import AutoProcessor
        processor = AutoProcessor.from_pretrained('vibevoice/VibeVoice-1.5B')
        print("  ✓ VibeVoice processor loaded")
    except Exception as e:
        print(f"  ✗ Failed to load VibeVoice: {e}")
        raise

    try:
        print("Loading Qwen2.5-1.5B tokenizer...")
        from transformers import AutoTokenizer
        tokenizer = AutoTokenizer.from_pretrained('Qwen/Qwen2.5-1.5B')
        print("  ✓ Qwen2.5-1.5B tokenizer loaded")
    except Exception as e:
        print(f"  ✗ Failed to load Qwen tokenizer: {e}")
        raise


if __name__ == '__main__':
    download_with_retry('vibevoice/VibeVoice-1.5B')
    download_with_retry('Qwen/Qwen2.5-1.5B')
    verify_models()
    print("\n✓ All models downloaded and verified!")
