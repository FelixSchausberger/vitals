#!/usr/bin/env bash
for f; do [ -s "$f" ] && [ "$(od -An -tx1 -N1 "$f" | tr -d ' ')" != "0a" ] && echo "" >>"$f"; done
