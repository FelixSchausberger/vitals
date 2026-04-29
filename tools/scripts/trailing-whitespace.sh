#!/usr/bin/env bash
for f; do sed -i 's/[[:space:]]*$//' "$f"; done
