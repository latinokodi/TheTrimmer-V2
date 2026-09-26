"""Makes ``backend`` importable however pytest was invoked.

The tests import ``trimmer`` and ``server`` the way the application does — from the
``backend`` directory — so that what is tested is what Electron actually spawns. A test that
imports through a different path is testing a different program.
"""

import sys
from pathlib import Path

BACKEND = Path(__file__).resolve().parent.parent
if str(BACKEND) not in sys.path:
    sys.path.insert(0, str(BACKEND))
