"""Semantic CPU requirements, independent of measured resource usage.

These harnesses assert physical worker widths. Exact counts override measured
CPUs and focus mode; measured memory and CPU class remain authoritative.
"""

EXACT_TEST_CPUS = {
    "hirsel-cli-test-cli": 4,  # pubsub and suite-runner native width four
    "hirsel-cli-test-free": 8,  # F1 actual-eight admission and replay matrices
}
