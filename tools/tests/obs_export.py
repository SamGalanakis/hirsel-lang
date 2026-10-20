#!/usr/bin/env python3
"""Offline evidence/time contracts and a real OTLP/HTTP fixture collector."""
import importlib.util
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('exporter', ROOT / 'tools/obs_export.py')
exporter = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(exporter)
FIX = Path(__file__).parent / 'fixtures/obs-export'
SCHEMA = json.loads((FIX / 'proto-json-schema.json').read_text())


def load(name):
    return json.loads((FIX / name).read_text())


def validate(value, message):
    """Validate emitted fields with protoc's pinned descriptors and OTLP JSON deviations.

    Strict for our sender (including unknown fields), so proto field misspellings
    cannot silently disappear in a permissive collector. No protobuf dependency.
    """
    assert type(value) is dict, message
    fields = SCHEMA['messages'][message]
    oneofs = set()
    for name, values in value.items():
        assert name in fields, (message, name)
        field = fields[name]
        if 'oneof' in field:
            assert field['oneof'] not in oneofs
            oneofs.add(field['oneof'])
        if field.get('repeated'):
            assert type(values) is list
        else:
            values = [values]
        for v in values:
            kind = field['type']
            if kind in SCHEMA['messages']:
                validate(v, kind)
            elif kind in SCHEMA['enums']:
                assert type(v) is int and v in SCHEMA['enums'][kind]
            elif kind in ('fixed64', 'uint64', 'int64', 'sfixed64', 'sint64'):
                assert type(v) is str and str(int(v)) == v
                signed = kind in ('int64', 'sfixed64', 'sint64')
                assert (-2**63 if signed else 0) <= int(v) < (2**63 if signed else 2**64)
            elif kind in ('double', 'float'):
                assert type(v) in (int, float)
            elif kind == 'string':
                assert type(v) is str
            elif kind == 'bytes':
                assert type(v) is str
                if name in ('traceId', 'spanId', 'parentSpanId'):
                    assert len(v) == (32 if name == 'traceId' else 16)
                    int(v, 16)
                else:
                    import base64
                    base64.b64decode(v, validate=True)
            elif kind == 'bool':
                assert type(v) is bool
            elif kind in ('fixed32', 'uint32', 'int32', 'sint32', 'sfixed32'):
                assert type(v) is int
            else:
                raise AssertionError(kind)


def validate_request(signal, body):
    service = {'logs': ('logs', 'Logs'), 'traces': ('trace', 'Trace'), 'metrics': ('metrics', 'Metrics')}
    package, name = service[signal]
    validate(body, f'opentelemetry.proto.collector.{package}.v1.Export{name}ServiceRequest')


class Collector:
    def __init__(self, replies):
        self.requests = []
        self.replies = iter(replies)
        owner = self
        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                body = self.rfile.read(int(self.headers['Content-Length']))
                signal = self.path.removeprefix('/v1/')
                owner.requests.append((signal, body))
                try:
                    assert self.headers['Content-Type'] == 'application/json'
                    validate_request(signal, json.loads(body))
                except Exception as error:
                    owner.error = error
                    self.send_error(400)
                    return
                response = next(owner.replies)
                if response == 'lost-ack':
                    self.connection.shutdown(socket.SHUT_RDWR)
                    self.connection.close()
                    return
                status, content = response
                payload = exporter.encoded(content)
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(payload)))
                self.send_header('Retry-After', '0')
                self.end_headers()
                self.wfile.write(payload)
            def log_message(self, *args):
                pass
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.endpoint = f'http://127.0.0.1:{self.server.server_port}'
        self.error = None
    def __enter__(self):
        self.thread.start()
        return self
    def __exit__(self, *args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        if self.error:
            raise self.error


class Export(unittest.TestCase):
    def bundle(self):
        return exporter.prepare(load('view.json'), load('projection.json'), load('correlation.json'))

    def logs(self):
        return exporter.prepare(load('view.json'))

    def deliver(self, collector, bundle=None, **kwargs):
        with tempfile.TemporaryDirectory() as temp:
            receipt = Path(temp) / 'receipt.jsonl'
            ok = exporter.send(bundle or self.logs(), collector.endpoint, 'fixture-export', receipt,
                               sleep=lambda _: None, **kwargs)
            rows = [json.loads(line) for line in receipt.read_text().splitlines()]
        return ok, rows

    def test_pinned_reference_examples_and_emitted_captured_payloads_validate(self):
        self.assertEqual(SCHEMA['pin'], exporter.PIN)
        for signal, name in [('logs', 'logs'), ('traces', 'trace'), ('metrics', 'metrics')]:
            validate_request(signal, load(name + '.json'))
        with Collector([(200, {})] * 3) as collector:
            ok, rows = self.deliver(collector, self.bundle())
            self.assertTrue(ok)
            for signal, payload in collector.requests:
                self.assertEqual(payload, (FIX / f'captured-{signal}.json').read_bytes())
            self.assertEqual(rows[0]['evidence'], load('view.json')['evidence'])
            self.assertEqual(rows[-1], {'state': 'finished', 'success': True})

    def test_severity_ids_units_original_time_and_exact_integer_mapping(self):
        requests = {r['signal']: r['body'] for r in self.bundle()['requests']}
        logs = requests['logs']['resourceLogs'][0]['scopeLogs'][0]['logRecords']
        self.assertEqual([r['severityNumber'] for r in logs], [5, 9, 13, 17])
        self.assertEqual(logs[0]['traceId'], '5b8efff798038103d269b633813fc60c')
        self.assertEqual(logs[0]['spanId'], 'eee19b7ec3c1b174')
        self.assertEqual(logs[0]['timeUnixNano'], '1544712660000000000')
        self.assertNotIn('observedTimeUnixNano', logs[0])
        span = requests['traces']['resourceSpans'][0]['scopeSpans'][0]['spans'][0]
        self.assertEqual(int(span['endTimeUnixNano']) - int(span['startTimeUnixNano']), 100)
        self.assertEqual(span['parentSpanId'], 'eee19b7ec3c1b173')
        self.assertEqual(span['links'][0]['spanId'], 'eee19b7ec3c1b172')
        metrics = requests['metrics']['resourceMetrics'][0]['scopeMetrics'][0]['metrics']
        histogram = metrics[0]
        self.assertEqual(histogram['unit'], 'ns')
        point = histogram['histogram']['dataPoints'][0]
        self.assertEqual(point['bucketCounts'], ['1', '1'])
        self.assertEqual(point['count'], '2')
        self.assertEqual(metrics[1]['sum']['dataPoints'][0]['asInt'], '9223372036854775807')
        self.assertFalse(any(a['key'].startswith('hirsel.ordinal') for a in point['attributes']))

    def test_missing_time_refuses_spans_metrics_and_never_uses_monotonic_as_epoch(self):
        view, projection = load('view.json'), load('projection.json')
        with self.assertRaisesRegex(ValueError, 'original timing'):
            exporter.prepare(view, projection)
        view['events'][1]['original_time_ns'] = None
        with self.assertRaisesRegex(ValueError, 'original timing'):
            exporter.prepare(view, projection, load('correlation.json'))
        untimed = self.logs()['requests'][0]['body']['resourceLogs'][0]['scopeLogs'][0]['logRecords']
        self.assertTrue(all('timeUnixNano' not in r for r in untimed))
        self.assertTrue(all('observedTimeUnixNano' not in r for r in untimed))

    def test_evidence_ids_units_histogram_and_numeric_refusals_are_atomic(self):
        for key in ('trace_id', 'span_id'):
            projection = load('projection.json')
            projection['records'][0][key] = '0' * (32 if key == 'trace_id' else 16)
            with self.assertRaisesRegex(ValueError, 'ID'):
                exporter.prepare(load('view.json'), projection, load('correlation.json'))
        for value in [None, 'wrong', 'n s', 'μs']:
            projection = load('projection.json')
            projection['records'][5]['unit'] = value
            if value == 'wrong':  # User units are opaque ASCII; no fabricated conversion.
                bundle = exporter.prepare(load('view.json'), projection, load('correlation.json'))
                self.assertIn('wrong', str(bundle))
            else:
                with self.assertRaises(ValueError):
                    exporter.prepare(load('view.json'), projection, load('correlation.json'))
        for field, value in [('bounds', [1, 1]), ('bounds', [2**53 + 1]),
                             ('bucket_counts', [2**64, 0]), ('sum', 2**53 + 1), ('end', 99)]:
            projection = load('projection.json')
            projection['records'][5][field] = value
            with self.assertRaises(ValueError):
                exporter.prepare(load('view.json'), projection, load('correlation.json'))
        for which in ('projection', 'correlation'):
            projection, correlation = load('projection.json'), load('correlation.json')
            (projection if which == 'projection' else correlation)['evidence']['capture'] = 'other'
            with self.assertRaisesRegex(ValueError, 'evidence'):
                exporter.prepare(load('view.json'), projection, correlation)
        view = load('view.json')
        view['timing']['clock'] = 'replay-duration'
        with self.assertRaisesRegex(ValueError, 'original timing'):
            exporter.prepare(view, load('projection.json'), load('correlation.json'))

    def test_partial_success_warning_and_empty_partial_are_never_retried(self):
        for partial, expected, state in [({'rejectedLogRecords': '1', 'errorMessage': 'bad log'}, False, 'partial-success'),
                                         ({'rejectedLogRecords': '0', 'errorMessage': 'warning'}, True, 'partial-success'),
                                         ({}, True, 'success')]:
            with Collector([(200, {'partialSuccess': partial})]) as collector:
                ok, rows = self.deliver(collector, attempts=3)
                self.assertEqual(ok, expected)
                self.assertEqual(len(collector.requests), 1)
                self.assertEqual(rows[-2]['state'], state)

    def test_lost_ack_retries_identical_bytes_and_records_possible_duplicates(self):
        with Collector(['lost-ack', (200, {})]) as collector:
            ok, rows = self.deliver(collector, attempts=2)
            self.assertTrue(ok)
            self.assertEqual(len(collector.requests), 2)
            self.assertEqual(collector.requests[0], collector.requests[1])
            self.assertTrue(next(r for r in rows if r.get('state') == 'unacknowledged')['duplication_possible'])
            self.assertTrue(rows[-2]['duplication_possible'])

    def test_retryable_statuses_only_and_protocol_errors_are_terminal(self):
        for status in (400, 401, 413, 500, 429, 502, 503, 504):
            retry = status in (429, 502, 503, 504)
            with Collector([(status, {}), (200, {})]) as collector:
                ok, rows = self.deliver(collector, attempts=2)
                self.assertEqual(ok, retry)
                self.assertEqual(len(collector.requests), 2 if retry else 1)
        for content in ({'partialSuccess': {'rejectedLogRecords': '-1'}}, [],
                        {'partialSuccess': {'rejectedLogRecords': True}}):
            with Collector([(200, content)]) as collector:
                ok, rows = self.deliver(collector, attempts=2)
                self.assertFalse(ok)
                self.assertEqual(len(collector.requests), 1)
                self.assertEqual(rows[-2]['state'], 'invalid-response')

    def test_reexport_has_distinct_provenance_and_preserves_capture(self):
        with Collector([(200, {})]) as collector:
            ok, rows = self.deliver(collector, reexport_of='first-export')
            self.assertTrue(ok)
            self.assertEqual(rows[0]['reexport_of'], 'first-export')
            attrs = json.loads(collector.requests[0][1])['resourceLogs'][0]['resource']['attributes']
            attrs = {a['key']: a['value'] for a in attrs}
            self.assertEqual(attrs['hirsel.export.mode'], {'stringValue': 're-export'})
            self.assertEqual(attrs['hirsel.export.reexport_of'], {'stringValue': 'first-export'})
            self.assertEqual(attrs['hirsel.evidence.capture']['stringValue'], 'fixture')

    def test_replay_analysis_and_offline_preparation_produce_zero_export_requests(self):
        view = load('view.json')
        view['replay_observer'] = dict(analysis='replay-1', evidence=view['evidence'], effects=[
            dict(origin='replay-observer', effect='must never export this')])
        with (patch.object(exporter, 'post', side_effect=AssertionError('export during replay')),
              patch.object(exporter.time, 'time', side_effect=AssertionError('clock during replay'))):
            bundle = exporter.prepare(view)
        self.assertEqual(len(bundle['requests'][0]['body']['resourceLogs'][0]['scopeLogs'][0]['logRecords']), 2)
        self.assertNotIn('must never export', str(bundle))
        self.assertEqual(bundle['replay_analysis'], 'replay-1')
        view['events'][0]['origin'] = 'replay-observer'
        with self.assertRaisesRegex(ValueError, 'production'):
            exporter.prepare(view)

    def test_prefix_completeness_and_accepted_bytes_survive_export(self):
        view = load('view.json')
        view['complete'] = False
        view['events'][1]['accepted_bytes'] = 2
        body = exporter.prepare(view)['requests'][0]['body']
        resource = body['resourceLogs'][0]
        self.assertIn(exporter.attribute('hirsel.evidence.complete', False), resource['resource']['attributes'])
        self.assertIn(exporter.attribute('hirsel.accepted_bytes', '2'), resource['scopeLogs'][0]['logRecords'][1]['attributes'])

    def test_receipt_exclusive_and_redirect_cannot_expand_endpoint_grant(self):
        with tempfile.TemporaryDirectory() as temp:
            receipt = Path(temp) / 'receipt'
            receipt.write_text('existing intent')
            with self.assertRaises(FileExistsError):
                exporter.send(self.logs(), 'http://localhost', 'id', receipt,
                              transport=lambda *args: self.fail('network before receipt'))
            self.assertEqual(receipt.read_text(), 'existing intent')
        with Collector([(302, {})]) as collector:
            ok, rows = self.deliver(collector, attempts=2)
            self.assertFalse(ok)
            self.assertEqual(len(collector.requests), 1)

    def test_cli_prepare_and_send_require_explicit_independent_grants(self):
        tool = ROOT / 'tools/obs_export.py'
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / 'bundle'
            result = subprocess.run([sys.executable, tool, 'prepare', FIX / 'view.json', '-o', output],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(output.read_text()), self.logs())
            with Collector([(200, {})]) as collector:
                result = subprocess.run([sys.executable, tool, 'send', output, '--endpoint', collector.endpoint,
                    '--export-id', 'cli-export', '--receipt', str(Path(temp) / 'receipt')], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(collector.requests), 1)


if __name__ == '__main__':
    unittest.main()
