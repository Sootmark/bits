"""Run a BitsParser checkout (the first argument) with the arguments after it.

BitsParser imports its own modules from its directory, which `python3 -I`
leaves off the path: this puts the checkout there, and nothing else.
"""

import runpy
import sys

checkout = sys.argv[1]
sys.path.insert(0, checkout)
sys.argv = [checkout + "/BitsParser.py"] + sys.argv[2:]
runpy.run_path(checkout + "/BitsParser.py", run_name="__main__")
