#!/usr/bin/env python3
"""Prepare evidence-bound OTLP JSON offline; send only with explicit transport/storage grants."""
import argparse
import copy
import datetime
import email.utils
import hashlib
import http.client
import json
import math
from pathlib import Path
import random
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

PIN = '09769935deb8b2ebcf73ad1e2142b691d17bf70b'
MAX_REQUEST = 64 * 1024 * 1024
MAX_RESPONSE = 4 * 1024 * 1024
SEVERITY = {'Debug': 5, 'Info': 9, 'Warn': 13, 'Error': 17}
SIGNALS = {'logs': ('resourceLogs', 'scopeLogs', 'logRecords', 'rejectedLogRecords'),
           'traces': ('resourceSpans', 'scopeSpans', 'spans', 'rejectedSpans'),
           'metrics': ('resourceMetrics', 'scopeMetrics', 'metrics', 'rejectedDataPoints')}


def require(ok, message):
    if not ok:
        raise ValueError(message)


def integer(value, bits=64, signed=False):
    low = -(1 << (bits - 1)) if signed else 0
    high = (1 << (bits - (1 if signed else 0))) - 1
    require(type(value) is int and low <= value <= high, f'expected {bits}-bit integer')
    return value


def text(value, maximum=4096):
    require(isinstance(value, str) and 0 < len(value.encode('utf-8')) <= maximum,
            'expected bounded nonempty text')
    return value


def identity(value, size):
    require(isinstance(value, str) and re.fullmatch('[0-9a-fA-F]{' + str(size * 2) + '}', value)
            and int(value, 16) != 0, 'invalid nonzero trace/span ID')
    return value.lower()


def digest(value):
    return hashlib.sha256(encoded(value)).hexdigest()


def encoded(value):
    return (json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False) + '\n').encode()


def attribute(key, value):
    if type(value) is bool:
        v = {'boolValue': value}
    elif type(value) is int:
        v = {'intValue': str(integer(value, signed=True))}
    else:
        require(isinstance(value, str), 'attribute must be a scalar string/bool/i64')
        v = {'stringValue': value}
    return {'key': key, 'value': v}


def attributes(values):
    require(isinstance(values, dict) and len(values) <= 128, 'too many/invalid attributes')
    for key, value in values.items():
        text(key, 256)
        require(not key.startswith('hirsel.'), 'hirsel.* attributes are reserved for evidence')
        if isinstance(value, str):
            require(len(value.encode()) <= 4096, 'attribute value too large')
    return [attribute(k, v) for k, v in sorted(values.items())]


def prepare(view, projection=None, correlation=None):
    """No I/O, clock or network. Typed projection is explicit analysis, never inferred evidence."""
    require(isinstance(view, dict), 'expected observation view object')
    require(view.get('schema') == 'hirsel-observation-view' and view.get('version') == 1,
            'unsupported observation view')
    evidence = view.get('evidence', {})
    text(evidence.get('capture'), 256)
    require(re.fullmatch('[0-9a-f]{64}', evidence.get('sha256', '')), 'invalid evidence digest')
    require(type(view.get('complete')) is bool, 'missing completeness')
    events = view.get('events')
    require(isinstance(events, list), 'missing events')
    for ordinal, event in enumerate(events):
        require(type(event.get('ordinal')) is int and event['ordinal'] == ordinal,
                'invalid event order')
        require(event.get('origin') in ('application', 'core'), 'invalid production event origin')
    analysis = view.get('replay_observer')
    if analysis is not None:
        require(analysis.get('evidence') == evidence, 'replay analysis evidence mismatch')
    if projection is not None:
        require(projection.get('schema') == 'hirsel-export-projection' and projection.get('version') == 1
                and projection.get('evidence') == evidence, 'projection schema/evidence mismatch')
        text(projection.get('analysis'), 256)
        rows = projection.get('records')
        require(isinstance(rows, list), 'missing projection records')
    else:
        rows = [dict(kind='log', ordinal=e['ordinal'], level='Info',
                     message=e['request'] + ' -> ' + e['completion']) for e in events]
    require(len(rows) <= 100000, 'too many export records')
    if correlation is not None:
        require(correlation.get('schema') == 'hirsel-original-clock-correlation'
                and correlation.get('version') == 1 and correlation.get('evidence') == evidence,
                'clock correlation schema/evidence mismatch')
        text(correlation.get('provenance'), 4096)
        integer(correlation.get('monotonic_ns'))
        integer(correlation.get('unix_ns'))
        require(view.get('timing', {}).get('clock') == 'host-monotonic-ns',
                'clock correlation requires original timing sidecar samples')
    def event_at(ordinal):
        integer(ordinal)
        require(ordinal < len(events), 'projection ordinal absent from evidence prefix')
        return events[ordinal]
    def timestamp(ordinal, required=False):
        sample = event_at(ordinal).get('original_time_ns')
        if correlation is None or sample is None:
            require(not required, 'original timing and Unix correlation required; logical/replay time refused')
            return None
        integer(sample)
        ns = integer(correlation['unix_ns'] + sample - correlation['monotonic_ns'])
        require(ns > 0, 'OTLP timestamp must be nonzero')
        return str(ns)
    def interval(row):
        start = integer(row.get('start'))
        end = integer(row.get('end'))
        require(start <= end, 'reversed ordinal interval')
        a, b = timestamp(start, True), timestamp(end, True)
        require(int(a) <= int(b), 'reversed original time interval')
        return a, b
    def context(row):
        require(('trace_id' in row) == ('span_id' in row), 'trace/span IDs must occur together')
        if 'trace_id' not in row:
            return {}
        return {'traceId': identity(row['trace_id'], 16), 'spanId': identity(row['span_id'], 8)}
    resource = [attribute('hirsel.evidence.capture', evidence['capture']),
                attribute('hirsel.evidence.sha256', evidence['sha256']),
                attribute('hirsel.evidence.complete', view['complete']),
                attribute('hirsel.mapping.version', 'otlp@' + PIN),
                attribute('hirsel.time.provenance', 'original-correlated' if correlation else 'untimed')]
    if correlation:
        resource += [attribute('hirsel.time.correlation.sha256', digest(correlation)),
                     attribute('hirsel.time.correlation.provenance', correlation['provenance'])]
    if projection:
        resource += [attribute('hirsel.projection.analysis', projection['analysis']),
                     attribute('hirsel.projection.sha256', digest(projection))]
    signals = {signal: [] for signal in SIGNALS}
    for row in rows:
        require(isinstance(row, dict), 'invalid projected record')
        require(row.get('origin', 'application') in ('application', 'core'), 'replay observations are not production traffic')
        kind = row.get('kind')
        ordinal = row.get('ordinal') if kind == 'log' else row.get('end')
        event = event_at(ordinal)
        attrs = attributes(row.get('attributes', {})) + [attribute('hirsel.ordinal', str(ordinal)),
                attribute('hirsel.origin', event['origin'])]
        if event.get('original_time_ns') is not None:
            attrs.append(attribute('hirsel.original.monotonic_ns', str(integer(event['original_time_ns']))))
        if event.get('accepted_bytes') is not None:
            attrs.append(attribute('hirsel.accepted_bytes', str(integer(event['accepted_bytes']))))
        if kind == 'log':
            level = row.get('level')
            require(level in SEVERITY, 'unknown log severity')
            record = dict(severityNumber=SEVERITY[level], severityText=level,
                          body={'stringValue': text(row.get('message'), 65536)}, attributes=attrs, **context(row))
            ns = timestamp(ordinal)
            if ns is not None:
                record['timeUnixNano'] = ns
            signals['logs'].append(record)
        elif kind == 'span':
            start, end = interval(row)
            ctx = context(row)
            require(bool(ctx), 'span requires trace and span IDs')
            kinds = {'Internal': 1, 'Server': 2, 'Client': 3, 'Producer': 4, 'Consumer': 5}
            require(row.get('span_kind') in kinds, 'invalid span kind')
            record = dict(name=text(row.get('name')), startTimeUnixNano=start,
                          endTimeUnixNano=end, kind=kinds[row['span_kind']], attributes=attrs, **ctx)
            if row.get('parent_span_id') is not None:
                record['parentSpanId'] = identity(row['parent_span_id'], 8)
            require(isinstance(row.get('links', []), list) and len(row.get('links', [])) <= 128, 'too many span links')
            record['links'] = [dict(traceId=identity(link['trace_id'], 16),
                                    spanId=identity(link['span_id'], 8)) for link in row.get('links', [])]
            status = row.get('status', 'Unset')
            require(status in ('Unset', 'Success', 'Error'), 'span outcome must be explicit')
            record['status'] = {'code': {'Unset': 0, 'Success': 1, 'Error': 2}[status]}
            signals['traces'].append(record)
        elif kind in ('counter', 'updowncounter', 'gauge', 'histogram'):
            start, end = interval(row)
            unit = text(row.get('unit'), 63)
            require(unit.isascii() and not any(c.isspace() for c in unit), 'unit must be ASCII without whitespace')
            point = dict(startTimeUnixNano=start, timeUnixNano=end, attributes=attributes(row.get('attributes', {})))
            record = dict(name=text(row.get('name'), 255), unit=unit)
            if kind == 'histogram':
                bounds, counts = row.get('bounds'), row.get('bucket_counts')
                require(isinstance(bounds, list) and len(bounds) <= 1024 and isinstance(counts, list)
                        and len(counts) == len(bounds) + 1, 'invalid histogram buckets')
                for n in bounds:
                    integer(n)
                    require(n <= 2**53, 'histogram bound cannot be represented exactly in OTLP double')
                require(all(a < b for a, b in zip(bounds, bounds[1:])), 'histogram bounds must increase')
                for n in counts:
                    integer(n)
                count = integer(sum(counts))
                total = integer(row.get('sum'))
                require(total <= 2**53, 'histogram sum cannot be represented exactly in OTLP double')
                point.update(count=str(count), sum=total, explicitBounds=bounds,
                             bucketCounts=[str(n) for n in counts])
                record['histogram'] = dict(aggregationTemporality=2, dataPoints=[point])
            else:
                value = integer(row.get('value'), signed=kind != 'counter')
                # OTLP NumberDataPoint.asInt is signed even for monotonic counters.
                integer(value, signed=True)
                point['asInt'] = str(value)
                if kind == 'gauge':
                    record['gauge'] = dict(dataPoints=[point])
                else:
                    record['sum'] = dict(aggregationTemporality=2, isMonotonic=kind == 'counter', dataPoints=[point])
            existing = next((m for m in signals['metrics'] if m['name'] == record['name']), None)
            if existing:
                require(existing['unit'] == unit and set(existing) == set(record)
                        and (kind == 'gauge' or kind == 'histogram'
                             or existing['sum']['isMonotonic'] == record['sum']['isMonotonic']),
                        'incompatible metric descriptor in one scope')
                data = 'gauge' if kind == 'gauge' else 'histogram' if kind == 'histogram' else 'sum'
                existing[data]['dataPoints'].extend(record[data]['dataPoints'])
            else:
                signals['metrics'].append(record)
        else:
            raise ValueError('unsupported projection kind')
    requests = []
    for signal, records in signals.items():
        if records:
            root, scope, field, _ = SIGNALS[signal]
            body = {root: [dict(resource={'attributes': resource}, **{scope: [dict(
                scope={'name': 'hirsel.observation', 'version': '1'}, **{field: records})]})]}
            require(len(encoded(body)) <= MAX_REQUEST, 'OTLP request exceeds size limit')
            requests.append(dict(signal=signal, body=body))
    return dict(schema='hirsel-otlp-bundle', version=1, mapping=PIN, evidence=evidence,
                complete=view['complete'], replay_analysis=None if analysis is None else analysis.get('analysis'),
                projection_sha256=None if projection is None else digest(projection),
                correlation_sha256=None if correlation is None else digest(correlation), requests=requests)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def post(url, body, timeout):
    """Explicit endpoint only: no ambient proxies, redirects, or application providers."""
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    req = urllib.request.Request(url, data=body, headers={'Content-Type': 'application/json'}, method='POST')
    try:
        response = opener.open(req, timeout=timeout)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        body = response.read(MAX_RESPONSE + 1)
        require(len(body) <= MAX_RESPONSE, 'collector response exceeds limit')
        return response.status, response.headers, body


def retry_delay(headers, attempt):
    value = headers.get('Retry-After')
    if value:
        try:
            seconds = int(value)
        except ValueError:
            try:
                date = email.utils.parsedate_to_datetime(value)
                seconds = (date - datetime.datetime.now(datetime.timezone.utc)).total_seconds()
            except (ValueError, TypeError, OverflowError):
                seconds = 0
        return max(0, seconds)
    return min(30, 2**(attempt - 1)) * random.uniform(0.5, 1.5)


def send(bundle, endpoint, export_id, receipt, reexport_of=None, attempts=1, timeout=10,
         transport=post, sleep=time.sleep):
    """One immutable request per signal. Persist intent before sending; partial success is final."""
    require(bundle.get('schema') == 'hirsel-otlp-bundle' and bundle.get('version') == 1
            and bundle.get('mapping') == PIN, 'unsupported export bundle')
    text(export_id, 256)
    if reexport_of is not None:
        text(reexport_of, 256)
        require(reexport_of != export_id, 're-export must have a new export identity')
    require(type(attempts) is int and 1 <= attempts <= 10, 'attempts must be 1..10')
    require(type(timeout) in (float, int) and math.isfinite(timeout) and 0 < timeout <= 300,
            'timeout must be in (0,300] seconds')
    url = urllib.parse.urlsplit(endpoint)
    require(url.scheme in ('http', 'https') and url.hostname and not url.username and not url.password
            and not url.query and not url.fragment, 'endpoint must be an explicit HTTP(S) base URL')
    requests = copy.deepcopy(bundle.get('requests'))
    require(isinstance(requests, list) and len(requests) <= 3, 'invalid export requests')
    prepared, seen = [], set()
    for request in requests:
        signal = request['signal']
        require(signal in SIGNALS and signal not in seen, 'duplicate/invalid signal')
        seen.add(signal)
        root = SIGNALS[signal][0]
        for resource in request['body'][root]:
            attrs = resource['resource']['attributes']
            attrs += [attribute('hirsel.export.id', export_id),
                      attribute('hirsel.export.mode', 're-export' if reexport_of else 'export')]
            if reexport_of:
                attrs.append(attribute('hirsel.export.reexport_of', reexport_of))
        body = encoded(request['body'])
        require(len(body) <= MAX_REQUEST, 'OTLP request exceeds size limit')
        prepared.append((signal, body))
    # Exclusive creation prevents an accidental rerun from overwriting delivery evidence.
    with Path(receipt).open('x') as ledger:
        def record(row):
            import os
            ledger.write(encoded(row).decode())
            ledger.flush()
            os.fsync(ledger.fileno())
        record(dict(schema='hirsel-otlp-export', version=1, export_id=export_id, reexport_of=reexport_of,
                    evidence=bundle['evidence'], complete=bundle['complete'], mapping=PIN,
                    bundle_sha256=digest(bundle), replay_analysis=bundle.get('replay_analysis'),
                    endpoint=endpoint, duplication_possible=bool(reexport_of)))
        all_ok = True
        for signal, body in prepared:
            ambiguous = bool(reexport_of)
            for attempt in range(1, attempts + 1):
                record(dict(signal=signal, attempt=attempt, state='intent', request_sha256=hashlib.sha256(body).hexdigest(),
                            duplication_possible=ambiguous))
                headers = {}
                try:
                    status, headers, response = transport(endpoint.rstrip('/') + '/v1/' + signal, body, timeout)
                    require(len(response) <= MAX_RESPONSE, 'collector response exceeds limit')
                    if status == 200:
                        require(headers.get('Content-Type', '').split(';')[0].strip() == 'application/json',
                                'collector must return application/json')
                        reply = json.loads(response)
                        require(isinstance(reply, dict), 'invalid collector response')
                        partial = reply.get('partialSuccess', {})
                        require(isinstance(partial, dict), 'invalid partial success')
                        raw = partial.get(SIGNALS[signal][3], '0')
                        require(type(raw) in (int, str) and re.fullmatch('[0-9]+', str(raw)), 'invalid rejection count')
                        rejected = integer(int(raw), signed=True)
                        message = partial.get('errorMessage', '')
                        require(isinstance(message, str), 'invalid collector errorMessage')
                        state = 'partial-success' if rejected or message else 'success'
                        record(dict(signal=signal, attempt=attempt, state=state, rejected=rejected,
                                    message=message, duplication_possible=ambiguous))
                        all_ok &= rejected == 0
                        break
                    retryable = status in (429, 502, 503, 504)
                    record(dict(signal=signal, attempt=attempt, state='http-error', status=status,
                                retryable=retryable, duplication_possible=ambiguous))
                except (OSError, urllib.error.URLError, http.client.HTTPException) as error:
                    ambiguous = True
                    retryable = True
                    record(dict(signal=signal, attempt=attempt, state='unacknowledged', error=str(error),
                                duplication_possible=True))
                except (ValueError, TypeError) as error:
                    record(dict(signal=signal, attempt=attempt, state='invalid-response', error=str(error),
                                duplication_possible=True))
                    all_ok = False
                    break
                if not retryable or attempt == attempts:
                    all_ok = False
                    break
                ambiguous = True
                sleep(retry_delay(headers, attempt))
        record(dict(state='finished', success=all_ok))
        return all_ok


def load(path):
    return json.loads(Path(path).read_text())


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('prepare', help='offline conversion; no transport or clock access')
    p.add_argument('view')
    p.add_argument('--projection')
    p.add_argument('--correlation')
    p.add_argument('-o', '--output', required=True)
    s = sub.add_parser('send', help='explicitly grant collector transport and receipt storage')
    s.add_argument('bundle')
    s.add_argument('--endpoint', required=True)
    s.add_argument('--export-id', required=True)
    s.add_argument('--receipt', required=True)
    s.add_argument('--reexport-of')
    s.add_argument('--attempts', type=int, default=1)
    s.add_argument('--timeout', type=float, default=10)
    args = parser.parse_args(argv)
    try:
        if args.command == 'prepare':
            bundle = prepare(load(args.view), load(args.projection) if args.projection else None,
                             load(args.correlation) if args.correlation else None)
            with Path(args.output).open('xb') as output:
                output.write(encoded(bundle))
            return 0
        return 0 if send(load(args.bundle), args.endpoint, args.export_id, args.receipt,
                         args.reexport_of, args.attempts, args.timeout) else 1
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(f'obs-export: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
