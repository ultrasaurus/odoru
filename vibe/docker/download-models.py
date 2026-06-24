#!/usr/bin/env python3
"""Pre-download a single HF model into the Docker image. Pass model_id as argv[1]."""

import os
import sys
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


if __name__ == '__main__':
    if len(sys.argv) != 2:
        print("Usage: download-models.py <model_id>")
        sys.exit(1)
    download_with_retry(sys.argv[1])
