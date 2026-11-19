import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from lib import check

check(lambda hits: any(h["planted"] for h in hits))
