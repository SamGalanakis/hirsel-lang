#!/usr/bin/env python3
"""Retention protects in-flight logs and decodes archived commands without builds."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
# This test has the module's filename, so load its implementation explicitly.
SPEC = importlib.util.spec_from_file_location('retained_events', Path(__file__).resolve().parents[1] / 'buck2/event_logs.py')
logs = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(logs)


class Retention(unittest.TestCase):
    def test_age_and_size_prune_oldest_but_protect_live_writers_and_readers(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            old, newer = root / 'old.pb.zst', root / 'new.pb.zst'
            old.write_bytes(b'123456')
            newer.write_bytes(b'1234')
            os.utime(old, (100, 100))
            os.utime(newer, (200, 200))
            logs.prune(root, now=201, max_age=1000, max_bytes=5)
            self.assertFalse(old.exists())
            self.assertTrue(newer.exists())
            with logs.recorded('gate', root) as live:
                live.write_bytes(b'active')
                logs.prune(root, now=10**12, max_age=0, max_bytes=0)
                self.assertTrue(live.exists())
                self.assertEqual(logs.discover(root, 24), [])
            self.assertEqual(logs.discover(root, 24), [live])
            with logs.reading(live) as ready:
                self.assertTrue(ready)
                logs.prune(root, max_bytes=0)
                self.assertTrue(live.exists())
            logs.prune(root, max_bytes=0)
            self.assertFalse(live.exists())

    def test_failed_runs_are_retained_and_build_flags_do_not_change(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            buck = root / 'buck'
            buck.write_text('#!' + sys.executable + '\nimport sys,json,pathlib\n'
                            'args=sys.argv[1:]\n'
                            'if "--event-log" in args: pathlib.Path(args[args.index("--event-log")+1]).write_text("failure")\n'
                            'print(json.dumps(args))\nsys.exit(7 if args[0]=="build" else 0)\n')
            buck.chmod(0o755)
            with patch.dict(os.environ, {'HIRSEL_BUCK2_LOG_DIR': str(root / 'logs')}):
                result = logs.run(['build', '-c', 'hirsel.tests=focus', '//:one'], buck2=buck,
                                  capture_output=True, text=True)
                self.assertEqual(result.returncode, 7)
                args = json.loads(result.stdout)
                self.assertEqual([args[0], *args[3:]], ['build', '-c', 'hirsel.tests=focus', '//:one'])
                self.assertEqual(len(logs.discover(root / 'logs', 24)), 1)
                with self.assertRaises(RuntimeError), logs.recorded('interrupted') as failed:
                    failed.write_bytes(b'partial')
                    raise RuntimeError('interrupted')
                self.assertTrue(failed.exists())
                query = logs.run(['uquery', '//:one'], buck2=buck, capture_output=True, text=True)
                self.assertEqual(json.loads(query.stdout), ['uquery', '//:one'])

    def test_compressed_inputs_use_the_decoder_without_an_isolation_override(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            path = root / 'events.pb.zst'
            path.write_text('{"event": 1}\n')
            buck = root / 'buck'
            buck.write_text('#!' + sys.executable + '\nimport sys,pathlib\n'
                            'assert sys.argv[1:3]==["log","show"],sys.argv\n'
                            'print(pathlib.Path(sys.argv[3]).read_text(),end="")\n')
            buck.chmod(0o755)
            self.assertEqual(list(logs.lines(path, buck2=buck)), ['{"event": 1}\n'])


if __name__ == '__main__':
    unittest.main()
