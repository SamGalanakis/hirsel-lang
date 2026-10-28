"""Infrastructure diagnostics shared by main's stage and test verdicts.

Exit conventions come from Python subprocess (negative signal numbers),
GNU timeout (124) and tools/capped:165-200 (128 + signal, memory/timeout notes).
Buck2's TIMED_OUT status is defined in event_logs.py.
"""
import re
import signal


# Require failure diagnostics, not just words such as "memory" or "remote" in
# test names. In particular, an ordinary assertion failure remains a code red.
DIAGNOSTICS = (
    ('expired RE CAS artifact', r'artifact that has expired in the RE CAS'),
    ('memory limit', r'\boom[_ -]kill(?:ed)?\b|out of memory|memory limit .+ reached'),
    ('timeout', r'\btimed[ _-]?out\b|\bTimeoutExpired\b|\btimeout\s+[1-9]\d*\b|deadline exceeded'),
    ('signal', r'\bSIG(?:KILL|TERM|SEGV|ABRT|BUS|ILL|FPE|INT|QUIT|XCPU|XFSZ)\b|'
               r'(?:killed|terminated|died|death) (?:by |with )?signal\b|\bsignal[:= ]+\d+\b'),
    ('remote execution', r'\binfra failure\b(?!\s+0\b)|\bRE (?:infrastructure )?error\b|'
                         r'remote[ _-]execution\b[^\n]*(?:error|fail|unavailable)|'
                         r'(?:error|fail)[^\n]*remote[ _-]execution|'
                         r'\b(?:RemoteExecutionError|RemoteError|REClientError)\b|'
                         r'no remote-execution (?:configuration|hook)'),
    ('pool unavailable', r'\bpool\b[^\n]*(?:\bdown\b|\bunavailable\b|\bunreachable\b|\bno workers\b)|'
                         r'\b(?:connection refused|failed to connect|transport error)\b'),
)


def infra_reason(exit_code=0, details='', *, timed_out=False):
    """Return a short infra reason, or None for an ordinary code failure."""
    details = re.sub(r'\x1b\[[0-9;]*m', '', details)
    # Query's Debug source map contains program text, including timeout literals.
    details = re.sub(r', sources: SourceMap \{[^\n]*', '', details)
    embedded = re.search(r'\b(?:exit(?:ed)?(?: with)?(?: status| code)?|return code)[:= ]+(-?\d+)\b', details, re.I)
    if not exit_code and embedded:
        exit_code = int(embedded[1])
    if exit_code == 71:
        return 'memory limit (exit 71)'
    if timed_out or exit_code == 124:
        return 'timeout'
    number = -exit_code if exit_code < 0 else exit_code - 128
    if number in signal.valid_signals() and number > 0:
        return f'signal {number}'
    for reason, pattern in DIAGNOSTICS:
        if re.search(pattern, details, re.I):
            return reason
    return None
