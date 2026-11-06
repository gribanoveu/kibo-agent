#!/bin/sh
# Nothing changed, and the plan is about the file it was asked about.
set -e
diff -r -x .git -x __pycache__ "$ORIG" .
grep -q 'client.py' "$PLAN"
