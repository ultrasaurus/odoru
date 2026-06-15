#!/bin/bash
set -e
cd "$(dirname "$0")/.."
: > data/augment_traveling_normalized.txt
while IFS= read -r line; do
  cargo run -q -- normalize "$line" >> data/augment_traveling_normalized.txt
done < data/augment_traveling.txt
