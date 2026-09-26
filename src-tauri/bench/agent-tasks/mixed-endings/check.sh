#!/bin/sh
# The tests the agent saw, untouched and passing; the tiers file byte for byte.
set -e
diff -r -x __pycache__ "$ORIG/tests" tests
python3 -m unittest discover -s tests 2>&1
python3 "$TASK/check.py"
