#!/usr/bin/env python3
"""Runs the Hirsel conformance testee for conformance_test_runner.

    conformance_test_runner [flags] packages/protobuf@1/conformance/adapter.py

The testee speaks the runner's length-prefixed binary protocol directly. This
launcher supplies the stdin and stdout grants. TESTEE overrides the native build
`testee` beside this script.
"""
import os
import shlex

here = os.path.dirname(os.path.abspath(__file__))
grants = '{"io":{"stdin":"stdin.bytes@1","stdout":"stdout.bytes@1","dirs":[]}}'
command = shlex.split(os.environ.get('TESTEE', '')) or [
    os.path.join(here, 'testee'), '--hirsel-grants', grants]
os.execvp(command[0], command)
