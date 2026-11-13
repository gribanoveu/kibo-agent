import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from lib import check

# The collection's script signs in by POST: the one write a read-only task allows.
check(lambda hits: any(h["planted"] for h in hits), writes_to=("/login",))
