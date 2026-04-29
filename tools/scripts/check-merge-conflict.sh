#!/usr/bin/env bash
for f; do grep -qEn "^(<<<<<<<|=======|>>>>>>>|=======|<<<<<<<)" "$f" 2>/dev/null && exit 1; done
exit 0
