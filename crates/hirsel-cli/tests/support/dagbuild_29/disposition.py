"""Only explicitly retained ID/backend/adapter/exit/diagnostic refusals qualify.

A recognized failure remains INCOMPLETE, never a replay pass. New failure shapes
fail the harness. This table contains no effects, source hashes or scheduling API.
"""
import json
from pathlib import Path
import re


def known(scope,producer,case,width,engine,process):
    key='|'.join([scope,producer,case,str(width),engine])
    choices=json.loads(Path(__file__).with_name('known-incomplete.json').read_text()).get(key,[])
    match=re.search(rb'ReplayMismatch\([0-9]+\)',process.stderr)
    actual=dict(exit=process.returncode,diagnostic=match.group().decode() if match else '')
    return actual in choices
