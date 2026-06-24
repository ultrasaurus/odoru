#!/bin/bash
set -e

mkdir -p /root/.ssh
chmod 700 /root/.ssh
if [ -n "$PUBLIC_KEY" ]; then
    echo "$PUBLIC_KEY" >> /root/.ssh/authorized_keys
    chmod 600 /root/.ssh/authorized_keys
fi

ssh-keygen -A
/usr/sbin/sshd

# Durable job state on RunPod (non-GCP, no metadata server): decode the
# base64'd GCS service-account key into a file and point vibe-service at it.
# On Cloud Run this is unset — ambient metadata credentials are used instead.
if [ -n "$GCS_SA_KEY_B64" ]; then
    echo "$GCS_SA_KEY_B64" | base64 -d > /root/gcs-sa-key.json
    chmod 600 /root/gcs-sa-key.json
    export GCS_SA_KEY_PATH=/root/gcs-sa-key.json
fi

exec /usr/local/bin/vibe-service
