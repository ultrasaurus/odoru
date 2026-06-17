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

exec tail -f /dev/null
