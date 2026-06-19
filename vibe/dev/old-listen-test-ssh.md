# Old listen test procedure (SSH-based)

This is the manual SSH workflow used before `vibe-service` existed. The
new path (`synthesize` command + `listen-test.md`) is still being tested
— this file will be deleted once that workflow is confirmed reliable.

See [listen-test.md](listen-test.md) for the current procedure.

---

1. Pick/extract a text excerpt (`Speaker 1: ` prefix per paragraph) into
   `vibe/data/<name>.txt`.
2. Normalize it:
   `cargo run --example normalize_dump -p tts < vibe/data/<name>.txt > vibe/data/<name>_normalized.txt`
3. Start the pod if it's stopped: `cargo run -- start-pod <pod_id>` (from
   `vibe/`). If the container was recreated, `vibevoice` may need
   reinstalling: `ssh ... "cd /workspace/VibeVoice && pip install -e ."`.
4. Upload the normalized text:
   `scp -P <port> -o StrictHostKeyChecking=accept-new vibe/data/<name>_normalized.txt root@<ip>:/workspace/VibeVoice/demo/`
5. Run inference on the pod:
   `cd /workspace/VibeVoice && python demo/inference_from_file.py --model_path vibevoice/VibeVoice-1.5B --txt_path demo/<name>_normalized.txt --speaker_names Sarah --cfg_scale 2.0 --output_dir /workspace/output_<name>`
   - Run in background/nohup and monitor for `"RTF (Real"` in the log to
     confirm completion (other warnings like the FlashAttention2 fallback
     can give false "done" signals).
6. Download the wav:
   `scp -P <port> root@<ip>:/workspace/output_<name>/*.wav vibe/data/<name>_generated.wav`
   (run scp from `vibe/`, not the repo root, or it lands in the wrong
   `data/` dir).
7. Listen, note mispronunciations/hallucinations, and record findings in
   `dev/normalize-future.md`.
8. Stop the pod when done for the session: `cargo run -- stop-pod <pod_id>`.
