import json
import sys

from helper import local

assert local == "local-import-ok"
json.dump(json.load(sys.stdin), sys.stdout)
