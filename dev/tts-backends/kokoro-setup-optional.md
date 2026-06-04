# Kokoro developement

kokoro is installed in ~/.kokoro

## download all english voices

```
cd ~/.kokoro/voices
python -c "
from huggingface_hub import hf_hub_download, list_repo_files
import os

repo = 'onnx-community/Kokoro-82M-v1.0-ONNX'
prefixes = ('af_', 'am_', 'bf_', 'bm_')

voices = [
    f.replace('voices/', '')
    for f in list_repo_files(repo)
    if f.startswith('voices/') and f.endswith('.bin')
    and any(f[len('voices/'):].startswith(p) for p in prefixes)
]

for v in sorted(voices):
    if os.path.exists(v):
        print(f'Skipping {v} (already exists)')
        continue
    print(f'Downloading {v}...')
    hf_hub_download(repo, filename=f'voices/{v}', local_dir='tmp_dl')
    os.rename(f'tmp_dl/voices/{v}', v)

import shutil
if os.path.exists('tmp_dl'):
    shutil.rmtree('tmp_dl')

print(f'Done: {len(voices)} voices')
"
```