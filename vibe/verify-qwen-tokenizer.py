#!/usr/bin/env python3
"""Verify Qwen2.5-1.5B tokenizer loads correctly (catches tokenizer/transformers
version mismatches at build time instead of at inference time)."""

import os

hf_home = os.environ.get('HF_HOME', '/workspace/.cache/huggingface')
os.environ['HF_HOME'] = hf_home

from transformers import AutoTokenizer

print("Verifying Qwen2.5-1.5B tokenizer can be loaded...")
tokenizer = AutoTokenizer.from_pretrained('Qwen/Qwen2.5-1.5B')
print("  Qwen2.5-1.5B tokenizer loaded successfully")
